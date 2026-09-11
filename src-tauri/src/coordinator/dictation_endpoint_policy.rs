// A short, explicit continuation word is different from an ordinary complete
// utterance. Give “然后/但是/另外/所以…” one natural thinking pause without
// slowing every recording. The host keeps firmware's fixed one-second silence
// fallback alive only inside this bounded window.
const EMBEDDED_DANGLING_CONTINUATION_END_TIMEOUT_MS: u64 = 2_500;
const EMBEDDED_SETTLED_TARGET_SCHEDULING_ALLOWANCE_MS: u64 =
    EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS - EMBEDDED_SETTLED_TARGET_WALL_CLOCK_MS;
// Leave room for the BLE command's 300ms acknowledgement timeout before the
// firmware's 1.0s fallback. The old 800ms interval had a 1.1s worst case.
const EMBEDDED_DANGLING_FIRMWARE_KEEPALIVE_INTERVAL_MS: u64 = 600;

fn preview_ends_with_sentence_terminal(preview: Option<&str>) -> bool {
    let Some(text) = preview.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    text.chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
        .is_some_and(|ch| matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | '…'))
}

#[cfg(test)]
fn target_speaker_endpoint_due_after_visible_body_gate(
    body_started: bool,
    provider_clock_endpoint_due: bool,
    settled_wall_clock_endpoint_due: bool,
) -> bool {
    // Once body text is visible, a provider frame can clear its provisional
    // tail while retaining an old speaker boundary. The provider audio clock
    // is then technically overdue and used to stop on that same frame, before
    // the guarded settled-text clock could observe another second of local
    // speech. Make the guarded wall clock authoritative for visible body text.
    // Provider-clock endpointing remains available for no-body abandonment.
    settled_wall_clock_endpoint_due || (!body_started && provider_clock_endpoint_due)
}

fn update_has_fresh_unclassified_local_speech(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    endpoint_timeout_ms: u64,
) -> bool {
    update
        .audio_duration_ms
        .zip(update.local_speech_end_ms)
        .is_some_and(|(audio_ms, speech_ms)| {
            audio_ms.saturating_sub(speech_ms) < endpoint_timeout_ms
                && !local_speech_confidently_non_target(update, speech_ms)
        })
}

/// A provider two-pass frame can add real body words while retaining a
/// slightly older utterance boundary. The settled-text clock is rearmed by the
/// preview callback, but firmware owns an independent one-second safety
/// endpoint and otherwise keeps counting from the previous `VREC:SPEECH`.
///
/// Refresh that firmware clock only when the authoritative preview gained
/// lexical content and the same update still has recent owner-compatible local
/// speech. This deliberately excludes punctuation-only revisions, provider
/// bookkeeping after another speaker, and explicit local NonTarget evidence.
fn authoritative_preview_growth_has_recent_owner_speech(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    previous_preview: Option<&str>,
    current_preview: Option<&str>,
) -> bool {
    let previous_chars = previous_preview
        .map(embedded_audio_partial_preview_stability_key)
        .map_or(0, |text| text.chars().count());
    let current_chars = current_preview
        .map(embedded_audio_partial_preview_stability_key)
        .map_or(0, |text| text.chars().count());
    if current_chars <= previous_chars || update.pending_unattributed_speech {
        return false;
    }

    let owner_established = update.target_speech_end_ms.is_some()
        || update.local_target_speech_end_ms.is_some();
    let provider_other_speaker_advanced = update
        .target_speech_end_ms
        .zip(update.stable_attributed_speech_end_ms)
        .is_some_and(|(target_ms, attributed_ms)| attributed_ms > target_ms);
    if !owner_established
        || provider_other_speaker_advanced
        || update_has_recent_strong_non_target(update)
    {
        return false;
    }

    let latest_audio_ms = update
        .audio_duration_ms
        .into_iter()
        .chain(update.provider_audio_duration_ms)
        .max();
    latest_audio_ms
        .zip(update.local_speech_end_ms)
        .is_some_and(|(audio_ms, speech_ms)| {
            audio_ms.saturating_sub(speech_ms) < EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
                && !local_speech_confidently_non_target(update, speech_ms)
        })
}

