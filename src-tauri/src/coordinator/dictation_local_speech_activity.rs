// Independent local human-speech activity for the embedded endpoint.
//
// The raw PCM energy gate remains useful for AGC and diagnostics, but it is
// not an endpoint authority: the installed device's idle floor can advance it
// for an entire quiet tail.  This adapter uses the official Sherpa-ONNX VAD
// C API on a dedicated worker.  Audio ingestion only copies a bounded chunk
// into a non-blocking queue; model work never runs on the BLE/coordinator path.

#[cfg(target_os = "windows")]
mod local_speech_vad {
    use std::ffi::{c_char, c_void, CString};
    use std::fs;
    use std::io::{BufReader, Read};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
    use std::sync::Arc;
    use std::thread;

    use libloading::Library;
    use parking_lot::Mutex as ParkingMutex;
    use sha2::{Digest, Sha256};

    type LocalSpeechEvidence = crate::asr::volcengine::LocalSpeechEvidence;
    type LocalSpeechActivityState = crate::asr::volcengine::LocalSpeechActivityState;

    const SAMPLE_RATE: u64 = 16_000;
    const VAD_WINDOW_SAMPLES: usize = 512;
    // Keep the detector's rolling buffer and max-utterance guard above the
    // expected manual dictation span. Sherpa changes threshold/min-silence
    // behavior after max_speech_duration; a 10 s value would make a
    // continuous long sentence use a different endpoint policy.
    const VAD_BUFFER_SECONDS: f32 = 60.0;
    const VAD_THRESHOLD: f32 = 0.5;
    const VAD_MIN_SILENCE_SECONDS: f32 = 0.25;
    const VAD_MIN_SPEECH_SECONDS: f32 = 0.25;
    // The canonical detector keeps its 250 ms confirmation contract.  This
    // second same-model instance is only an onset barrier: one 512-sample
    // window is enough to publish PendingSpeech while the canonical detector
    // is still deciding whether the new low/short utterance is real.
    const VAD_PENDING_MIN_SPEECH_SECONDS: f32 = 0.032;
    const VAD_MAX_SPEECH_SECONDS: f32 = 60.0;
    const VAD_QUEUE_CAPACITY: usize = 64;
    const VAD_MODEL_NAME: &str = "silero_vad.onnx";
    const VAD_MODEL_SHA256: &str =
        "9E2449E1087496D8D4CABA907F23E0BD3F78D91FA552479BB9C23AC09CBB1FD6";

    #[repr(C)]
    struct SileroVadModelConfig {
        model: *const c_char,
        threshold: f32,
        min_silence_duration: f32,
        min_speech_duration: f32,
        window_size: i32,
        max_speech_duration: f32,
    }

    #[repr(C)]
    struct TenVadModelConfig {
        model: *const c_char,
        threshold: f32,
        min_silence_duration: f32,
        min_speech_duration: f32,
        window_size: i32,
        max_speech_duration: f32,
    }

    #[repr(C)]
    struct VadModelConfig {
        silero_vad: SileroVadModelConfig,
        sample_rate: i32,
        num_threads: i32,
        provider: *const c_char,
        debug: i32,
        ten_vad: TenVadModelConfig,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SpeechSegment {
        start: i32,
        samples: *mut f32,
        n: i32,
    }

    type VoiceActivityDetector = c_void;
    type CreateVad = unsafe extern "C" fn(*const VadModelConfig, f32) -> *const VoiceActivityDetector;
    type DestroyVad = unsafe extern "C" fn(*const VoiceActivityDetector);
    type AcceptWaveform =
        unsafe extern "C" fn(*const VoiceActivityDetector, *const f32, i32);
    type DetectorEmpty = unsafe extern "C" fn(*const VoiceActivityDetector) -> i32;
    type DetectorDetected = unsafe extern "C" fn(*const VoiceActivityDetector) -> i32;
    type DetectorPop = unsafe extern "C" fn(*const VoiceActivityDetector);
    type DetectorFront =
        unsafe extern "C" fn(*const VoiceActivityDetector) -> *const SpeechSegment;
    type DestroySpeechSegment = unsafe extern "C" fn(*const SpeechSegment);
    type DetectorReset = unsafe extern "C" fn(*const VoiceActivityDetector);

