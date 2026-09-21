include!("dictation_wake_fusion_core.rs");

#[cfg(target_os = "windows")]
fn enrolled_terminal_kws_can_accept_phonetic_near(
    enrolled_owner_matched: bool,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
) -> bool {
    // Three independent signals are required: KWS at the caller, enrolled
    // owner identity here, and a one-unit local-ASR phonetic near match.
    enrolled_owner_matched && phonetic_near_phrase_evidence(confirmation, phrase_chars)
}

#[cfg(target_os = "windows")]
fn enrolled_terminal_local_near_can_accept(
    enrolled_owner_matched: bool,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    task_origin_bytes: usize,
) -> bool {
    // Installed incident 501: the full terminal transcript began with 3/4
    // wake-phrase phonetic units at edit distance one and the enrolled owner
    // independently matched, but both KWS paths missed. Recover only this
    // terminal, start-aligned owner case. Requiring body text after the phrase
    // prevents an incomplete "开始录" from becoming a wake by itself.
    let minimum_prefix_units = phrase_chars.saturating_sub(1).max(1);
    enrolled_owner_matched
        && !confirmation.matched
        && confirmation.phrase_relation == crate::wake_phrase::LocalPhraseRelation::Absent
        && task_origin_bytes == 0
        && confirmation.phonetic_best_window_start == 0
        && confirmation.phonetic_prefix_units >= minimum_prefix_units
        && confirmation.phonetic_best_distance <= PHONETIC_NEAR_MAX_DISTANCE
        && confirmation.transcript_chars > phrase_chars
}

#[cfg(target_os = "windows")]
fn open_terminal_local_near_can_accept(
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    task_origin_bytes: usize,
) -> bool {
    // Session 2274297156 (2026-09-20 08:37): accented owner wake transcribed
    // "开su音…" (prefix 1, distance 2, head-aligned, body text) while the
    // drifted bank could not enroll-match, so every enrolled near tier was
    // dead. The kernel tier adds the voiceprint floor as the bystander guard.
    crate::speech_decision_kernel::open_near_phrase_wake_can_activate(
        crate::speech_decision_kernel::OpenNearPhraseWakeEvidence {
            voiceprint_score: verification.as_ref().ok().map(|result| result.score),
            task_origin_bytes,
            best_window_start: confirmation.phonetic_best_window_start,
            best_distance: confirmation.phonetic_best_distance,
            transcript_chars: confirmation.transcript_chars,
            phrase_chars,
            terminal_window: true,
        },
    )
}

#[cfg(target_os = "windows")]
fn enrolled_terminal_kws_phonetic_fusion_signal(
    enrolled_owner_matched: bool,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    embedded_session_id: u32,
) -> Option<denzic_voice_activation_v1_core::PhraseSignal> {
    if !enrolled_terminal_kws_can_accept_phonetic_near(
        enrolled_owner_matched,
        confirmation,
        phrase_chars,
    ) {
        return None;
    }
    log::info!(
        "[wake-phrase] terminal enrolled KWS fused with owner phonetic near-match embedded_session_id={} phonetic_best_distance={} transcript_chars={}",
        embedded_session_id,
        confirmation.phonetic_best_distance,
        confirmation.transcript_chars
    );
    Some(denzic_voice_activation_v1_core::PhraseSignal::KeywordModel)
}

#[cfg(target_os = "windows")]
const OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED: u8 =
    crate::speech_decision_kernel::OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED;

#[cfg(target_os = "windows")]
fn overlap_degraded_owner_phrase_evidence(
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    task_origin_bytes: usize,
) -> bool {
    crate::speech_decision_kernel::owner_overlap_degraded_phrase_evidence(
        crate::speech_decision_kernel::OwnerOverlapPhraseEvidence {
            phrase_matched: confirmation.matched,
            phrase_absent: confirmation.phrase_relation
                == crate::wake_phrase::LocalPhraseRelation::Absent,
            task_origin_bytes,
            best_window_start: confirmation.phonetic_best_window_start,
            best_distance: confirmation.phonetic_best_distance,
            transcript_chars: confirmation.transcript_chars,
            phrase_chars,
        },
    )
}

#[cfg(target_os = "windows")]
fn enrolled_owner_repeated_overlap_near_can_accept(
    enrolled_owner_matched: bool,
    confirmations: u8,
) -> bool {
    crate::speech_decision_kernel::repeated_owner_overlap_wake_can_activate(
        enrolled_owner_matched,
        confirmations,
    )
}

#[cfg(not(target_os = "windows"))]
fn enrolled_owner_repeated_overlap_near_can_accept(
    _enrolled_owner_matched: bool,
    _confirmations: u8,
) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn live_owner_near_wake_can_attempt(
    phrase_enrolled: bool,
    bounded_followup: bool,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    task_origin_bytes: usize,
    current_window_origin_bytes: usize,
) -> bool {
    crate::speech_decision_kernel::live_owner_near_wake_can_attempt(
        crate::speech_decision_kernel::LiveOwnerNearWakeEvidence {
            phrase_enrolled,
            bounded_followup,
            task_origin_bytes,
            current_window_origin_bytes,
            best_window_start: confirmation.phonetic_best_window_start,
            prefix_units: confirmation.phonetic_prefix_units,
            best_distance: confirmation.phonetic_best_distance,
            transcript_chars: confirmation.transcript_chars,
            phrase_chars,
        },
    )
}

#[cfg(target_os = "windows")]
fn live_owner_near_wake_end_seconds(
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    task_origin_bytes: usize,
) -> f32 {
    let relative_end = denzic_voice_activation_v1_core::refined_local_wake_end_seconds(
        denzic_voice_activation_v1_core::LocalConfirmationBoundaryInput {
            keyword_end_seconds: 0.0,
            recovered_keyword_end_seconds: confirmation.recovered_keyword_end_seconds,
            phrase_relation: crate::wake_phrase::LocalPhraseRelation::PhoneticStart,
            transcript_chars: confirmation.transcript_chars,
            phrase_chars,
            snapshot_pcm_ms: confirmation.snapshot_pcm_ms,
            end_pad_seconds: WAKE_END_PAD_SECONDS,
            local_endpoint_max_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
        },
    );
    task_origin_bytes as f32 / 32_000.0 + relative_end
}
