// Independent local human-speech activity for the embedded endpoint.
//
// The raw PCM energy gate remains useful for AGC and diagnostics, but it is
// not an endpoint authority: the installed device's idle floor can advance it
// for an entire quiet tail.  This adapter uses the official Sherpa-ONNX VAD
// C API on a dedicated worker.  Audio ingestion only copies a bounded chunk
// into a non-blocking queue; model work never runs on the BLE/coordinator path.

#[cfg(target_os = "windows")]
mod local_speech_vad {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::Arc;
    use std::thread;

    use parking_lot::Mutex as ParkingMutex;

    type LocalSpeechEvidence = crate::asr::volcengine::LocalSpeechEvidence;
    type LocalSpeechActivityState = crate::asr::volcengine::LocalSpeechActivityState;
    type SpeechSignalTimeline = crate::speech_signal_quality::SpeechSignalTimeline;

    const SAMPLE_RATE: u64 = 16_000;
    // The canonical detector keeps its 250 ms confirmation contract.  This
    // second same-model instance is only an onset barrier: one 512-sample
    // window is enough to publish PendingSpeech while the canonical detector
    // is still deciding whether the new low/short utterance is real.
    const VAD_PENDING_MIN_SPEECH_SECONDS: f32 = 0.032;
    // Preserve both onset confirmation and silence hysteresis before asserting
    // a stable gap. Active intervals remain wholly unresolved until completed.
    const QUALITY_LOOKBACK_SAMPLES: u64 = 8_000;
    // Capacity is audio, not transport packet count: a normal wake takeover
    // can enqueue its retained history before the runtime has initialized.
    // This is an analysis-memory bound, never a recording/STOP deadline.
    const VAD_QUEUE_MAX_PCM_BYTES: usize = 60 * 16_000 * 2;
    use crate::speech_vad_windows::{
        VadRuntime, ensure_vad_model, VAD_MIN_SPEECH_SECONDS,
        VAD_WINDOW_SAMPLES, VAD_MODEL_SHA256,
    };

    struct VadJob {
        start_samples: u64,
        end_samples: u64,
        pcm: Vec<u8>,
        generation: u64,
    }

    pub(super) struct LocalSpeechActivity {
        job_tx: Option<Sender<VadJob>>,
        queued_pcm_bytes: Arc<AtomicUsize>,
        analysis_generation: Arc<AtomicU64>,
        recovering_overrun: bool,
        evidence_sink: Arc<ParkingMutex<LocalSpeechEvidence>>,
        quality_sink: Arc<ParkingMutex<SpeechSignalTimeline>>,
        invalidated: Arc<AtomicBool>,
        next_input_start_samples: Option<u64>,
        permanently_unknown: bool,
    }

    impl LocalSpeechActivity {
        pub(super) fn new(evidence_sink: Arc<ParkingMutex<LocalSpeechEvidence>>) -> Self {
            Self::with_quality_timeline(evidence_sink, Arc::new(ParkingMutex::new(SpeechSignalTimeline::default())))
        }

