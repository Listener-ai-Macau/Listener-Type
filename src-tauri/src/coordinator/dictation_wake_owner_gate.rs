// The session classifier treats <=0.34 as explicit NonTarget. Preserve margin
// above that boundary and require two phrase-backed snapshots before recovering
// an enrolled owner below the normal 0.42 verification threshold.
const OWNER_AMBIGUOUS_MIN_SCORE: f32 = 0.38;
const OWNER_AMBIGUOUS_CONFIRMATIONS: u8 = 2;
// When local ASR has already confirmed the complete configured phrase, a
// near-threshold owner score is stronger evidence than either signal alone.
// Accept this narrow fusion on the first snapshot so a clean owner wake does
// not wait for the 1.8s retry. KWS-only candidates still require the normal
// voiceprint threshold or two consistent ambiguous snapshots.
const OWNER_FAST_LOCAL_PHRASE_MIN_SCORE: f32 = 0.40;
// Installed session 1980: local ASR confirmed the complete phrase at the start,
// but the same enrolled owner scored 0.264/0.228 in a noisy real capture. Keep
// an explicit low-score non-owner floor while allowing a complete local phrase
// to recover after the 1.8s verification window. KWS-only evidence cannot use it.
const OWNER_LOCAL_PHRASE_FALLBACK_MIN_SCORE: f32 = 0.20;
const OWNER_LOCAL_PHRASE_FALLBACK_MIN_PCM_MS: usize = 1_800;

fn should_prefetch_owner_verification(
    phrase_enrolled: bool,
    already_started: bool,
    pcm_bytes: usize,
) -> bool {
    phrase_enrolled && !already_started && pcm_bytes >= OWNER_VERIFICATION_START_BYTES
}

fn maybe_prefetch_owner_verification(
    candidate: &mut BufferedSpeakerCandidate,
    phrase: &str,
    embedded_session_id: u32,
) {
    let phrase_enrolled = crate::speaker_verification::is_enrolled_for_phrase(phrase);
    if !should_prefetch_owner_verification(
        phrase_enrolled,
        candidate.owner_verification_task.is_some(),
        candidate.pcm.len(),
    ) {
        return;
    }
    let pcm = candidate.pcm.clone();
    let voiceprint_phrase = phrase.to_string();
    candidate.owner_verification_task = Some(tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let result = crate::speaker_verification::verify(&pcm, &voiceprint_phrase);
        (result, started.elapsed().as_millis() as u64)
    }));
    log::info!(
        "[speaker-verification] owner check prefetched beside stage2 embedded_session_id={} pcm_ms={}",
        embedded_session_id,
        candidate.pcm.len() / 32
    );
}

fn next_owner_verification_retry_after(
    pcm_ms: usize,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> Option<usize> {
    match verification {
        Ok(result) if result.matched => None,
        // A short voiced span is a normal early-window condition, not a terminal
        // identity decision. Other runtime errors stay fail-closed, but receive
        // the same bounded 1.8s/2.4s retry ladder before final rejection.
        Ok(_) | Err(_) => next_owner_verification_retry_ms(pcm_ms),
    }
}

fn note_ambiguous_owner_evidence(
    confirmations: &mut u8,
    best_score: &mut f32,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    score: f32,
) -> bool {
    if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::None
        || score < OWNER_AMBIGUOUS_MIN_SCORE
    {
        *confirmations = 0;
        *best_score = 0.0;
        return false;
    }
    *confirmations = confirmations.saturating_add(1);
    *best_score = best_score.max(score);
    *confirmations >= OWNER_AMBIGUOUS_CONFIRMATIONS
}

fn local_phrase_can_recover_owner_gate(
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    pcm_ms: usize,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> bool {
    phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript
        && pcm_ms >= OWNER_LOCAL_PHRASE_FALLBACK_MIN_PCM_MS
        && verification
            .as_ref()
            .is_ok_and(|result| result.score >= OWNER_LOCAL_PHRASE_FALLBACK_MIN_SCORE)
}

fn local_phrase_can_fast_accept_owner_gate(
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> bool {
    phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript
        && verification
            .as_ref()
            .is_ok_and(|result| result.score >= OWNER_FAST_LOCAL_PHRASE_MIN_SCORE)
}

fn evaluate_owner_gate_evidence(
    confirmations: &mut u8,
    best_score: &mut f32,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    pcm_ms: usize,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> (bool, bool) {
    let fast_phrase_recovery =
        local_phrase_can_fast_accept_owner_gate(phrase_signal, verification);
    let voiceprint_match = fast_phrase_recovery || match verification {
        Ok(result) if result.matched => true,
        Ok(result) => note_ambiguous_owner_evidence(
            confirmations,
            best_score,
            phrase_signal,
            result.score,
        ),
        Err(_) => false,
    };
    let phrase_recovery = fast_phrase_recovery
        || local_phrase_can_recover_owner_gate(phrase_signal, pcm_ms, verification);
    (voiceprint_match || phrase_recovery, phrase_recovery)
}

fn evaluate_candidate_owner_gate(
    candidate: &mut BufferedSpeakerCandidate,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> (bool, bool) {
    evaluate_owner_gate_evidence(
        &mut candidate.owner_ambiguous_confirmations,
        &mut candidate.owner_best_ambiguous_score,
        phrase_signal,
        candidate.pcm.len() / 32,
        verification,
    )
}

// Run terminal verification before phrase recall. Otherwise repeated local-ASR
// Absents can skip the independent KWS cascade even when the completed buffer
// strongly matches the enrolled owner. Owner evidence only permits KWS to run;
// it never becomes phrase evidence by itself.
async fn terminal_owner_verification_for_recall(
    pcm: &[u8],
    phrase: &str,
) -> (
    bool,
    Result<crate::speaker_verification::VerificationResult, String>,
    u64,
) {
    let enrolled = crate::speaker_verification::is_enrolled_for_phrase(phrase);
    let pcm = pcm.to_vec();
    let phrase = phrase.to_string();
    let task = tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let result = crate::speaker_verification::verify(&pcm, &phrase);
        (result, started.elapsed().as_millis() as u64)
    })
    .await;
    let (verification, elapsed_ms) = match task {
        Ok(result) => result,
        Err(err) => (Err(format!("声纹验证任务失败: {err}")), 0),
    };
    let matched = enrolled && verification.as_ref().is_ok_and(|result| result.matched);
    (matched, verification, elapsed_ms)
}
