use crate::asr::volcengine::{LocalSpeechActivityState, LocalSpeechEvidence};

const FOUNDRY_PREVIEW_FIRST_AUDIO_MS: u64 = 1_120;
const FOUNDRY_PREVIEW_STEP_MS: u64 = 720;
const FOUNDRY_PREVIEW_PREFIX_MS: u64 = 16_000;
const FOUNDRY_PREVIEW_WINDOW_MS: usize = 6_000;
const FOUNDRY_PREVIEW_TAIL_MS: u64 = 250;

struct FoundryMicrophoneConsumer {
    asr: Arc<FoundryLocalWhisperAsr>,
    capture: Mutex<(LocalSpeechActivity, u64)>,
    evidence: Arc<Mutex<LocalSpeechEvidence>>,
}

impl FoundryMicrophoneConsumer {
    fn new(asr: Arc<FoundryLocalWhisperAsr>) -> Self {
        let evidence = Arc::new(Mutex::new(LocalSpeechEvidence::default()));
        let activity = LocalSpeechActivity::with_native_endpoint_quality_timeline(
            Arc::clone(&evidence),
            Arc::new(Mutex::new(
                crate::speech_signal_quality::SpeechSignalTimeline::default(),
            )),
        );
        Self {
            asr,
            capture: Mutex::new((activity, 0)),
            evidence,
        }
    }
}

impl crate::recorder::AudioConsumer for FoundryMicrophoneConsumer {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return;
        }
        let mut capture = self.capture.lock();
        let start = capture.1;
        let end = start + (pcm.len() / 2) as u64;
        self.asr.consume_pcm_chunk(pcm);
        capture.0.submit(start, end, pcm);
        capture.1 = end;
    }
}

fn foundry_preview_end_ms(evidence: LocalSpeechEvidence, previous_end: u64) -> Option<u64> {
    let end = match evidence.state {
        LocalSpeechActivityState::Speech => evidence.analyzed_through_ms,
        LocalSpeechActivityState::NonSpeech => evidence
            .last_detected_speech_end_ms?
            .saturating_add(FOUNDRY_PREVIEW_TAIL_MS)
            .min(evidence.analyzed_through_ms),
        LocalSpeechActivityState::Unknown | LocalSpeechActivityState::PendingSpeech => return None,
    };
    (end >= FOUNDRY_PREVIEW_FIRST_AUDIO_MS
        && end.saturating_sub(previous_end) >= FOUNDRY_PREVIEW_STEP_MS)
        .then_some(end)
}

fn foundry_microphone_is_active(inner: &Arc<Inner>, owner: SessionId) -> bool {
    let state = inner.state.lock();
    state.session_id == owner
        && !state.cancelled
        && matches!(
            state.phase,
            SessionPhase::Starting | SessionPhase::Listening
        )
}

fn foundry_microphone_stop_due(
    observation: &LocalVadStopObservation,
    captured_ms: u64,
    now: Instant,
) -> bool {
    let quiet_ms = if observation.evidence.last_detected_speech_end_ms.is_some() {
        EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    } else {
        EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS
    };
    observation.confirmed_silence().is_some_and(|silence| {
        crate::personal_vad::PresenceEndpoint::default().due_on_confirmed_silence(
            silence,
            captured_ms,
            now,
            quiet_ms,
        )
    })
}

