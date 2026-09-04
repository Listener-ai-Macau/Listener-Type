// Keep a margin above the explicit non-owner floor. A KWS hit is stronger phrase
// evidence than an exploratory ASR guess, so one owner-compatible snapshot can
// recover the wake even when the enrolled score is below the normal 0.42 match
// threshold. This addresses real owner wakes that scored ~0.33 in the field.
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
// KWS is an explicit wake-phrase model result. Require a sufficiently long
// candidate and a score above the session non-target floor; do not make KWS
// alone bypass the enrolled owner check.
const OWNER_KWS_FALLBACK_MIN_SCORE: f32 = 0.30;
const OWNER_KWS_FALLBACK_MIN_PCM_MS: usize = 1_800;

#[derive(Debug, Clone, Copy)]
struct OwnerGateEvaluation {
    access: crate::speech_decision_kernel::OwnerAccessEvidence,
    recovered_by_local_phrase: bool,
}

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
        candidate.owner_verification_attempted || candidate.owner_verification_task.is_some(),
        candidate.pcm.len(),
    ) {
        return;
    }
    let pcm = candidate.pcm.clone();
    let voiceprint_phrase = phrase.to_string();
    candidate.owner_verification_attempted = true;
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
    if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::None {
        *confirmations = 0;
        *best_score = 0.0;
        return false;
    }
    // Keep this as temporal evidence, not a single-window threshold. A noisy
    // mono capture commonly produces one weak embedding between two usable
    // windows; resetting the whole history on that dip made wake intermittent
    // even though the phrase detector remained positive. Strong windows add
    // confidence, while a weak phrase-backed window only decays one step.
    if score >= OWNER_AMBIGUOUS_MIN_SCORE {
        *confirmations = confirmations.saturating_add(1);
        *best_score = best_score.max(score);
    } else {
        *confirmations = confirmations.saturating_sub(1);
    }
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