    struct VadRuntime {
        _onnx: Library,
        _providers: Library,
        _sherpa: Library,
        vad: *const VoiceActivityDetector,
        destroy_vad: DestroyVad,
        accept_waveform: AcceptWaveform,
        detector_empty: DetectorEmpty,
        detector_detected: DetectorDetected,
        detector_pop: DetectorPop,
        detector_front: DetectorFront,
        destroy_speech_segment: DestroySpeechSegment,
        detector_reset: DetectorReset,
    }

    unsafe impl Send for VadRuntime {}

    impl Drop for VadRuntime {
        fn drop(&mut self) {
            if !self.vad.is_null() {
                unsafe { (self.destroy_vad)(self.vad) };
            }
        }
    }

    impl VadRuntime {
        fn load(
            root: &Path,
            model_path: &Path,
            min_speech_duration: f32,
        ) -> Result<Self, String> {
            let model = CString::new(model_path.to_string_lossy().as_bytes())
                .map_err(|_| "VAD model path contains an invalid character".to_string())?;
            let provider = CString::new("cpu").expect("literal has no nul");
            let onnx_path = root.join("onnxruntime.dll");
            let providers_path = root.join("onnxruntime_providers_shared.dll");
            let sherpa_path = root.join("sherpa-onnx-c-api.dll");

            unsafe {
                let onnx = Library::new(&onnx_path)
                    .map_err(|err| format!("load VAD onnxruntime.dll failed: {err}"))?;
                let providers = Library::new(&providers_path)
                    .map_err(|err| format!("load VAD onnxruntime providers failed: {err}"))?;
                let sherpa = Library::new(&sherpa_path)
                    .map_err(|err| format!("load VAD sherpa-onnx C API failed: {err}"))?;
                let create_vad: CreateVad = *sherpa
                    .get(b"SherpaOnnxCreateVoiceActivityDetector\0")
                    .map_err(|err| format!("missing Sherpa VAD create API: {err}"))?;
                let destroy_vad: DestroyVad = *sherpa
                    .get(b"SherpaOnnxDestroyVoiceActivityDetector\0")
                    .map_err(|err| format!("missing Sherpa VAD destroy API: {err}"))?;
                let accept_waveform: AcceptWaveform = *sherpa
                    .get(b"SherpaOnnxVoiceActivityDetectorAcceptWaveform\0")
                    .map_err(|err| format!("missing Sherpa VAD waveform API: {err}"))?;
                let detector_empty: DetectorEmpty = *sherpa
                    .get(b"SherpaOnnxVoiceActivityDetectorEmpty\0")
                    .map_err(|err| format!("missing Sherpa VAD empty API: {err}"))?;
                let detector_detected: DetectorDetected = *sherpa
                    .get(b"SherpaOnnxVoiceActivityDetectorDetected\0")
                    .map_err(|err| format!("missing Sherpa VAD detected API: {err}"))?;
                let detector_pop: DetectorPop = *sherpa
                    .get(b"SherpaOnnxVoiceActivityDetectorPop\0")
                    .map_err(|err| format!("missing Sherpa VAD pop API: {err}"))?;
                let detector_front: DetectorFront = *sherpa
                    .get(b"SherpaOnnxVoiceActivityDetectorFront\0")
                    .map_err(|err| format!("missing Sherpa VAD front API: {err}"))?;
                let destroy_speech_segment: DestroySpeechSegment = *sherpa
                    .get(b"SherpaOnnxDestroySpeechSegment\0")
                    .map_err(|err| format!("missing Sherpa VAD segment destroy API: {err}"))?;
                let detector_reset: DetectorReset = *sherpa
                    .get(b"SherpaOnnxVoiceActivityDetectorReset\0")
                    .map_err(|err| format!("missing Sherpa VAD reset API: {err}"))?;

                let config = VadModelConfig {
                    silero_vad: SileroVadModelConfig {
                        model: model.as_ptr(),
                        threshold: VAD_THRESHOLD,
                        min_silence_duration: VAD_MIN_SILENCE_SECONDS,
                        min_speech_duration,
                        window_size: VAD_WINDOW_SAMPLES as i32,
                        max_speech_duration: VAD_MAX_SPEECH_SECONDS,
                    },
                    sample_rate: SAMPLE_RATE as i32,
                    num_threads: 1,
                    provider: provider.as_ptr(),
                    debug: 0,
                    ten_vad: TenVadModelConfig {
                        model: std::ptr::null(),
                        threshold: 0.0,
                        min_silence_duration: 0.0,
                        min_speech_duration: 0.0,
                        window_size: 0,
                        max_speech_duration: 0.0,
                    },
                };
                let vad = create_vad(&config, VAD_BUFFER_SECONDS);
                if vad.is_null() {
                    return Err("Sherpa VAD model initialization returned null".to_string());
                }
                Ok(Self {
                    _onnx: onnx,
                    _providers: providers,
                    _sherpa: sherpa,
                    vad,
                    destroy_vad,
                    accept_waveform,
                    detector_empty,
                    detector_detected,
                    detector_pop,
                    detector_front,
                    destroy_speech_segment,
                    detector_reset,
                })
            }
        }