fn spawn_foundry_microphone_tasks(
    inner: &Arc<Inner>,
    owner: SessionId,
    capture: Arc<FoundryMicrophoneConsumer>,
) {
    let endpoint_inner = Arc::clone(inner);
    let endpoint_capture = Arc::clone(&capture);
    async_runtime::spawn(async move {
        let mut observation =
            LocalVadStopObservation::new(LocalSpeechEvidence::default(), Instant::now());
        while foundry_microphone_is_active(&endpoint_inner, owner) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let transition = {
                // Use the latest analyzed window and the PCM frontier at the
                // actual STOP claim. The audio callback cannot advance capture
                // while this decision is made, and unknown/pending VAD holds it.
                let captured = endpoint_capture.capture.lock();
                let evidence = endpoint_capture.evidence.lock();
                let now = Instant::now();
                observation.observe(*evidence, now);
                let no_speech =
                    evidence.activity_epoch == 0 && evidence.last_detected_speech_end_ms.is_none();
                let tail_analyzed = captured.1.saturating_sub(evidence.analyzed_through_samples)
                    <= 2 * crate::speech_vad_windows::VAD_WINDOW_SAMPLES as u64;
                if foundry_microphone_stop_due(&observation, captured.1 / 16, now)
                    && (!no_speech || tail_analyzed)
                {
                    let transition = claim_native_microphone_stop(&endpoint_inner, owner);
                    if no_speech && transition.is_some() {
                        // A fully quiet take can produce captions such as
                        // "(sad music)" in Whisper. Never deliver those.
                        endpoint_capture.asr.mark_confirmed_no_speech();
                    }
                    transition
                } else {
                    None
                }
            };
            if let Some(transition) = transition {
                log::info!("[foundry-asr] microphone quiet stop session_id={owner}");
                let _ = finish_end_session_after_stop_transition(&endpoint_inner, transition).await;
                break;
            }
        }
    });

    // Warm the final decoder independently. The already-warm Chinese preview
    // can start while Whisper is coming back from idle eviction.
    let runtime = Arc::clone(&inner.foundry_local_runtime);
    let final_model = capture.asr.model_alias().to_owned();
    async_runtime::spawn(async move {
        if let Err(error) = runtime.ensure_cached_loaded(&final_model).await {
            log::warn!("[foundry-asr] microphone warmup unavailable session_id={owner}: {error:#}");
        }
    });
    let preview_inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        let use_fast_preview = matches!(capture.asr.language_hint(), None | Some("zh"));
        let mut editor_checked = false;
        let mut previous_end = 0;
        let mut previous_text = String::new();
        while foundry_microphone_is_active(&preview_inner, owner) {
            tokio::time::sleep(Duration::from_millis(120)).await;
            if !foundry_microphone_is_active(&preview_inner, owner) {
                break;
            }
            let evidence = *capture.evidence.lock();
            let Some(end) = foundry_preview_end_ms(evidence, previous_end) else {
                continue;
            };
            previous_end = end;
            let full_prefix = end <= FOUNDRY_PREVIEW_PREFIX_MS;
            let fast = if use_fast_preview {
                let window = if full_prefix { end as usize } else { FOUNDRY_PREVIEW_WINDOW_MS };
                let pcm = capture.asr.preview_pcm(end, window);
                tokio::task::spawn_blocking(move || {
                    crate::asr::local::wake_helper::transcribe_if_ready(
                        &pcm,
                        Duration::from_secs(2),
                    )
                    .ok()
                    .filter(|draft| !draft.text.trim().is_empty())
                })
                .await
                .ok()
                .flatten()
            } else {
                None
            };
            let (text, engine, complete) = if let Some(draft) = fast {
                (draft.text, "paraformer", full_prefix)
            } else {
                match capture.asr.preview_recent_audio(end, Duration::from_secs(4)).await {
                    Ok(Some(text)) => (text, "whisper", end <= FOUNDRY_PREVIEW_WINDOW_MS as u64),
                    Ok(None) => continue,
                    Err(error) => {
                        log::warn!("[foundry-asr] microphone preview skipped session_id={owner}: {error:#}");
                        continue;
                    }
                }
            };
            let text = crate::asr::local::punctuation::restore_chinese_punctuation(text, true).await;
            if !foundry_microphone_is_active(&preview_inner, owner) {
                break;
            }
            if text.is_empty() || text == previous_text {
                continue;
            }
            if publish_dictation_capsule(
                &preview_inner,
                owner,
                DictationUiState::Recording,
                0.0,
                Some(text.clone()),
                None,
            ) {
                log::info!(
                    "[foundry-asr] microphone preview session_id={owner} audio_ms={end} engine={engine}"
                );
                if complete {
                    publish_foundry_streaming_composition(
                        &preview_inner, owner, &text, &mut editor_checked,
                    ).await;
                }
                previous_text = text;
            }
        }
    });
}

#[cfg(test)]
mod foundry_microphone_tests {
    use super::*;

    #[test]
    fn preview_advances_with_speech_and_does_not_extend_in_silence() {
        let speech = LocalSpeechEvidence {
            state: LocalSpeechActivityState::Speech,
            analyzed_through_ms: 1_600,
            ..LocalSpeechEvidence::default()
        };
        assert_eq!(foundry_preview_end_ms(speech, 0), Some(1_600));
        assert_eq!(
            foundry_preview_end_ms(LocalSpeechEvidence { analyzed_through_ms: 1_120, ..speech }, 0),
            Some(1_120)
        );
        assert_eq!(
            foundry_preview_end_ms(LocalSpeechEvidence { analyzed_through_ms: 1_119, ..speech }, 0),
            None
        );
        assert_eq!(foundry_preview_end_ms(speech, 1_600), None);
        let quiet = LocalSpeechEvidence {
            state: LocalSpeechActivityState::NonSpeech,
            analyzed_through_ms: 5_000,
            last_detected_speech_end_ms: Some(3_000),
            ..speech
        };
        assert_eq!(foundry_preview_end_ms(quiet, 1_600), Some(3_250));
        assert_eq!(foundry_preview_end_ms(quiet, 3_250), None);
        assert_eq!(
            foundry_preview_end_ms(
                LocalSpeechEvidence {
                    state: LocalSpeechActivityState::PendingSpeech,
                    ..quiet
                },
                0
            ),
            None
        );
    }

    #[test]
    fn local_end_requires_fresh_complete_quiet_and_holds_pending_speech() {
        let now = Instant::now();
        let mut observation = LocalVadStopObservation::new(
            LocalSpeechEvidence {
                state: LocalSpeechActivityState::Speech,
                analyzed_through_ms: 2_000,
                last_detected_speech_end_ms: Some(2_000),
                revision: 1,
                activity_epoch: 1,
                ..LocalSpeechEvidence::default()
            },
            now,
        );
        let quiet = LocalSpeechEvidence {
            state: LocalSpeechActivityState::NonSpeech,
            analyzed_through_ms: 5_000,
            revision: 2,
            ..observation.evidence
        };
        observation.observe(quiet, now);
        assert!(foundry_microphone_stop_due(&observation, 5_020, now));
        assert!(!foundry_microphone_stop_due(&observation, 5_251, now));
        observation.observe(
            LocalSpeechEvidence {
                state: LocalSpeechActivityState::PendingSpeech,
                analyzed_through_ms: 5_030,
                revision: 3,
                activity_epoch: 2,
                ..quiet
            },
            now,
        );
        assert!(!foundry_microphone_stop_due(&observation, 5_040, now));
        let initial = LocalVadStopObservation::new(
            LocalSpeechEvidence {
                state: LocalSpeechActivityState::NonSpeech,
                analyzed_through_ms: 8_000,
                revision: 1,
                ..LocalSpeechEvidence::default()
            },
            now,
        );
        assert!(foundry_microphone_stop_due(&initial, 8_000, now));
    }
}