        pub(super) fn with_quality_timeline(evidence_sink: Arc<ParkingMutex<LocalSpeechEvidence>>,
            quality_sink: Arc<ParkingMutex<SpeechSignalTimeline>>) -> Self {
            let (job_tx, job_rx) = mpsc::channel();
            let queued_pcm_bytes = Arc::new(AtomicUsize::new(0));
            let analysis_generation = Arc::new(AtomicU64::new(1));
            let invalidated = Arc::new(AtomicBool::new(false));
            let spawned = thread::Builder::new()
                .name("listener-local-vad".to_string())
                .spawn({
                    let evidence_sink = Arc::clone(&evidence_sink);
                    let invalidated = Arc::clone(&invalidated);
                    let quality_sink = Arc::clone(&quality_sink);
                    let queued_pcm_bytes = Arc::clone(&queued_pcm_bytes);
                    let analysis_generation = Arc::clone(&analysis_generation);
                    move || run_vad_worker(job_rx, evidence_sink, quality_sink, invalidated,
                        queued_pcm_bytes, analysis_generation)
                });
            if let Err(err) = spawned {
                log::warn!("[asr] local VAD worker could not start: {err}");
                invalidated.store(true, Ordering::Release);
                return Self {
                    job_tx: None,
                    queued_pcm_bytes, analysis_generation, recovering_overrun: false,
                    evidence_sink,
                    quality_sink,
                    invalidated,
                    next_input_start_samples: None,
                    permanently_unknown: true,
                };
            }
            Self {
                job_tx: Some(job_tx),
                queued_pcm_bytes, analysis_generation, recovering_overrun: false,
                evidence_sink,
                quality_sink,
                invalidated,
                next_input_start_samples: None,
                permanently_unknown: false,
            }
        }

        pub(super) fn disabled() -> Self {
            Self {
                job_tx: None,
                queued_pcm_bytes: Arc::new(AtomicUsize::new(0)),
                analysis_generation: Arc::new(AtomicU64::new(1)),
                recovering_overrun: false,
                evidence_sink: Arc::new(ParkingMutex::new(LocalSpeechEvidence::default())),
                quality_sink: Arc::new(ParkingMutex::new(SpeechSignalTimeline::default())),
                invalidated: Arc::new(AtomicBool::new(true)),
                next_input_start_samples: None,
                permanently_unknown: true,
            }
        }

        fn mark_unknown(&mut self, reason: &str) {
            if !self.permanently_unknown {
                log::warn!("[asr] local VAD became permanently unknown reason={reason}");
            }
            self.permanently_unknown = true;
            self.invalidated.store(true, Ordering::Release);
            self.quality_sink.lock().invalidate();
            let mut latest = self.evidence_sink.lock();
            latest.revision = latest.revision.saturating_add(1);
            latest.state = LocalSpeechActivityState::Unknown;
            latest.trailing_non_speech_ms = None;
        }

        pub(super) fn submit(
            &mut self,
            start_samples: u64,
            end_samples: u64,
            pcm: &[u8],
        ) -> LocalSpeechEvidence {
            if self
                .next_input_start_samples
                .is_some_and(|expected| expected != start_samples)
            {
                self.mark_unknown("non_contiguous_pcm");
            }
            self.next_input_start_samples = Some(end_samples);

            if pcm.is_empty() { return *self.evidence_sink.lock(); }
            if pcm.len() % 2 != 0 || start_samples.checked_add((pcm.len()/2) as u64) != Some(end_samples) {
                self.mark_unknown("invalid_pcm_range");
            }
            if !self.permanently_unknown {
                if self.queued_pcm_bytes.fetch_update(Ordering::AcqRel, Ordering::Acquire,
                    |queued| queued.checked_add(pcm.len()).filter(|total| *total <= VAD_QUEUE_MAX_PCM_BYTES)).is_err()
                {
                    if !self.recovering_overrun {
                        self.recovering_overrun = true;
                        let generation = self.analysis_generation.fetch_add(1, Ordering::AcqRel) + 1;
                        self.quality_sink.lock().invalidate();
                        let mut latest = self.evidence_sink.lock();
                        latest.revision = latest.revision.saturating_add(1);
                        latest.state = LocalSpeechActivityState::Unknown;
                        latest.trailing_non_speech_ms = None;
                        log::warn!("[asr] local VAD analysis overrun; retiring queued generation and awaiting fresh PCM generation={generation} source_end_samples={end_samples}");
                    }
                    return *self.evidence_sink.lock();
                }
                let job = VadJob {
                    start_samples,
                    end_samples,
                    pcm: pcm.to_vec(),
                    generation: self.analysis_generation.load(Ordering::Acquire),
                };
                let Some(job_tx) = self.job_tx.as_ref() else {
                    self.queued_pcm_bytes.fetch_sub(pcm.len(), Ordering::AcqRel);
                    self.mark_unknown("worker_channel_missing");
                    return *self.evidence_sink.lock();
                };
                if job_tx.send(job).is_err() {
                    self.queued_pcm_bytes.fetch_sub(pcm.len(), Ordering::AcqRel);
                    self.mark_unknown("worker_disconnected");
                } else {
                    self.recovering_overrun = false;
                }
            }
            *self.evidence_sink.lock()
        }
    }