        fn reset(&self) {
            unsafe { (self.detector_reset)(self.vad) };
        }

        fn accept(&self, samples: &[f32]) {
            debug_assert_eq!(samples.len(), VAD_WINDOW_SAMPLES);
            unsafe {
                (self.accept_waveform)(
                    self.vad,
                    samples.as_ptr(),
                    i32::try_from(samples.len()).unwrap_or(VAD_WINDOW_SAMPLES as i32),
                );
            }
        }

        fn detected(&self) -> bool {
            unsafe { (self.detector_detected)(self.vad) != 0 }
        }

        fn drain_speech_end_ms(&self, base_samples: u64) -> Option<u64> {
            let mut latest_end_ms: Option<u64> = None;
            unsafe {
                while (self.detector_empty)(self.vad) == 0 {
                    let segment_ptr = (self.detector_front)(self.vad);
                    if segment_ptr.is_null() {
                        break;
                    }
                    let segment = *segment_ptr;
                    if segment.start >= 0 && segment.n > 0 {
                        let end_samples = (segment.start as u64).saturating_add(segment.n as u64);
                        let end_ms = base_samples
                            .saturating_add(end_samples)
                            .saturating_mul(1_000)
                            / SAMPLE_RATE;
                        latest_end_ms = Some(latest_end_ms.map_or(end_ms, |old| old.max(end_ms)));
                    }
                    (self.destroy_speech_segment)(segment_ptr);
                    (self.detector_pop)(self.vad);
                }
            }
            latest_end_ms
        }
    }

    struct VadJob {
        start_samples: u64,
        end_samples: u64,
        pcm: Vec<u8>,
    }

    pub(super) struct LocalSpeechActivity {
        job_tx: Option<SyncSender<VadJob>>,
        evidence_sink: Arc<ParkingMutex<LocalSpeechEvidence>>,
        invalidated: Arc<AtomicBool>,
        next_input_start_samples: Option<u64>,
        permanently_unknown: bool,
    }

