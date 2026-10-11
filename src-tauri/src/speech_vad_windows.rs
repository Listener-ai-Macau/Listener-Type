//! Shared Sherpa VAD adapter. Each audio source owns a distinct runtime instance.
use std::ffi::{c_char, c_void, CString};
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use libloading::Library;
use sha2::{Digest, Sha256};

/// Synchronous, continuous quality for an already serialized audio worker.
/// Its timeline belongs only to the PCM supplied here, never the microphone's
/// original mixture. Unknown/active intervals remain possible speech.
pub(crate) struct SourceSpeechQuality {
    runtime: VadRuntime,
    pending: Vec<f32>,
    received: u64,
    analyzed: u64,
    active_start: Option<u64>,
    timeline: crate::speech_signal_quality::SpeechSignalTimeline,
}

impl SourceSpeechQuality {
    pub(crate) fn new() -> Result<Self, String> {
        let model = ensure_vad_model()?;
        let root = crate::persistence::speaker_verification_root().map_err(|e| e.to_string())?;
        Ok(Self { runtime: VadRuntime::load(&root, &model, VAD_MIN_SPEECH_SECONDS)?,
            pending: Vec::new(), received: 0, analyzed: 0, active_start: None,
            timeline: Default::default() })
    }

    pub(crate) fn accept(&mut self, pcm: &[u8]) -> Result<(), String> {
        if pcm.len() % 2 != 0 { return Err("odd extracted PCM byte count".into()); }
        self.received += (pcm.len() / 2) as u64;
        self.pending.extend(pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0],b[1]]) as f32 / 32768.0));
        while self.pending.len() >= VAD_WINDOW_SAMPLES {
            let frame = self.pending.drain(..VAD_WINDOW_SAMPLES).collect::<Vec<_>>();
            self.runtime.accept(&frame);
            self.analyzed += VAD_WINDOW_SAMPLES as u64;
            self.collect();
            if self.runtime.detected() {
                self.active_start.get_or_insert(self.analyzed.saturating_sub(8000));
            } else { self.active_start = None; }
            let stable = self.analyzed.saturating_sub(8000);
            self.timeline.advance_known_through(self.active_start.map_or(stable, |start| start.min(stable)));
        }
        Ok(())
    }

    fn collect(&mut self) {
        for (start, end) in self.runtime.drain_speech_ranges(0) {
            let end = end.min(self.received);
            if start < end { self.timeline.complete_speech(start, end); }
        }
    }

    pub(crate) fn finish(&mut self) {
        if !self.pending.is_empty() {
            self.pending.resize(VAD_WINDOW_SAMPLES, 0.0);
            self.runtime.accept(&self.pending);
            self.pending.clear();
        }
        self.runtime.flush();
        self.collect();
        self.timeline.advance_known_through(self.received);
    }

    pub(crate) fn is_valid(&self) -> bool { !self.timeline.is_invalid() }
    pub(crate) fn speech_upper_bound_ms(&self, start: u64, end: u64) -> Option<usize> {
        self.timeline.speech_upper_bound_ms(start, end)
    }
    pub(crate) fn resolved_speech_ms(&self, start: u64, end: u64) -> Option<usize> {
        self.timeline.resolved_speech_ms(start,end)
    }
    pub(crate) fn confirmed_speech_end_ms_in(&self, start: u64, end: u64) -> Option<u64> {
        self.timeline.confirmed_speech_end_ms_in(start,end)
    }
    pub(crate) fn completed_speech_ranges_ms(&self) -> Vec<(u64,u64)> {
        self.timeline.completed_speech_ranges_ms()
    }
}
const SAMPLE_RATE: u64 = 16_000;
pub(crate) const VAD_WINDOW_SAMPLES: usize = 512;
const VAD_BUFFER_SECONDS: f32 = 60.0;
const VAD_THRESHOLD: f32 = 0.5;
const VAD_MIN_SILENCE_SECONDS: f32 = 0.25;
pub(crate) const VAD_MIN_SPEECH_SECONDS: f32 = 0.25;
const VAD_MAX_SPEECH_SECONDS: f32 = 60.0;
const VAD_MODEL_NAME: &str = "silero_vad.onnx";
pub(crate) const VAD_MODEL_SHA256: &str =
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

pub(crate) struct VadRuntime {
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
    detector_flush: DetectorReset,
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
    pub(crate) fn load(
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
            let detector_flush: DetectorReset = *sherpa
                .get(b"SherpaOnnxVoiceActivityDetectorFlush\0")
                .map_err(|err| format!("missing Sherpa VAD flush API: {err}"))?;

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
                detector_flush,
            })
        }
    }

    pub(crate) fn reset(&self) {
        unsafe { (self.detector_reset)(self.vad) };
    }

    fn flush(&self) {
        unsafe { (self.detector_flush)(self.vad) };
    }

    pub(crate) fn accept(&self, samples: &[f32]) {
        debug_assert_eq!(samples.len(), VAD_WINDOW_SAMPLES);
        unsafe {
            (self.accept_waveform)(
                self.vad,
                samples.as_ptr(),
                i32::try_from(samples.len()).unwrap_or(VAD_WINDOW_SAMPLES as i32),
            );
        }
    }

    pub(crate) fn detected(&self) -> bool {
        unsafe { (self.detector_detected)(self.vad) != 0 }
    }

    pub(crate) fn drain_speech_end_ms(&self, base_samples: u64) -> Option<u64> {
        self.drain_speech_ranges(base_samples).into_iter()
            .map(|(_, end)| end.saturating_mul(1_000) / SAMPLE_RATE).max()
    }

    pub(crate) fn drain_speech_ranges(&self, base_samples: u64) -> Vec<(u64, u64)> {
        let mut ranges = Vec::new();
        unsafe {
            while (self.detector_empty)(self.vad) == 0 {
                let segment_ptr = (self.detector_front)(self.vad);
                if segment_ptr.is_null() {
                    break;
                }
                let segment = *segment_ptr;
                if segment.start >= 0 && segment.n > 0 {
                    let start = base_samples.saturating_add(segment.start as u64);
                    ranges.push((start, start.saturating_add(segment.n as u64)));
                }
                (self.destroy_speech_segment)(segment_ptr);
                (self.detector_pop)(self.vad);
            }
        }
        ranges
    }
}

pub(crate) fn ensure_vad_model() -> Result<PathBuf, String> {
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
