#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
struct LocalConfirmationPrefixRetryState {
    task_is_retry: bool,
    pending: bool,
    used: bool,
    retry_after_attempts: usize,
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
            && attempts == self.retry_after_attempts
            && new_audio_bytes >= LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_BYTES
    }

    fn note_started(&mut self, is_retry: bool) {
        self.task_is_retry = is_retry;
        if is_retry {
            self.pending = false;
            self.used = true;
        }
    }

    fn blocks_heavy_recovery(&self, local_confirmation_in_flight: bool) -> bool {
        self.pending || (self.task_is_retry && local_confirmation_in_flight)
    }
}

/// A slower/noisy physical "开始录音" can still be only "开始" at the first
/// 0.8 s rung. Retry after 140 ms of new audio only for that start-aligned
/// strong prefix, instead of waiting for the 1.8 s ladder rung. The
/// opportunistic retry is non-authoritative when absent, preserving the fixed
/// rejection budget and the established later fallback.
#[cfg(target_os = "windows")]
const LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_MS: usize = 140;
#[cfg(target_os = "windows")]
const LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_BYTES: usize =
    LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_MS * 32;
#[cfg(target_os = "windows")]
// `local_confirmation_attempts` is incremented when a ladder task starts, so
// the second rung is observed as 2 when its result is recorded. Using 1 here
// silently disabled the bounded fast follow-up for a strong 3/4-syllable
// prefix.
const LOCAL_CONFIRMATION_PREFIX_RETRY_AFTER_ATTEMPTS: usize = 2;

#[cfg(target_os = "windows")]
fn should_defer_exploratory_local_confirmation_for_fast_preroll(
    keyword_model_hit: bool,
    attempts: usize,
    pcm_bytes: usize,
    elapsed: Duration,
) -> bool {
    let pcm_ms = pcm_bytes / 32;
    let elapsed_ms = usize::try_from(elapsed.as_millis()).unwrap_or(usize::MAX);
    !keyword_model_hit
        && attempts == 0
        && pcm_ms >= LOCAL_CONFIRMATION_START_MS
        && pcm_ms < FAST_PREROLL_LOCAL_CONFIRM_DEFER_UNTIL_MS
        && elapsed_ms.saturating_mul(2) < pcm_ms
}

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
    task_origin_bytes: usize,
    embedded_session_id: u32,
) {
    // Both the initial and rolling windows get one ordinary exploratory pass;
    // the bounded follow-up unlocks after that first pass. It never consumes
    // the normal Absent budget and cannot accept without a complete relation.
    let retry_after_attempts = if task_origin_bytes == 0 {
        LOCAL_CONFIRMATION_PREFIX_RETRY_AFTER_ATTEMPTS
    } else {
        1
    };
    if candidate.local_confirmation_prefix_retry.task_is_retry
        || candidate.local_confirmation_attempts != retry_after_attempts
        || candidate.local_confirmation_prefix_retry.used
        || !local_confirmation_prefix_retry_eligible(confirmation, phrase_chars)
    {
        return;
    }
    candidate.local_confirmation_prefix_retry.pending = true;
    candidate.local_confirmation_prefix_retry.retry_after_attempts = retry_after_attempts;
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
    if !prefix_retry
        && overlap_degraded_owner_phrase_evidence(
            confirmation,
            phrase_chars,
            task_origin_bytes,
        )
    {
        candidate.local_owner_overlap_near_confirmations = candidate
            .local_owner_overlap_near_confirmations
            .saturating_add(1);
        log::info!(
            "[wake-phrase] overlap-degraded owner phrase evidence embedded_session_id={} count={}/{} prefix_units={} distance={}",
            embedded_session_id,
            candidate.local_owner_overlap_near_confirmations,
            OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED,
            confirmation.phonetic_prefix_units,
            confirmation.phonetic_best_distance
        );
    }
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