    impl LocalSpeechActivity {
        pub(super) fn new(evidence_sink: Arc<ParkingMutex<LocalSpeechEvidence>>) -> Self {
            let (job_tx, job_rx) = mpsc::sync_channel(VAD_QUEUE_CAPACITY);
            let invalidated = Arc::new(AtomicBool::new(false));
            let spawned = thread::Builder::new()
                .name("listener-local-vad".to_string())
                .spawn({
                    let evidence_sink = Arc::clone(&evidence_sink);
                    let invalidated = Arc::clone(&invalidated);
                    move || run_vad_worker(job_rx, evidence_sink, invalidated)
                });
            if let Err(err) = spawned {
                log::warn!("[asr] local VAD worker could not start: {err}");
                invalidated.store(true, Ordering::Release);
                return Self {
                    job_tx: None,
                    evidence_sink,
                    invalidated,
                    next_input_start_samples: None,
                    permanently_unknown: true,
                };
            }
            Self {
                job_tx: Some(job_tx),
                evidence_sink,
                invalidated,
                next_input_start_samples: None,
                permanently_unknown: false,
            }
        }

        pub(super) fn disabled() -> Self {
            Self {
                job_tx: None,
                evidence_sink: Arc::new(ParkingMutex::new(LocalSpeechEvidence::default())),
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

            if !self.permanently_unknown {
                let job = VadJob {
                    start_samples,
                    end_samples,
                    pcm: pcm.to_vec(),
                };
                let Some(job_tx) = self.job_tx.as_ref() else {
                    self.mark_unknown("worker_channel_missing");
                    return *self.evidence_sink.lock();
                };
                if let Err(err) = job_tx.try_send(job) {
                    match err {
                        TrySendError::Full(_) => self.mark_unknown("worker_queue_full"),
                        TrySendError::Disconnected(_) => self.mark_unknown("worker_disconnected"),
                    }
                }
            }
            *self.evidence_sink.lock()
        }
    }

    fn run_vad_worker(
        job_rx: Receiver<VadJob>,
        evidence_sink: Arc<ParkingMutex<LocalSpeechEvidence>>,
        invalidated: Arc<AtomicBool>,
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

        for job in job_rx {
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
                    if let Some(end_ms) = runtime.drain_speech_end_ms(base_samples) {
                        last_speech_end_ms = Some(last_speech_end_ms.map_or(end_ms, |old| old.max(end_ms)));
                    }
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
            *latest = evidence;
        }
    }

    fn ensure_vad_model() -> Result<PathBuf, String> {
        if let Some(path) = bundled_vad_model_path() {
            let actual = sha256_file(&path)?;
            if actual == VAD_MODEL_SHA256 {
                return Ok(path);
            }
            return Err(format!(
                "bundled local VAD model hash mismatch: expected={} actual={actual}",
                VAD_MODEL_SHA256
            ));
        }

        let root = crate::persistence::speaker_verification_root()
            .map_err(|err| format!("create local VAD model directory failed: {err}"))?;
        let model_path = root.join(VAD_MODEL_NAME);
        if model_path.exists() {
            let actual = sha256_file(&model_path)?;
            if actual == VAD_MODEL_SHA256 {
                return Ok(model_path);
            }
            return Err(format!(
                "local VAD model hash mismatch: expected={} actual={actual}",
                VAD_MODEL_SHA256
            ));
        }

        Err(format!(
            "local VAD model is not bundled and no verified cache exists: {}",
            model_path.display()
        ))
    }

    fn bundled_vad_model_path() -> Option<PathBuf> {
        #[cfg(debug_assertions)]
        {
            let development = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/models")
                .join(VAD_MODEL_NAME);
            if development.is_file() {
                return Some(development);
            }
        }

        let executable = std::env::current_exe().ok()?;
        let root = executable.parent()?;
        [
            root.join("resources/models").join(VAD_MODEL_NAME),
            root.join("models").join(VAD_MODEL_NAME),
            root.join(VAD_MODEL_NAME),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file())
    }

    fn sha256_file(path: &Path) -> Result<String, String> {
        let file = fs::File::open(path).map_err(|err| format!("open VAD model failed: {err}"))?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|err| format!("read VAD model failed: {err}"))?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(format!("{:X}", hasher.finalize()))
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