    fn run_vad_worker(
        job_rx: Receiver<VadJob>,
        evidence_sink: Arc<ParkingMutex<LocalSpeechEvidence>>,
        quality_sink: Arc<ParkingMutex<SpeechSignalTimeline>>,
        invalidated: Arc<AtomicBool>,
        queued_pcm_bytes: Arc<AtomicUsize>,
        analysis_generation: Arc<AtomicU64>,
    ) {
        let model_path = match ensure_vad_model() {
            Ok(path) => path,
            Err(err) => {
                log::warn!("[asr] local VAD model unavailable: {err}");
                return;
            }
        };
        let runtime_root = match crate::persistence::speaker_verification_root() {
            Ok(root) => root,
            Err(err) => {
                log::warn!("[asr] local VAD runtime directory unavailable: {err}");
                return;
            }
        };
        let runtime = match VadRuntime::load(
            &runtime_root,
            &model_path,
            VAD_MIN_SPEECH_SECONDS,
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                log::warn!("[asr] local VAD runtime unavailable: {err}");
                return;
            }
        };
        let pending_runtime = match VadRuntime::load(
            &runtime_root,
            &model_path,
            VAD_PENDING_MIN_SPEECH_SECONDS,
        ) {
            Ok(runtime) => Some(runtime),
            Err(err) => {
                // The canonical detector remains useful if the optional
                // pending-onset instance cannot load.  Do not turn this
                // diagnostic safeguard into a session-wide VAD failure.
                log::warn!("[asr] pending-onset VAD unavailable; continuing with canonical VAD: {err}");
                None
            }
        };
        log::info!(
            "[asr] local VAD worker ready model={} sha256={} runtime_root={} pending_onset={}",
            model_path.display(),
            VAD_MODEL_SHA256,
            runtime_root.display(),
            pending_runtime.is_some()
        );

        let mut base_samples = 0_u64;
        let mut expected_start_ms = None;
        let mut processed_samples = 0_u64;
        let mut pending_samples = Vec::<f32>::new();
        let mut last_speech_end_ms: Option<u64> = None;
        let mut pending_speech_start_ms: Option<u64> = None;
        let mut last_reported_state: Option<LocalSpeechActivityState> = None;
        let mut activity_epoch = 0_u64;
        let mut invalid = false;
        let mut revision = 0_u64;
        let mut unresolved_speech_start: Option<u64> = None;