impl SettledTargetEndpointClock {
    fn update_allows_endpoint(
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        armed_from_visible_body_fallback: bool,
        latest_visible_body_ends_terminal: Option<bool>,
        manual_terminal_bridge_until: Option<Instant>,
        armed_at: Instant,
        now: Instant,
    ) -> bool {
        // A boundary frame can settle the preview before a continuing clause.
        // Protect only risky body shapes; a general uncertainty hold made
        // short-command auto-end vary between 1.2 and 3.7 seconds.
        let fresh_unclassified_local_speech = update_has_fresh_unclassified_local_speech(
            update,
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        );
        let pending_owner_tail = update.pending_unattributed_speech
            && !update_has_recent_strong_non_target(update)
            && has_unresolved_recent_owner_speech(
                update,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            );
        // For an enrolled tracker, low-level energy is not endpoint evidence.
        // A room can remain acoustically active after the owner stops, and an
        // “Uncertain” voiceprint window is not an owner match. Only a positive
        // owner watermark may keep the owner endpoint open; generic energy is
        // retained below solely for manual/no-profile sessions.
        let uncertain_tail_within_wall_ceiling = now.saturating_duration_since(armed_at)
            < Duration::from_millis(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS);
        let bounded_unclassified_local_speech = if update.local_speaker_tracking_enabled
            && update.local_target_speech_end_ms.is_some()
        {
            // Once the wake owner is established, a later local window can be
            // genuinely the same speaker while its embedding is temporarily
            // Uncertain (quiet syllable, overlap, or a short boundary split).
            // The old branch only accepted provider-uncovered owner speech;
            // when cloud and local watermarks were equal it treated that
            // Uncertain tail as silence and fired `inactive_1000ms` mid-word.
            // Keep the bounded identity-uncertainty hold here. Explicit
            // NonTarget evidence still wins through
            // `local_speech_confidently_non_target`, and the existing
            // two-second cap guarantees that room noise cannot hold forever.
            has_uncertain_owner_identity_tail(update)
                || has_unresolved_recent_owner_speech(
                    update,
                    EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
                )
        } else if update.local_speaker_tracking_enabled && update.target_speech_end_ms.is_some() {
            // Cloud attribution without a positive local owner edge is weaker:
            // retain only speech close to that owner boundary. Otherwise fresh
            // room energy could consume the whole uncertainty wall after every
            // settled command. The wall bound also expires a stale snapshot.
            uncertain_tail_within_wall_ceiling
                && has_unresolved_recent_owner_speech(
                    update,
                    EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
                )
        } else {
            fresh_unclassified_local_speech
        };
        // An enrolled owner may begin the next sentence immediately after a
        // provider-added terminal mark. Installed session 531 still had local
        // speech at the live audio edge (and no NonTarget evidence), yet the
        // terminal preview let the settled clock stop mid-utterance. For an
        // enrolled tracker, retain that bounded owner-compatible speech no
        // matter how the previous preview was punctuated. Manual sessions keep
        // the narrower open-clause/terminal-bridge policy. Real silence still
        // expires on the original endpoint timeout; this does not extend it.
        let enrolled_owner_established = update.local_speaker_tracking_enabled
            && (update.target_speech_end_ms.is_some()
                || update.local_target_speech_end_ms.is_some());
        let active_body_still_speaking = bounded_unclassified_local_speech
            && (enrolled_owner_established
                || latest_visible_body_ends_terminal == Some(false)
                || manual_terminal_bridge_until.is_some_and(|until| now < until));
        !active_body_still_speaking
            && (!pending_owner_tail
                || update_has_recent_strong_non_target(update))
            && (armed_from_visible_body_fallback
                || (update.speaker_info_present
                    && update.speaker_id.is_some()
                    && update.target_speech_end_ms.is_some()))
    }

    fn note_visible_body_boundary(
        &mut self,
        ends_terminal: bool,
        visible_chars: usize,
        now: Instant,
    ) {
        let signature = (ends_terminal, visible_chars);
        if self.last_visible_body_signature != Some(signature) {
            self.last_visible_body_signature = Some(signature);
            self.product_endpoint.note_text_revision();
        }
        if ends_terminal {
            let transition_visible_chars = visible_chars.max(self.open_body_peak_visible_chars);
            if transition_visible_chars >= MANUAL_TERMINAL_BRIDGE_MIN_VISIBLE_CHARS
                && self.latest_visible_body_ends_terminal == Some(false)
            {
                self.manual_terminal_bridge_until =
                    Some(now + Duration::from_millis(MANUAL_TERMINAL_BRIDGE_MAX_MS));
                self.manual_terminal_bridge_rearm_pending = true;
            }
            self.open_body_peak_visible_chars = 0;
        } else {
            self.manual_terminal_bridge_until = None;
            self.manual_terminal_bridge_rearm_pending = false;
            self.open_body_peak_visible_chars =
                if self.latest_visible_body_ends_terminal == Some(false) {
                    self.open_body_peak_visible_chars.max(visible_chars)
                } else {
                    visible_chars
                };
        }
        self.latest_visible_body_ends_terminal = Some(ends_terminal);
    }
}

