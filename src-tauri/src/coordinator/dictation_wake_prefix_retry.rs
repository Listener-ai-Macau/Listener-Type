#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
struct LocalConfirmationPrefixRetryState {
    task_is_retry: bool,
    pending: bool,
    used: bool,
}

#[cfg(target_os = "windows")]
impl LocalConfirmationPrefixRetryState {
    fn reset_window(&mut self) {
        *self = Self::default();
    }

    fn should_start(&self, ladder_ready: bool, attempts: usize, new_audio_bytes: usize) -> bool {
        !ladder_ready
            && !self.used
            && self.pending
            && attempts == LOCAL_CONFIRMATION_PREFIX_RETRY_AFTER_ATTEMPTS
            && new_audio_bytes >= LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_BYTES
    }

    fn note_started(&mut self, is_retry: bool) {
        self.task_is_retry = is_retry;
        if is_retry {
            self.pending = false;
            self.used = true;
        }
    }
}

/// A slower physical "开始录音" can still be only "开始" at the 1.6 s rung.
/// Retry after 140 ms of new audio only for that start-aligned strong prefix.
/// The opportunistic retry is non-authoritative when absent, preserving the
/// fixed rejection budget and the established 2.0 s fallback.
#[cfg(target_os = "windows")]
const LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_MS: usize = 140;
#[cfg(target_os = "windows")]
const LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_BYTES: usize =
    LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_MS * 32;
#[cfg(target_os = "windows")]
const LOCAL_CONFIRMATION_PREFIX_RETRY_AFTER_ATTEMPTS: usize = 3;

#[cfg(target_os = "windows")]
fn local_confirmation_prefix_retry_eligible(
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
) -> bool {
    let minimum_prefix_units = (phrase_chars / 2).max(2);
    !confirmation.matched
        && confirmation.phrase_relation == crate::wake_phrase::LocalPhraseRelation::Absent
        && confirmation.phonetic_best_window_start == 0
        && confirmation.phonetic_prefix_units >= minimum_prefix_units
        && confirmation.phonetic_best_distance
            <= phrase_chars.saturating_sub(minimum_prefix_units)
}

#[cfg(target_os = "windows")]
fn note_local_confirmation_prefix(
    candidate: &mut BufferedSpeakerCandidate,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    embedded_session_id: u32,
) {
    if candidate.local_confirmation_prefix_retry.task_is_retry
        || candidate.local_confirmation_attempts
            != LOCAL_CONFIRMATION_PREFIX_RETRY_AFTER_ATTEMPTS
        || candidate.local_confirmation_prefix_retry.used
        || !local_confirmation_prefix_retry_eligible(confirmation, phrase_chars)
    {
        return;
    }
    candidate.local_confirmation_prefix_retry.pending = true;
    log::info!(
        "[wake-phrase] strong start prefix scheduled bounded follow-up embedded_session_id={} prefix_units={} distance={} after_new_audio_ms={}",
        embedded_session_id,
        confirmation.phonetic_prefix_units,
        confirmation.phonetic_best_distance,
        LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_MS
    );
}

#[cfg(target_os = "windows")]
struct LocalConfirmationAbsentOutcome {
    authoritative_full_absent: bool,
    local_absent_count: u8,
    kws_absent_count: u8,
    counted_kws_absent: bool,
    prefix_retry: bool,
}

#[cfg(target_os = "windows")]
fn record_local_confirmation_absent(
    candidate: &mut BufferedSpeakerCandidate,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    task_origin_bytes: usize,
    task_has_keyword_model_hit: bool,
    embedded_session_id: u32,
) -> LocalConfirmationAbsentOutcome {
    let prefix_retry = candidate.local_confirmation_prefix_retry.task_is_retry;
    let phonetic_near = phonetic_near_phrase_evidence(confirmation, phrase_chars);
    let authoritative_full_absent = completed_secondary_absent_is_authoritative(
        confirmation.phrase_relation,
        confirmation.transcript_chars,
        phrase_chars,
    );
    candidate.local_kws_fusion_evidence |= phonetic_near;
    if !prefix_retry {
        candidate.local_absent_count = candidate.local_absent_count.saturating_add(1);
        if let Some(coverage) = authoritative_local_absent_coverage(
            confirmation.phrase_relation,
            confirmation.transcript_chars,
            phrase_chars,
            task_origin_bytes,
            task_origin_bytes
                .saturating_add(confirmation.snapshot_pcm_ms.saturating_mul(32)),
        ) {
            candidate.local_absent_coverage = Some(coverage);
        }
    }
    let mut counted_kws_absent = false;
    if task_has_keyword_model_hit && !prefix_retry {
        if authoritative_full_absent {
            candidate.kws_local_absent_count = candidate.kws_local_absent_count.max(1);
            counted_kws_absent = true;
            log::info!(
                "[wake-phrase] stage2 full-length Absent authoritative embedded_session_id={} transcript_chars={} phrase_chars={} first_hit_pcm_ms={:?}",
                embedded_session_id,
                confirmation.transcript_chars,
                phrase_chars,
                candidate.kws_first_hit_pcm_ms
            );
        } else if kws_absent_counts_toward_reject(
            candidate.kws_first_hit_pcm_ms,
            confirmation.snapshot_pcm_ms,
        ) {
            candidate.kws_local_absent_count = candidate.kws_local_absent_count.saturating_add(1);
            counted_kws_absent = true;
        } else {
            log::info!(
                "[wake-phrase] stage2 early Absent held (phrase horizon) embedded_session_id={} snapshot_pcm_ms={} first_hit_pcm_ms={:?} min_post_hit_ms={}",
                embedded_session_id,
                confirmation.snapshot_pcm_ms,
                candidate.kws_first_hit_pcm_ms,
                KWS_ABSENT_COUNT_MIN_POST_HIT_MS
            );
        }
    }
    if prefix_retry {
        log::info!(
            "[wake-phrase] bounded prefix follow-up absent without consuming reject budget embedded_session_id={} transcript_chars={} prefix_units={}",
            embedded_session_id,
            confirmation.transcript_chars,
            confirmation.phonetic_prefix_units
        );
    }
    LocalConfirmationAbsentOutcome {
        authoritative_full_absent,
        local_absent_count: candidate.local_absent_count,
        kws_absent_count: candidate.kws_local_absent_count,
        counted_kws_absent,
        prefix_retry,
    }
}