        let mut worker_generation = 0;
        for job in job_rx {
            queued_pcm_bytes.fetch_sub(job.pcm.len(), Ordering::AcqRel);
            if job.generation != analysis_generation.load(Ordering::Acquire) { continue; }
            if worker_generation != job.generation {
                if worker_generation != 0 {
                    runtime.reset();
                    if let Some(runtime) = pending_runtime.as_ref() { runtime.reset(); }
                    base_samples = job.start_samples;
                    expected_start_ms = None;
                    processed_samples = 0;
                    pending_samples.clear();
                    last_speech_end_ms = None;
                    pending_speech_start_ms = None;
                    last_reported_state = None;
                    unresolved_speech_start = None;
                    activity_epoch = activity_epoch.saturating_add(1);
                    invalid = false;
                }
                worker_generation = job.generation;
                // Previous coverage cannot cross the dropped interval. The
                // rebuilt ledger starts at fresh source coordinates only.
                {
                    let mut quality = quality_sink.lock();
                    if job.generation != analysis_generation.load(Ordering::Acquire) { continue; }
                    *quality = SpeechSignalTimeline::default();
                }
                revision = revision.max(evidence_sink.lock().revision);
            }
            if invalidated.load(Ordering::Acquire) {
                return;
            }
            let expected_end_ms = job
                .start_samples
                .saturating_add((job.pcm.len() / 2) as u64);
            if job.pcm.len() % 2 != 0 || expected_end_ms != job.end_samples {
                invalid = true;
            }
            if let Some(expected_start_samples) = expected_start_ms {
                if expected_start_samples != job.start_samples {
                    invalid = true;
                }
            } else {
                base_samples = job.start_samples;
                quality_sink.lock().begin_at(base_samples);
            }
            expected_start_ms = Some(job.end_samples);

            if !invalid {
                pending_samples.extend(
                    job.pcm
                        .chunks_exact(2)
                        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0),
                );
                while pending_samples.len() >= VAD_WINDOW_SAMPLES {
                    let frame = pending_samples.drain(..VAD_WINDOW_SAMPLES).collect::<Vec<_>>();
                    runtime.accept(&frame);
                    if let Some(pending_runtime) = pending_runtime.as_ref() {
                        pending_runtime.accept(&frame);
                    }
                    processed_samples = processed_samples.saturating_add(VAD_WINDOW_SAMPLES as u64);
                    let completed = runtime.drain_speech_ranges(base_samples);
                    if let Some(end_ms) = completed.iter().map(|(_, end)| end.saturating_mul(1_000) / SAMPLE_RATE).max() {
                        last_speech_end_ms = Some(last_speech_end_ms.map_or(end_ms, |old| old.max(end_ms)));
                    }
                    let analyzed = base_samples.saturating_add(processed_samples);
                    let possible_speech = runtime.detected()
                        || pending_runtime.as_ref().is_some_and(|runtime| runtime.detected());
                    if possible_speech {
                        unresolved_speech_start.get_or_insert(analyzed.saturating_sub(QUALITY_LOOKBACK_SAMPLES));
                    } else {
                        unresolved_speech_start = None;
                    }
                    let mut quality = quality_sink.lock();
                    // A producer overrun may retire this job while inference
                    // is in flight. Never republish its stale coverage after
                    // the producer has invalidated the quality ledger.
                    if job.generation != analysis_generation.load(Ordering::Acquire) { break; }
                    let was_invalid = quality.is_invalid();
                    for (start, end) in completed {
                        quality.complete_speech(start, end);
                    }
                    let stable_frontier = analyzed.saturating_sub(QUALITY_LOOKBACK_SAMPLES);
                    quality.advance_known_through(unresolved_speech_start.map_or(stable_frontier,
                        |start| start.min(stable_frontier)));
                    if !was_invalid && quality.is_invalid() {
                        log::warn!("[speaker-verification] continuous speech quality unavailable: late/inconsistent VAD interval analyzed_samples={analyzed}");
                    }
                    drop(quality);
                    // The fast instance is an onset-only guard.  It must not
                    // retain completed segments across a long session: its
                    // queue is diagnostic state, not an alternate endpoint
                    // source.  Drain and discard its segments after every
                    // shared frame while keeping its live `detected()` bit
                    // available for PendingSpeech below.
                    if let Some(pending_runtime) = pending_runtime.as_ref() {
                        let _ = pending_runtime.drain_speech_end_ms(base_samples);
                    }
                }
            } else {
                quality_sink.lock().invalidate();
                runtime.reset();
                if let Some(pending_runtime) = pending_runtime.as_ref() {
                    pending_runtime.reset();
                }
                pending_samples.clear();
            }