fn target_speaker_end_timeout_ms_for_preview(preview: Option<&str>) -> u64 {
    if preview_has_dangling_continuation(preview) {
        EMBEDDED_DANGLING_CONTINUATION_END_TIMEOUT_MS
    } else if preview_ends_with_sentence_terminal(preview) {
        EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    } else if preview.map(str::trim).is_some_and(|text| !text.is_empty()) {
        // Open body without a terminal mark. Prefer hang over cutting a
        // clause that ASR has not punctuated yet.
        EMBEDDED_DANGLING_CONTINUATION_END_TIMEOUT_MS
    } else {
        EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    }
}

/// One immutable interpretation of the current product session for an
/// endpoint decision. Callback, watchdog and stop dispatch must consume the
/// same value; recomputing the no-body mode after the controller has already
/// committed a shorter deadline produces a truthful-looking but false stop
/// reason (live session 1896 logged `no_body_3000ms` after a ~900 ms commit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TargetSpeakerEndpointPolicy {
    body_started: bool,
    initial_body_wait_active: bool,
    /// Start of the accepted automatic wake's bounded body window.  This is
    /// also the no-body endpoint origin: callback and provider activity must
    /// not create a second timer after the window expires.
    automatic_no_body_started_at: Option<Instant>,
    endpoint_timeout_ms: u64,
    wall_clock_timeout_ms: u64,
    stop_reason: &'static str,
}

fn resolve_target_speaker_endpoint_policy(
    inner: &Arc<Inner>,
    session_id: SessionId,
    preview: Option<&str>,
    audio_duration_ms: Option<u64>,
) -> TargetSpeakerEndpointPolicy {
    let body_started = automatic_wake_body_started(inner, session_id);
    let automatic_wake = automatic_wake_session_active(inner, session_id);
    let mode_timeout_ms = target_speaker_end_timeout_ms_for_preview(preview);
    let endpoint_timeout_ms = if automatic_wake && !body_started {
        EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS.max(mode_timeout_ms)
    } else {
        mode_timeout_ms
    };
    let (initial_body_wait_active, automatic_no_body_started_at) = if automatic_wake
        && !body_started
    {
        automatic_wake_initial_body_wait_snapshot(inner, session_id, audio_duration_ms)
    } else {
        (false, None)
    };
    TargetSpeakerEndpointPolicy {
        body_started,
        initial_body_wait_active,
        automatic_no_body_started_at,
        endpoint_timeout_ms,
        wall_clock_timeout_ms: settled_target_wall_clock_timeout_ms(endpoint_timeout_ms),
        stop_reason: target_speaker_inactive_stop_reason(endpoint_timeout_ms),
    }
}

fn settled_target_wall_clock_timeout_ms(endpoint_timeout_ms: u64) -> u64 {
    endpoint_timeout_ms
        .saturating_sub(EMBEDDED_SETTLED_TARGET_SCHEDULING_ALLOWANCE_MS)
        .max(1)
}

fn preview_has_dangling_continuation(preview: Option<&str>) -> bool {
    let Some(text) = preview.map(str::trim).filter(|text| !text.is_empty()) else {
        return false;
    };
    let lexical_tail = text
        .trim_end_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '，' | ','
                        | '、'
                        | '。'
                        | '！'
                        | '？'
                        | '.'
                        | '!'
                        | '?'
                        | '…'
                        | ':'
                        | '：'
                        | ';'
                        | '；'
                )
        })
        .to_ascii_lowercase();
    const DANGLING_SUFFIXES: &[&str] = &[
        "然后",
        "但是",
        "不过",
        "而且",
        "并且",
        "另外",
        "还有",
        "接着",
        "最后",
        "所以",
        "因此",
        "因为",
        "如果",
        "假如",
        "虽然",
        "可是",
        "或者",
        "以及",
        "就是",
        "也就是",
        "比如",
        "例如",
        "首先",
        "其次",
        "至于",
        "那么",
        "那这样的话",
        "换句话说",
        "and",
        "but",
        "because",
        "so",
        "then",
        "finally",
        "also",
    ];
    DANGLING_SUFFIXES.iter().any(|suffix| {
        if suffix.is_ascii() {
            lexical_tail.split_whitespace().last() == Some(*suffix)
        } else {
            lexical_tail.ends_with(suffix)
        }
    })
}
