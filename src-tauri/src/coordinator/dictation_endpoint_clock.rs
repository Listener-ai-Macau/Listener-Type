/// Wall-clock companion to the provider audio clock.
///
/// The provider can publish a complete, speaker-attributed preview and then
/// stop advancing its covered-audio timestamp while local low-level noise
/// continues to trip the energy detector. In that state the ordinary endpoint
/// waits for the firmware fallback even though the user-visible text has been
/// settled for several seconds. Arm this clock only for a visible body with a
/// stable cloud target. Provisional speech cancels it, and a newer target
/// boundary rearms it, so it cannot race an actively growing owner utterance.
#[derive(Debug, Default)]
struct SettledTargetEndpointClock {
    generation: u64,
    armed_target_end_ms: Option<u64>,
    armed_at: Option<Instant>,
    armed_from_visible_body_fallback: bool,
    /// A provisional cloud tail normally cancels the owner endpoint. Preserve
    /// its original deadline out of band so a later sustained-local-other
    /// decision can restore that deadline instead of starting another wait.
    paused_armed_target_end_ms: Option<u64>,
    paused_armed_at: Option<Instant>,
    latest_update: Option<crate::asr::volcengine::TargetSpeakerUpdate>,
    /// Whether the latest visible preview ends at a sentence boundary. A
    /// provider row can stop advancing for several seconds in the middle of a
    /// long clause; the settled-text wall clock must not cut that open body
    /// while local speech is still reaching the microphone.
    latest_visible_body_ends_terminal: Option<bool>,
    /// Peak visible length across the current run of open previews. Speaker
    /// attribution can replace a long open provider preview with a much
    /// shorter terminal supplement; the pre-replacement length is the signal
    /// that distinguishes that risky shrink from an ordinary short command.
    open_body_peak_visible_chars: usize,
    /// A short terminal supplement can close an earlier open preview while
    /// the provider has skipped words between them. In manual/no-voiceprint
    /// recording, bridge that suspicious transition for a bounded interval;
    /// ordinary terminal-first short commands keep the normal 900 ms clock.
    manual_terminal_bridge_until: Option<Instant>,
    pending_was_seen: bool,
}

const RECENT_STRONG_NON_TARGET_WINDOW_MS: u64 = 900;
const MANUAL_TERMINAL_BRIDGE_MAX_MS: u64 = 3_000;
const MANUAL_TERMINAL_BRIDGE_MIN_VISIBLE_CHARS: usize = 20;