            revision = revision.saturating_add(1);
            let analyzed_through_samples = base_samples.saturating_add(processed_samples);
            let analyzed_through_ms = analyzed_through_samples.saturating_mul(1_000) / SAMPLE_RATE;
            let canonical_speech = !invalid && runtime.detected();
            let pending_speech = !invalid
                && !canonical_speech
                && pending_runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.detected());
            let state = if invalid {
                LocalSpeechActivityState::Unknown
            } else if canonical_speech {
                pending_speech_start_ms = None;
                LocalSpeechActivityState::Speech
            } else if pending_speech {
                if pending_speech_start_ms.is_none() {
                    pending_speech_start_ms = Some(
                        analyzed_through_samples
                            .saturating_sub(VAD_WINDOW_SAMPLES as u64)
                            .saturating_mul(1_000)
                            / SAMPLE_RATE,
                    );
                }
                LocalSpeechActivityState::PendingSpeech
            } else {
                pending_speech_start_ms = None;
                LocalSpeechActivityState::NonSpeech
            };
            let was_active = matches!(
                last_reported_state,
                Some(LocalSpeechActivityState::Speech | LocalSpeechActivityState::PendingSpeech)
            );
            let is_active = matches!(
                state,
                LocalSpeechActivityState::Speech | LocalSpeechActivityState::PendingSpeech
            );
            if is_active && !was_active {
                activity_epoch = activity_epoch.saturating_add(1);
            }
            if last_reported_state != Some(state) {
                log::info!(
                    "[asr] local VAD state transition state={state:?} activity_epoch={activity_epoch} analyzed_through_ms={analyzed_through_ms} pending_speech_start_ms={pending_speech_start_ms:?} canonical_detected={canonical_speech} pending_detected={pending_speech}"
                );
                last_reported_state = Some(state);
            }
            let evidence = if invalid {
                LocalSpeechEvidence {
                    analyzed_through_ms,
                    analyzed_through_samples,
                    last_detected_speech_end_ms: last_speech_end_ms,
                    pending_speech_start_ms: None,
                    trailing_non_speech_ms: None,
                    activity_epoch,
                    revision,
                    state: LocalSpeechActivityState::Unknown,
                }
            } else {
                LocalSpeechEvidence {
                    analyzed_through_ms,
                    analyzed_through_samples,
                    last_detected_speech_end_ms: last_speech_end_ms,
                    pending_speech_start_ms,
                    trailing_non_speech_ms: matches!(state, LocalSpeechActivityState::NonSpeech)
                        .then(|| {
                            last_speech_end_ms
                                .map(|end_ms| analyzed_through_ms.saturating_sub(end_ms))
                        })
                        .flatten(),
                    activity_epoch,
                    revision,
                    state,
                }
            };
            let mut latest = evidence_sink.lock();
            if invalidated.load(Ordering::Acquire) {
                return;
            }
            if job.generation != analysis_generation.load(Ordering::Acquire) { continue; }
            *latest = evidence;
        }
    }

    #[cfg(test)]
    mod backlog_recovery_tests {
        use super::*;
        use std::time::{Duration, Instant};

        #[test]
        #[ignore = "explicit recorded PCM overflow/recovery; no cloud/device/editor"]
        fn analysis_backlog_overrun_rebases_fresh_pcm_without_losing_the_session() {
            let root = std::path::PathBuf::from(std::env::var_os("LISTENER_VAD_BURST_REPLAY").unwrap());
            let bytes = std::fs::read(root.join("8ae1f493-2fba-42c3-9580-7dddfc1785d8.wav")).unwrap();
            let pcm = &bytes[44..];
            let (tx, rx) = mpsc::channel();
            let sink = Arc::new(ParkingMutex::new(LocalSpeechEvidence::default()));
            let quality = Arc::new(ParkingMutex::new(SpeechSignalTimeline::default()));
            let invalidated = Arc::new(AtomicBool::new(false));
            let queued = Arc::new(AtomicUsize::new(0));
            let generation = Arc::new(AtomicU64::new(1));
            // Hold the real consumer until the producer exceeds its audio
            // memory budget. This makes the overflow independent of CPU speed.
            let mut vad = LocalSpeechActivity { job_tx: Some(tx),
                evidence_sink: Arc::clone(&sink), quality_sink: Arc::clone(&quality),
                invalidated: Arc::clone(&invalidated), queued_pcm_bytes: Arc::clone(&queued),
                analysis_generation: Arc::clone(&generation), recovering_overrun: false,
                next_input_start_samples: None, permanently_unknown: false };
            let prefix_bytes = 61_000 * 32;
            for (i, chunk) in pcm[..prefix_bytes].chunks(3_200).enumerate() {
                vad.submit(i as u64 * 1_600, (i as u64 + 1) * 1_600, chunk);
            }
            assert!(vad.recovering_overrun);
            assert!(!vad.permanently_unknown);
            assert!(!invalidated.load(Ordering::Acquire));
            assert_eq!(sink.lock().state, LocalSpeechActivityState::Unknown);
            assert!(queued.load(Ordering::Acquire) <= VAD_QUEUE_MAX_PCM_BYTES);
            assert!(quality.lock().is_invalid());
            let worker = thread::spawn({
                let sink=Arc::clone(&sink);let quality=Arc::clone(&quality);
                let queued=Arc::clone(&queued);let generation=Arc::clone(&generation);
                move || run_vad_worker(rx,sink,quality,invalidated,queued,generation)
            });
            let deadline=Instant::now()+Duration::from_secs(5);
            while queued.load(Ordering::Acquire)!=0 {
                assert!(Instant::now()<deadline,"retired analysis history failed to drain");
                thread::sleep(Duration::from_millis(2));
            }
            for (i,chunk) in pcm[prefix_bytes..].chunks(3_200).enumerate() {
                let start=(prefix_bytes/2+i*1_600) as u64;
                let end=start+chunk.len() as u64/2;
                vad.submit(start,end,chunk);
                let deadline=Instant::now()+Duration::from_secs(2);
                while sink.lock().analyzed_through_samples+512<end {
                    assert!(Instant::now()<deadline,"fresh source failed to recover after overrun");
                    thread::sleep(Duration::from_millis(1));
                }
            }
            let final_evidence=*sink.lock();
            assert_eq!(final_evidence.state,LocalSpeechActivityState::NonSpeech);
            assert!(final_evidence.analyzed_through_samples+512>=pcm.len() as u64/2);
            assert_eq!(quality.lock().resolved_speech_ms(0,1000),None,
                "unprocessed retired history cannot become certified quiet/speech");
            assert!(!vad.permanently_unknown);
            drop(vad);worker.join().unwrap();
            std::fs::write(root.join("backlog-recovery-result.json"),serde_json::to_vec_pretty(&serde_json::json!({
                "retiredPrefixMs":61000,"queuedPcmBoundBytes":VAD_QUEUE_MAX_PCM_BYTES,
                "analysisGeneration":generation.load(Ordering::Acquire),
                "analyzedMs":final_evidence.analyzed_through_ms,"state":format!("{:?}",final_evidence.state),
                "unprocessedHistoryRemainsUnknown":true,"cloudCalls":0,"physicalStop":false
            })).unwrap()).unwrap();
        }
    }
}

#[cfg(target_os = "windows")]
use local_speech_vad::LocalSpeechActivity;

#[cfg(not(target_os = "windows"))]
struct LocalSpeechActivity;

#[cfg(not(target_os = "windows"))]
impl LocalSpeechActivity {
    fn new() -> Self {
        Self
    }

    fn disabled() -> Self {
        Self
    }

    fn submit(
        &mut self,
        _start_ms: u64,
        _end_ms: u64,
        _pcm: &[u8],
    ) -> crate::asr::volcengine::LocalSpeechEvidence {
        crate::asr::volcengine::LocalSpeechEvidence::default()
    }
}
