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