fn update_has_recent_strong_non_target(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> bool {
    let Some(non_target_end_ms) = update.local_non_target_speech_end_ms else {
        return false;
    };
    if update
        .local_target_speech_end_ms
        .is_some_and(|target_end_ms| target_end_ms > non_target_end_ms)
    {
        return false;
    }
    let latest_audio_ms = update
        .audio_duration_ms
        .into_iter()
        .chain(update.provider_audio_duration_ms)
        .max()
        .unwrap_or(non_target_end_ms);
    latest_audio_ms.saturating_sub(non_target_end_ms) <= RECENT_STRONG_NON_TARGET_WINDOW_MS
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

impl SettledTargetEndpointClock {
    fn update_allows_endpoint(
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        armed_from_visible_body_fallback: bool,
        latest_visible_body_ends_terminal: Option<bool>,
        manual_terminal_bridge_until: Option<Instant>,
        now: Instant,
    ) -> bool {
        // A provider utterance-boundary frame can settle the preview, then the
        // 900 ms timer can cut the next spoken clause even though PCM/local
        // speech are still advancing. Protect only the observable risky body
        // shapes below; applying the ordinary two-second uncertainty hold to
        // every terminal short command made auto-end randomly take 1.2–3.7 s.
        let fresh_unclassified_local_speech = update_has_fresh_unclassified_local_speech(
            update,
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        );
        // With an enrolled/local speaker tracker, raw energy after a stable
        // owner boundary is only a bounded uncertainty signal. Real session
        // 2284 kept reporting tiny room-noise windows as speech for 3.8 s after
        // the cloud owner sentence had settled. Reuse the existing two-second
        // owner-uncertainty ceiling here; manual/no-tracker sessions retain the
        // conservative fresh-speech protection used by the truncation guard.
        let bounded_unclassified_local_speech = if update.local_speaker_tracking_enabled
            && (update.target_speech_end_ms.is_some()
                || update.local_target_speech_end_ms.is_some())
        {
            has_unresolved_recent_local_speech(
                update,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            )
        } else {
            fresh_unclassified_local_speech
        };
        // An open visible clause always receives the fresh-speech protection.
        // A no-voiceprint terminal supplement receives the same protection
        // only when it just closed an earlier open clause, and only for the
        // bounded bridge interval. Confirmed NonTarget speech is excluded by
        // the helper and therefore still cannot hold the owner's endpoint.
        let active_body_still_speaking = bounded_unclassified_local_speech
            && (latest_visible_body_ends_terminal == Some(false)
                || (!update.local_speaker_tracking_enabled
                    && manual_terminal_bridge_until.is_some_and(|until| now < until)));
        !active_body_still_speaking
            && (!update.pending_unattributed_speech
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
        if ends_terminal {
            let transition_visible_chars =
                visible_chars.max(self.open_body_peak_visible_chars);
            if transition_visible_chars >= MANUAL_TERMINAL_BRIDGE_MIN_VISIBLE_CHARS
                && self.latest_visible_body_ends_terminal == Some(false)
            {
                self.manual_terminal_bridge_until =
                    Some(now + Duration::from_millis(MANUAL_TERMINAL_BRIDGE_MAX_MS));
            }
            self.open_body_peak_visible_chars = 0;
        } else {
            self.manual_terminal_bridge_until = None;
            self.open_body_peak_visible_chars =
                if self.latest_visible_body_ends_terminal == Some(false) {
                    self.open_body_peak_visible_chars.max(visible_chars)
                } else {
                    visible_chars
                };
        }
        self.latest_visible_body_ends_terminal = Some(ends_terminal);
    }

    fn pause_arm_for_provisional_tail(&mut self) {
        if let Some(armed_at) = self.armed_at {
            self.paused_armed_target_end_ms = self.armed_target_end_ms;
            self.paused_armed_at = Some(armed_at);
        }
        self.generation = self.generation.wrapping_add(1);
        self.armed_target_end_ms = None;
        self.armed_at = None;
        self.armed_from_visible_body_fallback = false;
    }

    fn clear_paused_arm(&mut self) {
        self.paused_armed_target_end_ms = None;
        self.paused_armed_at = None;
    }

    /// Returns a generation token when a new one-second timer must be started.
    fn observe(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
    ) -> Option<u64> {
        self.latest_update = Some(update.clone());
        let recent_strong_non_target = update_has_recent_strong_non_target(update);
        if update.pending_unattributed_speech && !recent_strong_non_target {
            self.pending_was_seen = true;
            if self.armed_at.is_some() {
                self.pause_arm_for_provisional_tail();
            }
            return None;
        }

        let stable_target_end_ms = (update.speaker_info_present && update.speaker_id.is_some())
            .then_some(update.target_speech_end_ms)
            .flatten();
        let stable_target_should_rearm = stable_target_end_ms.is_some()
            && (update.target_activity_advanced
                || self.pending_was_seen
                || self.armed_at.is_none()
                || self.armed_target_end_ms != stable_target_end_ms);
        // Some valid Volcengine previews arrive before diarization publishes a
        // speaker id. Once visible body text exists, arm a wall-clock fallback
        // instead of leaving the session entirely dependent on noisy firmware
        // VAD. A provisional tail still cancels the clock above.
        let unattributed_visible_body_should_arm =
            stable_target_end_ms.is_none() && self.armed_at.is_none();
        // Two consecutive very-low voiceprint windows identify current room
        // speech as a likely second speaker. Keep the already-running owner
        // timer in that state: provider diarization can temporarily fold both
        // people into one speaker id, and allowing either provider growth or
        // preview growth to re-arm here makes auto-end wait forever. This is
        // endpoint-only; it does not discard or rewrite recognized text.
        let should_rearm = body_started
            && (stable_target_should_rearm || unattributed_visible_body_should_arm)
            && (!recent_strong_non_target || self.armed_at.is_none());
        self.pending_was_seen = false;
        if !should_rearm {
            return None;
        }

        let restore_paused_owner_deadline = recent_strong_non_target
            && self.armed_at.is_none()
            && self.paused_armed_at.is_some();
        self.generation = self.generation.wrapping_add(1);
        if restore_paused_owner_deadline {
            self.armed_target_end_ms = self.paused_armed_target_end_ms;
            self.armed_at = self.paused_armed_at;
            // The paused boundary already came from visible owner text, and
            // the current frame now has sustained local other-speaker proof.
            // Do not require that provisional frame to repeat the cloud id.
            self.armed_from_visible_body_fallback = true;
        } else {
            self.armed_target_end_ms = stable_target_end_ms;
            self.armed_at = Some(now);
            self.armed_from_visible_body_fallback = stable_target_end_ms.is_none();
        }
        self.clear_paused_arm();
        Some(self.generation)
    }

    fn due_update(
        &self,
        generation: u64,
        now: Instant,
        timeout_ms: u64,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        if self.generation != generation {
            return None;
        }
        let armed_at = self.armed_at?;
        if now.saturating_duration_since(armed_at) < Duration::from_millis(timeout_ms) {
            return None;
        }
        self.latest_update
            .as_ref()
            .filter(|update| {
                Self::update_allows_endpoint(
                    update,
                    self.armed_from_visible_body_fallback,
                    self.latest_visible_body_ends_terminal,
                    self.manual_terminal_bridge_until,
                    now,
                )
            })
            .cloned()
    }

    fn arm_latest_for_visible_body(&mut self, now: Instant) -> Option<u64> {
        let update = self.latest_update.clone()?;
        let recent_strong_non_target = update_has_recent_strong_non_target(&update);
        if update.pending_unattributed_speech && !recent_strong_non_target {
            self.pending_was_seen = true;
            if self.armed_at.is_some() {
                self.pause_arm_for_provisional_tail();
            }
            return None;
        }
        if self.armed_at.is_some() && recent_strong_non_target {
            return None;
        }
        let stable_target_end_ms = (update.speaker_info_present && update.speaker_id.is_some())
            .then_some(update.target_speech_end_ms)
            .flatten();
        let restore_paused_owner_deadline =
            recent_strong_non_target && self.paused_armed_at.is_some();
        self.generation = self.generation.wrapping_add(1);
        if restore_paused_owner_deadline {
            self.armed_target_end_ms = self.paused_armed_target_end_ms;
            self.armed_at = self.paused_armed_at;
            self.armed_from_visible_body_fallback = true;
        } else {
            self.armed_target_end_ms = stable_target_end_ms;
            self.armed_at = Some(now);
            self.armed_from_visible_body_fallback = stable_target_end_ms.is_none();
        }
        self.clear_paused_arm();
        Some(self.generation)
    }

    fn is_due(&self, now: Instant, timeout_ms: u64) -> bool {
        self.armed_at.is_some_and(|armed_at| {
            now.saturating_duration_since(armed_at) >= Duration::from_millis(timeout_ms)
        }) && self.latest_update.as_ref().is_some_and(|update| {
            Self::update_allows_endpoint(
                update,
                self.armed_from_visible_body_fallback,
                self.latest_visible_body_ends_terminal,
                self.manual_terminal_bridge_until,
                now,
            )
        })
    }

    fn latest_due_update(
        &self,
        now: Instant,
        timeout_ms: u64,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        self.is_due(now, timeout_ms)
            .then(|| self.latest_update.clone())
            .flatten()
    }

    fn generation_is_current(&self, generation: u64) -> bool {
        self.generation == generation && self.armed_at.is_some()
    }
}

fn target_speaker_end_timeout_ms_for_preview(preview: Option<&str>) -> u64 {
    if preview_has_dangling_continuation(preview) {
        EMBEDDED_DANGLING_CONTINUATION_END_TIMEOUT_MS
    } else {
        EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
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

fn schedule_settled_target_endpoint_timer(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    generation: u64,
    endpoint_timeout_ms: u64,
) {
    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let endpoint_clock = Arc::clone(endpoint_clock);
    async_runtime::spawn(async move {
        let wall_clock_timeout_ms =
            settled_target_wall_clock_timeout_ms(endpoint_timeout_ms);
        let mut elapsed_ms = 0u64;
        // Firmware deliberately retains a one-second safety endpoint. While a
        // visible owner-safe preview ends in an explicit continuation word,
        // refresh that safety clock at most twice; generation checks cancel
        // the old schedule as soon as text grows or the session changes.
        while endpoint_timeout_ms > EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
            && elapsed_ms
                .saturating_add(EMBEDDED_DANGLING_FIRMWARE_KEEPALIVE_INTERVAL_MS)
                < wall_clock_timeout_ms
        {
            tokio::time::sleep(Duration::from_millis(
                EMBEDDED_DANGLING_FIRMWARE_KEEPALIVE_INTERVAL_MS,
            ))
            .await;
            elapsed_ms = elapsed_ms
                .saturating_add(EMBEDDED_DANGLING_FIRMWARE_KEEPALIVE_INTERVAL_MS);
            if !endpoint_clock.lock().generation_is_current(generation) {
                return;
            }
            note_embedded_asr_speech_activity(&inner, session_id);
        }
        tokio::time::sleep(Duration::from_millis(
            wall_clock_timeout_ms.saturating_sub(elapsed_ms),
        ))
        .await;
        let update = endpoint_clock.lock().due_update(
            generation,
            Instant::now(),
            wall_clock_timeout_ms,
        );
        let Some(update) = update else {
            return;
        };
        handle_target_speaker_update(&inner, session_id, &stop_dispatched, update, true);
    });
}

/// Session-scoped fallback for callback-order races.
///
/// Provider frames can synchronously emit a speaker update, a streaming
/// preview, and a two-pass supplement. Each can re-arm the generation-based
/// timer. A later local identity update may then invalidate the last queued
/// generation without producing another provider callback, leaving the
/// recording to the firmware's multi-second fallback. Polling the same guarded
/// clock keeps the one-second contract deterministic; `stop_dispatched` still
/// guarantees that this cannot issue a duplicate stop.
fn start_settled_target_endpoint_watchdog(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
) {
    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let endpoint_clock = Arc::clone(endpoint_clock);
    async_runtime::spawn(async move {
        const POLL_INTERVAL: Duration = Duration::from_millis(50);
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if stop_dispatched.load(Ordering::SeqCst) {
                return;
            }
            let session_active = {
                let state = inner.state.lock();
                state.session_id == session_id
                    && !state.cancelled
                    && matches!(
                        state.phase,
                        SessionPhase::Starting | SessionPhase::Listening
                    )
            };
            if !session_active {
                return;
            }
            let endpoint_timeout_ms = target_speaker_end_timeout_ms_for_preview(
                current_embedded_audio_partial_preview(&inner).as_deref(),
            );
            let update = endpoint_clock.lock().latest_due_update(
                Instant::now(),
                settled_target_wall_clock_timeout_ms(endpoint_timeout_ms),
            );
            if let Some(update) = update {
                handle_target_speaker_update(
                    &inner,
                    session_id,
                    &stop_dispatched,
                    update,
                    true,
                );
            }
        }
    });
}

fn arm_settled_target_endpoint_for_visible_body(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
) {
    let preview = current_embedded_audio_partial_preview(inner);
    let preview_ends_terminal = preview_ends_with_sentence_terminal(preview.as_deref());
    let preview_chars = preview.as_deref().map_or(0, |text| text.chars().count());
    let endpoint_timeout_ms = target_speaker_end_timeout_ms_for_preview(preview.as_deref());
    let now = Instant::now();
    let generation = {
        let mut clock = endpoint_clock.lock();
        clock.note_visible_body_boundary(preview_ends_terminal, preview_chars, now);
        clock.arm_latest_for_visible_body(now)
    };
    if let Some(generation) = generation {
        schedule_settled_target_endpoint_timer(
            inner,
            session_id,
            stop_dispatched,
            endpoint_clock,
            generation,
            endpoint_timeout_ms,
        );
    }
}
