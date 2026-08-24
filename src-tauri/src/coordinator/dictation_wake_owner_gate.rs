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

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn maybe_start_target_wake_extraction(
    candidate: &mut BufferedSpeakerCandidate,
    phrase: &str,
    embedded_session_id: u32,
) {
    if candidate.kind != BufferedSpeakerCandidateKind::Verification
        || candidate.target_wake_extraction_attempted
        || candidate.pcm.len() < TARGET_WAKE_EXTRACTION_START_BYTES
        || !target_wake_extraction_has_weak_phrase_evidence(
            candidate.local_kws_fusion_evidence,
            candidate.local_owner_overlap_near_confirmations,
        )
    {
        return;
    }
    candidate.target_wake_extraction_attempted = true;
    let embedding = match crate::speaker_verification::target_speaker_embedding_for_phrase(phrase) {
        Ok(Some(embedding)) => embedding,
        Ok(None) => return,
        Err(err) => {
            log::warn!(
                "[target-speaker] wake extraction skipped embedded_session_id={embedded_session_id}: {err}"
            );
            return;
        }
    };
    let pcm = candidate.pcm.clone();
    candidate.target_wake_extraction_task = Some(tauri::async_runtime::spawn_blocking(move || {
        crate::asr::target_speaker_extraction::extract_enrolled_owner_wake_candidate(
            &pcm, embedding,
        )
    }));
    log::info!(
        "[target-speaker] owner wake extraction started embedded_session_id={} source_pcm_ms={}",
        embedded_session_id,
        candidate.pcm.len() / 32
    );
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_wake_extraction_has_weak_phrase_evidence(
    local_kws_fusion_evidence: bool,
    local_owner_overlap_near_confirmations: u8,
) -> bool {
    // Separation is a heavyweight recovery path (1.1-3.0 s in installed live
    // traces). Starting it for every ambient candidate before either phrase
    // detector heard anything starved the ordinary single-speaker wake path and
    // even made the isolated local helper report busy. Require a cheap,
    // independent partial-phrase hint first. A full KWS/local match already has
    // the normal low-latency owner gate and does not need this recovery task.
    local_kws_fusion_evidence || local_owner_overlap_near_confirmations > 0
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
async fn evaluate_target_wake_extraction(
    inner: &Arc<Inner>,
    extracted: crate::asr::target_speaker_extraction::ExtractedWakeCandidate,
    phrase: &str,
    embedded_session_id: u32,
) -> Result<Option<ExtractedOwnerWakeEvidence>, String> {
    let confirmation_task = spawn_local_wake_confirmation(
        inner,
        extracted.pcm.clone(),
        phrase.to_string(),
        true,
    );
    let verify_pcm = extracted.pcm;
    let verify_phrase = phrase.to_string();
    let verification_task = tauri::async_runtime::spawn_blocking(move || {
        crate::speaker_verification::verify(&verify_pcm, &verify_phrase)
    });
    let (confirmation, verification) = tokio::join!(confirmation_task, verification_task);
    let confirmation = confirmation
        .map_err(|err| format!("target-speaker wake confirmation task failed: {err}"))??;
    let verification = verification
        .map_err(|err| format!("target-speaker wake verification task failed: {err}"))??;
    let phrase_matched = confirmation.matched
        && local_confirmation_can_activate(false, confirmation.phrase_relation);
    log::info!(
        "[target-speaker] owner wake extraction evaluated embedded_session_id={} source_pcm_ms={} extraction_ms={} local_ms={} phrase_matched={} phrase_relation={:?} owner_matched={} owner_score={:.6} residual_ratio={:.6}",
        embedded_session_id,
        extracted.source_pcm_ms,
        extracted.inference_ms,
        confirmation.inference_ms,
        phrase_matched,
        confirmation.phrase_relation,
        verification.matched,
        verification.score,
        extracted.residual_ratio
    );
    if !target_extracted_wake_can_activate(phrase_matched, verification.matched) {
        return Ok(None);
    }
    let wake_match = crate::wake_phrase::Match {
        start_seconds: None,
        end_seconds: refined_wake_end_seconds(
            0.0,
            &confirmation,
            phrase.chars().count(),
        ),
        matched_keyword: None,
    };
    Ok(Some(ExtractedOwnerWakeEvidence {
        wake_match,
        local_confirmation_ms: confirmation.inference_ms,
        extraction_ms: extracted.inference_ms,
        owner_score: verification.score,
        residual_ratio: extracted.residual_ratio,
    }))
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_extracted_wake_can_activate(phrase_matched: bool, owner_matched: bool) -> bool {
    phrase_matched && owner_matched
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
async fn terminal_target_wake_evidence(
    candidate: &mut BufferedSpeakerCandidate,
    inner: &Arc<Inner>,
    phrase: &str,
    embedded_session_id: u32,
) -> Option<ExtractedOwnerWakeEvidence> {
    let mut task = candidate.target_wake_extraction_task.take()?;
    let already_finished = task.inner().is_finished();
    let outcome = if already_finished {
        Some(task.await)
    } else {
        match tokio::time::timeout(
            Duration::from_millis(TARGET_WAKE_EXTRACTION_TERMINAL_WAIT_MS),
            &mut task,
        )
        .await
        {
            Ok(result) => Some(result),
            Err(_) => {
                task.abort();
                log::info!(
                    "[target-speaker] terminal owner wake extraction exceeded budget embedded_session_id={} budget_ms={}",
                    embedded_session_id,
                    TARGET_WAKE_EXTRACTION_TERMINAL_WAIT_MS
                );
                None
            }
        }
    };
    match outcome {
        Some(Ok(Ok(extracted))) => evaluate_target_wake_extraction(
            inner,
            extracted,
            phrase,
            embedded_session_id,
        )
        .await
        .unwrap_or_else(|err| {
            log::warn!(
                "[target-speaker] terminal owner wake evidence failed embedded_session_id={embedded_session_id}: {err}"
            );
            None
        }),
        Some(Ok(Err(err))) => {
            log::warn!(
                "[target-speaker] terminal owner wake extraction failed embedded_session_id={embedded_session_id}: {err}"
            );
            None
        }
        Some(Err(err)) => {
            log::warn!(
                "[target-speaker] terminal owner wake extraction task failed embedded_session_id={embedded_session_id}: {err}"
            );
            None
        }
        None => None,
    }
}
