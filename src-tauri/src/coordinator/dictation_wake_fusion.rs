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
const OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED: u8 = 3;

#[cfg(target_os = "windows")]
fn overlap_degraded_owner_phrase_evidence(
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
    task_origin_bytes: usize,
) -> bool {
    let minimum_prefix_units = (phrase_chars / 2).max(2);
    !confirmation.matched
        && confirmation.phrase_relation == crate::wake_phrase::LocalPhraseRelation::Absent
        && task_origin_bytes == 0
        && confirmation.phonetic_best_window_start == 0
        && confirmation.phonetic_prefix_units >= minimum_prefix_units
        && confirmation.phonetic_best_distance
            <= phrase_chars.saturating_sub(minimum_prefix_units)
        && confirmation.transcript_chars >= phrase_chars.saturating_add(1)
}

#[cfg(target_os = "windows")]
fn enrolled_owner_repeated_overlap_near_can_accept(
    enrolled_owner_matched: bool,
    confirmations: u8,
) -> bool {
    enrolled_owner_matched && confirmations >= OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED
}