fn kws_can_recover_owner_gate(
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    pcm_ms: usize,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> bool {
    phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel
        && pcm_ms >= OWNER_KWS_FALLBACK_MIN_PCM_MS
        && verification
            .as_ref()
            .is_ok_and(|result| result.score >= OWNER_KWS_FALLBACK_MIN_SCORE)
}

fn evaluate_owner_gate_evidence(
    confirmations: &mut u8,
    best_score: &mut f32,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    pcm_ms: usize,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> OwnerGateEvaluation {
    use crate::speaker_verification::VerificationPolicy;
    use crate::speech_decision_kernel::OwnerAccessEvidence;

    if let Ok(result) = verification {
        match result.policy {
            VerificationPolicy::OpenUnenrolled => {
                return OwnerGateEvaluation {
                    access: OwnerAccessEvidence::OpenUnenrolled,
                    recovered_by_local_phrase: false,
                };
            }
            VerificationPolicy::OpenInactiveProfile => {
                return OwnerGateEvaluation {
                    access: OwnerAccessEvidence::OpenInactiveProfile,
                    recovered_by_local_phrase: false,
                };
            }
            VerificationPolicy::Enrolled => {}
        }
    }
    let fast_phrase_recovery =
        local_phrase_can_fast_accept_owner_gate(phrase_signal, verification);
    let kws_recovery = kws_can_recover_owner_gate(phrase_signal, pcm_ms, verification);
    let voiceprint_match = fast_phrase_recovery || kws_recovery || match verification {
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
        || kws_recovery
        || local_phrase_can_recover_owner_gate(phrase_signal, pcm_ms, verification);
    OwnerGateEvaluation {
        access: match verification {
            Ok(_) if voiceprint_match || phrase_recovery => OwnerAccessEvidence::EnrolledMatch,
            Ok(_) => OwnerAccessEvidence::EnrolledNonMatch,
            Err(_) => OwnerAccessEvidence::Unavailable,
        },
        recovered_by_local_phrase: phrase_recovery,
    }
}

fn evaluate_candidate_owner_gate(
    candidate: &mut BufferedSpeakerCandidate,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> OwnerGateEvaluation {
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
    let matched = enrolled
        && verification
            .as_ref()
            .is_ok_and(|result| result.enrolled_owner_matched());
    (matched, verification, elapsed_ms)
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn maybe_start_target_wake_extraction(
    candidate: &mut BufferedSpeakerCandidate,
    phrase: &str,
    embedded_session_id: u32,
) {
    #[cfg(target_os = "windows")]
    if candidate.local_confirmation_prefix_retry.blocks_heavy_recovery(
        candidate.local_confirmation_task.is_some(),
    ) {
        return;
    }
    let weak_phrase_hint = target_wake_extraction_has_weak_phrase_evidence(
        candidate.kws_phrase_detected,
        candidate.local_kws_fusion_evidence,
        candidate.local_owner_overlap_near_confirmations,
    );
    if candidate.kind != BufferedSpeakerCandidateKind::Verification
        || candidate.target_wake_extraction_attempted
        || candidate.pcm.len() < TARGET_WAKE_EXTRACTION_START_BYTES
        || crate::speech_decision_kernel::decide_wake_recovery(
            crate::speech_decision_kernel::WakeRecoveryEvidence {
                weak_phrase_hint,
                terminal_owner_compatible: false,
            },
        ) != crate::speech_decision_kernel::WakeRecoveryDecision::AwaitSeparatedOwner
    {
        return;
    }
    start_target_wake_extraction(candidate, phrase, embedded_session_id, "weak_phrase_hint");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn start_target_wake_extraction(
    candidate: &mut BufferedSpeakerCandidate,
    phrase: &str,
    embedded_session_id: u32,
    reason: &'static str,
) {
    if candidate.target_wake_extraction_attempted
        || candidate.pcm.len() < TARGET_WAKE_EXTRACTION_START_BYTES
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
        "[target-speaker] owner wake extraction started embedded_session_id={} source_pcm_ms={} reason={}",
        embedded_session_id,
        candidate.pcm.len() / 32,
        reason
    );
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn maybe_start_terminal_owner_compatible_wake_extraction(
    candidate: &mut BufferedSpeakerCandidate,
    phrase: &str,
    embedded_session_id: u32,
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) {
    let terminal_owner_compatible = terminal_wake_source_owner_compatible(verification);
    let decision = crate::speech_decision_kernel::decide_wake_recovery(
        crate::speech_decision_kernel::WakeRecoveryEvidence {
            weak_phrase_hint: false,
            terminal_owner_compatible,
        },
    );
    if decision == crate::speech_decision_kernel::WakeRecoveryDecision::AwaitSeparatedOwner {
        start_target_wake_extraction(
            candidate,
            phrase,
            embedded_session_id,
            "terminal_owner_compatible",
        );
    }
}

#[cfg(target_os = "windows")]
fn terminal_wake_source_owner_compatible(
    verification: &Result<crate::speaker_verification::VerificationResult, String>,
) -> bool {
    verification.as_ref().is_ok_and(|result| {
        result.policy == crate::speaker_verification::VerificationPolicy::Enrolled
            && result.score >= OWNER_AMBIGUOUS_MIN_SCORE
    })
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_wake_extraction_has_weak_phrase_evidence(
    kws_phrase_detected: bool,
    local_kws_fusion_evidence: bool,
    local_owner_overlap_near_confirmations: u8,
) -> bool {
    // Separation is a heavyweight recovery path (1.1-3.0 s in installed live
    // traces). Starting it for every ambient candidate before either phrase
    // detector heard anything starved the ordinary single-speaker wake path and
    // even made the isolated local helper report busy. A host KWS hit is an
    // explicit phrase candidate and may start this path, but the separated
    // waveform still has to pass the enrolled owner verifier before activation.
    kws_phrase_detected
        || local_kws_fusion_evidence
        || local_owner_overlap_near_confirmations > 0
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
const fn target_wake_extraction_terminal_wait_ms(lazy_terminal_start: bool) -> u64 {
    if lazy_terminal_start {
        TARGET_WAKE_EXTRACTION_LAZY_TERMINAL_WAIT_MS
    } else {
        TARGET_WAKE_EXTRACTION_PREFETCHED_WAIT_MS
    }
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
async fn evaluate_target_wake_extraction(
    inner: &Arc<Inner>,
    extracted: crate::asr::target_speaker_extraction::ExtractedWakeCandidate,
    phrase: &str,
    embedded_session_id: u32,
    source_owner_compatible: bool,
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
        verification.enrolled_owner_matched(),
        verification.score,
        extracted.residual_ratio
    );
    let separated_owner_match = verification.enrolled_owner_matched();
    if !target_extracted_wake_can_activate(
        phrase_matched,
        source_owner_compatible,
        separated_owner_match,
    ) {
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
        owner_verified_by_extraction: separated_owner_match,
        residual_ratio: extracted.residual_ratio,
    }))
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_extracted_wake_can_activate(
    phrase_matched: bool,
    source_owner_compatible: bool,
    separated_owner_matched: bool,
) -> bool {
    crate::speech_decision_kernel::separated_wake_can_activate(
        crate::speech_decision_kernel::SeparatedWakeEvidence {
            exact_phrase: phrase_matched,
            source_owner_compatible,
            separated_owner_match: separated_owner_matched,
        },
    )
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
async fn terminal_target_wake_evidence(
    candidate: &mut BufferedSpeakerCandidate,
    inner: &Arc<Inner>,
    phrase: &str,
    embedded_session_id: u32,
    lazy_terminal_start: bool,
    source_owner_compatible: bool,
) -> Option<ExtractedOwnerWakeEvidence> {
    let mut task = candidate.target_wake_extraction_task.take()?;
    let already_finished = task.inner().is_finished();
    let wait_ms = target_wake_extraction_terminal_wait_ms(lazy_terminal_start);
    let outcome = if already_finished {
        Some(task.await)
    } else {
        match tokio::time::timeout(
            Duration::from_millis(wait_ms),
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
                    wait_ms
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
            source_owner_compatible,
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
