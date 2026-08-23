use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceprintStatus {
    pub available: bool,
    pub runtime_ready: bool,
    pub model_ready: bool,
    pub enrolled: bool,
    pub enrolled_phrase: Option<String>,
    pub requires_reenrollment: bool,
    pub state: String,
    pub progress: u8,
    pub capture_seconds_remaining: Option<u8>,
    pub capture_step: Option<u8>,
    pub capture_step_count: u8,
    pub capture_elapsed_ms: u32,
    pub step_speech_ms: Vec<u16>,
    pub signal_level: u8,
    pub capture_feedback: Option<String>,
    pub score: Option<f32>,
    pub threshold: f32,
    pub error: Option<String>,
    pub model_name: &'static str,
    pub runtime_version: &'static str,
    pub local_only: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct VerificationResult {
    pub matched: bool,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct SessionSpeakerProfile {
    embeddings: Arc<Vec<Vec<f32>>>,
    adaptive: bool,
}

impl SessionSpeakerProfile {
    pub fn is_adaptive(&self) -> bool {
        self.adaptive
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionSpeakerClassification {
    Target { score: f32 },
    NonTarget { score: f32 },
    Uncertain { score: f32 },
}

#[derive(Debug, Clone)]
pub struct SessionSpeakerObservation {
    pub classification: SessionSpeakerClassification,
    pub real_speech_ms: usize,
    /// Transcript-only evidence for a very strong owner mismatch. Keeping this
    /// separate from `classification` lets a 600-999 ms clean fragment stop
    /// room speech from entering the final ledger without changing the
    /// established wake or endpoint policy, which still requires a full
    /// identity window.
    pub transcript_hard_non_target: bool,
    embedding: Vec<f32>,
}

#[derive(Debug, Default)]
pub struct SessionSpeakerAdaptationGate {
    consecutive_candidates: Vec<(u64, SessionSpeakerObservation)>,
    pending_candidates: std::collections::VecDeque<(u64, SessionSpeakerObservation)>,
    bootstrap_candidates: Vec<SessionSpeakerObservation>,
    bootstrap_pending: Vec<SessionSpeakerObservation>,
    bootstrap_consecutive_non_target: usize,
    bootstrap_disqualified: bool,
    bootstrap_complete: bool,
}

const SESSION_SPEAKER_MIN_NON_TARGET_MS: usize = 1_000;
// The ESP32 microphone's idle floor is normally around 60-150 RMS while real
// speech windows in installed sessions are in the thousands.  A flat idle
// window used to satisfy the relative VAD (all frames are equally "active"),
// produce a low cosine score, and become a confident NonTarget observation.
// Identity decisions need an absolute signal floor as well: below this level
// the sample remains advisory/Uncertain and cannot end or erase a session.
const SESSION_SPEAKER_MIN_IDENTITY_PEAK_RMS: f32 = 512.0;
const SESSION_SPEAKER_TRANSCRIPT_HARD_NON_TARGET_MIN_MS: usize = 600;
const SESSION_SPEAKER_TRANSCRIPT_HARD_NON_TARGET_MAX_SCORE: f32 = 0.10;
const SESSION_SPEAKER_ADAPT_MIN_SCORE: f32 = 0.50;
const SESSION_SPEAKER_ADAPT_CONFIRMATIONS: usize = 2;
const SESSION_SPEAKER_BOOTSTRAP_CONFIRMATIONS: usize = 3;
const SESSION_SPEAKER_ADAPT_PENDING_LIMIT: usize = 6;
const SESSION_SPEAKER_MAX_EMBEDDINGS: usize = 4;

impl SessionSpeakerClassification {
    pub fn score(self) -> f32 {
        match self {
            Self::Target { score } | Self::NonTarget { score } | Self::Uncertain { score } => score,
        }
    }
}

impl SessionSpeakerObservation {
    fn is_adaptation_candidate(&self) -> bool {
        self.real_speech_ms >= SESSION_SPEAKER_MIN_NON_TARGET_MS
            && matches!(
                self.classification,
                SessionSpeakerClassification::Target { score }
                    if score >= SESSION_SPEAKER_ADAPT_MIN_SCORE
            )
    }

    fn is_bootstrap_candidate(&self) -> bool {
        self.real_speech_ms >= SESSION_SPEAKER_MIN_NON_TARGET_MS
            && matches!(
                self.classification,
                SessionSpeakerClassification::Target { .. }
            )
    }
}

impl SessionSpeakerAdaptationGate {
    pub fn note(&mut self, audio_end_ms: u64, observation: SessionSpeakerObservation) {
        if !self.bootstrap_complete && !self.bootstrap_disqualified {
            match observation.classification {
                SessionSpeakerClassification::Target { .. }
                    if observation.is_bootstrap_candidate() =>
                {
                    self.bootstrap_consecutive_non_target = 0;
                    self.bootstrap_candidates.push(observation.clone());
                    if self.bootstrap_candidates.len() >= SESSION_SPEAKER_BOOTSTRAP_CONFIRMATIONS {
                        self.bootstrap_pending = std::mem::take(&mut self.bootstrap_candidates);
                    }
                }
                SessionSpeakerClassification::NonTarget { .. } => {
                    self.bootstrap_candidates.clear();
                    self.bootstrap_consecutive_non_target =
                        self.bootstrap_consecutive_non_target.saturating_add(1);
                    if self.bootstrap_consecutive_non_target >= 2 {
                        self.bootstrap_disqualified = true;
                    }
                }
                SessionSpeakerClassification::Target { .. } => {
                    // 未满一整窗(<1000ms)的 Target：不算一格，但也不清空连击——
                    // 它仍是本人证据。LST-REC-025 只让 Uncertain/NonTarget 打断
                    // bootstrap 连击；2026-08-07 07:23 session 里 800/600ms 的
                    // Target 窗把连击清零，bootstrap 从未成立，后段声纹漂移冻结
                    // 在 8 字（「你看一」事故）。
                }
                _ => {
                    self.bootstrap_candidates.clear();
                    self.bootstrap_consecutive_non_target = 0;
                }
            }
        }
        if !observation.is_adaptation_candidate() {
            self.consecutive_candidates.clear();
            return;
        }
        self.consecutive_candidates
            .push((audio_end_ms, observation));
        if self.consecutive_candidates.len() < SESSION_SPEAKER_ADAPT_CONFIRMATIONS {
            return;
        }
        self.pending_candidates
            .extend(self.consecutive_candidates.drain(..));
        while self.pending_candidates.len() > SESSION_SPEAKER_ADAPT_PENDING_LIMIT {
            self.pending_candidates.pop_front();
        }
    }

    pub fn promote_covered(
        &mut self,
        profile: &mut SessionSpeakerProfile,
        stable_target_end_ms: Option<u64>,
    ) -> usize {
        if !self.bootstrap_complete && !self.bootstrap_pending.is_empty() {
            let added = bootstrap_session_speaker_profile(profile, &self.bootstrap_pending);
            self.bootstrap_pending.clear();
            if added > 0 {
                self.bootstrap_complete = true;
                return added;
            }
        }
        let Some(stable_target_end_ms) = stable_target_end_ms else {
            return 0;
        };
        let mut covered = Vec::new();
        while self
            .pending_candidates
            .front()
            .is_some_and(|(audio_end_ms, _)| *audio_end_ms <= stable_target_end_ms)
        {
            if let Some((_, observation)) = self.pending_candidates.pop_front() {
                covered.push(observation);
            }
        }
        adapt_session_speaker_profile(profile, &covered)
    }
}

fn bootstrap_session_speaker_profile(
    profile: &mut SessionSpeakerProfile,
    observations: &[SessionSpeakerObservation],
) -> usize {
    if !profile.adaptive || profile.embeddings.len() >= SESSION_SPEAKER_MAX_EMBEDDINGS {
        return 0;
    }
    let expected_dimension = profile.embeddings.first().map_or(0, Vec::len);
    let available = SESSION_SPEAKER_MAX_EMBEDDINGS.saturating_sub(profile.embeddings.len());
    let additions = observations
        .iter()
        .filter(|observation| {
            observation.is_bootstrap_candidate()
                && observation.embedding.len() == expected_dimension
        })
        .take(available)
        .map(|observation| observation.embedding.clone())
        .collect::<Vec<_>>();
    let count = additions.len();
    Arc::make_mut(&mut profile.embeddings).extend(additions);
    count
}

fn adapt_session_speaker_profile(
    profile: &mut SessionSpeakerProfile,
    observations: &[SessionSpeakerObservation],
) -> usize {
    if !profile.adaptive || profile.embeddings.len() >= SESSION_SPEAKER_MAX_EMBEDDINGS {
        return 0;
    }
    let expected_dimension = profile.embeddings.first().map_or(0, Vec::len);
    let available = SESSION_SPEAKER_MAX_EMBEDDINGS.saturating_sub(profile.embeddings.len());
    let additions = observations
        .iter()
        .filter(|observation| {
            observation.is_adaptation_candidate()
                && observation.embedding.len() == expected_dimension
        })
        .take(available)
        .map(|observation| observation.embedding.clone())
        .collect::<Vec<_>>();
    let count = additions.len();
    Arc::make_mut(&mut profile.embeddings).extend(additions);
    count
}

/// Add the already-verified wake voice as a session-only reference for an
/// enrolled owner. Persistent enrollment may have been recorded at another
/// distance, volume, or on another day; comparing natural dictation only with
/// those old wake-phrase samples left otherwise valid owner speech in the
/// Uncertain band for an entire session. The fresh exemplar is safe to use
/// here because the automatic wake gate has already matched the persisted
/// owner template before `session_profile_from_wake` is called.
///
/// This deliberately does not change `adaptive`: enrolled profiles still
/// cannot learn from later body windows, and this exemplar is never persisted.
fn append_verified_wake_session_exemplar(
    embeddings: &mut Vec<Vec<f32>>,
    verified_wake_embedding: Vec<f32>,
) -> bool {
    if embeddings.is_empty()
        || embeddings.len() >= SESSION_SPEAKER_MAX_EMBEDDINGS
        || verified_wake_embedding.len() != embeddings[0].len()
        || verified_wake_embedding.is_empty()
    {
        return false;
    }
    embeddings.push(verified_wake_embedding);
    true
}

// 会话分段分类阈值。2026-08-09 的第二个人得分可达 0.426–0.58；而
// 2026-08-14 installed session 1947 中，已经通过主人唤醒校验的同一说话人
// 正文连续得到 0.307–0.337。中间分数只能作为 Uncertain，不能冻结并截断
// 正文。仅 <=0.20 的强差异作为 NonTarget；>=0.55 才是确信 Target。
const SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE: f32 = 0.55;
const SESSION_SPEAKER_NON_TARGET_MAX_SCORE: f32 = 0.20;

fn session_speaker_classification_for_score(score: f32) -> SessionSpeakerClassification {
    if score >= SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE {
        SessionSpeakerClassification::Target { score }
    } else if score <= SESSION_SPEAKER_NON_TARGET_MAX_SCORE {
        SessionSpeakerClassification::NonTarget { score }
    } else {
        SessionSpeakerClassification::Uncertain { score }
    }
}

fn session_speaker_classification_for_evidence(
    score: f32,
    real_speech_ms: usize,
) -> SessionSpeakerClassification {
    let classification = session_speaker_classification_for_score(score);
    if real_speech_ms < SESSION_SPEAKER_MIN_NON_TARGET_MS
        && matches!(
            classification,
            SessionSpeakerClassification::NonTarget { .. }
        )
    {
        SessionSpeakerClassification::Uncertain { score }
    } else {
        classification
    }
}

fn session_speaker_classification_for_signal(
    score: f32,
    real_speech_ms: usize,
    peak_rms: f32,
) -> SessionSpeakerClassification {
    let classification = session_speaker_classification_for_evidence(score, real_speech_ms);
    if peak_rms < SESSION_SPEAKER_MIN_IDENTITY_PEAK_RMS {
        SessionSpeakerClassification::Uncertain { score }
    } else {
        classification
    }
}

fn session_speaker_transcript_hard_non_target(
    score: f32,
    real_speech_ms: usize,
    peak_rms: f32,
) -> bool {
    score <= SESSION_SPEAKER_TRANSCRIPT_HARD_NON_TARGET_MAX_SCORE
        && real_speech_ms >= SESSION_SPEAKER_TRANSCRIPT_HARD_NON_TARGET_MIN_MS
        && peak_rms >= SESSION_SPEAKER_MIN_IDENTITY_PEAK_RMS
}

#[cfg(target_os = "windows")]
mod platform {
    #[cfg(test)]
    use super::session_speaker_classification_for_evidence;
    use super::{
        session_speaker_classification_for_signal, session_speaker_transcript_hard_non_target,
        SessionSpeakerClassification, SessionSpeakerObservation, SessionSpeakerProfile,
        VerificationResult, VoiceprintStatus,
    };
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    #[cfg(test)]
    use denzic_speaker_verification_v1_core::DEFAULT_SCORE_MILLI;
    use denzic_speaker_verification_v1_core::{
        Action, CandidateOrigin, Input, Machine, Verdict, DEFAULT_MAX_CANDIDATE_MS,
    };
    use libloading::Library;
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::borrow::Cow;
    use std::ffi::{c_char, c_void, CString};
    use std::fs;
    use std::io::{BufReader, Read};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    const RUNTIME_VERSION: &str = "1.13.1";
    const MODEL_NAME: &str = "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";
    const MODEL_SHA256: &str = "F682B514C05D947EE3FA91CD6EC6C5C7543479A128373FA29B1FAEDCCD21FD11";
    const TARGET_SPEAKER_MODEL_SHA256: &str =
        "9CEC30564E3A87746BDD44108779EDDF5323CD9ED4BD0966B64883A6EEBBF46E";
    const TARGET_SPEAKER_EMBEDDING_DIMENSION: usize = 192;
    const RUNTIME_ARCHIVE_SHA256: &str =
        "6760B0E25EAAD0DADFFBA9029B1270778E0DBFD43F314CF070B2F9C1DCB4AF25";
    const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";
    const RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.1/sherpa-onnx-v1.13.1-win-x64-shared-MD-Release-no-tts.tar.bz2";
    const KEYRING_SERVICE: &str = "com.listener.type.voiceprint";
    const KEYRING_ACCOUNT: &str = "owner-template-v1";
    const KEYRING_SUPPLEMENTAL_ACCOUNT_PREFIX: &str = "owner-template-v2-";
    // v3 enrollment stores independent wake and session-comparison banks. Keep
    // the existing account prefix so upgrades can read/delete v1/v2 material,
    // but reserve enough protected entries for both banks.
    const MAX_SUPPLEMENTAL_TEMPLATES: usize = 7;
    const SAMPLE_RATE: i32 = 16_000;
    const VERIFICATION_MIN_SPEECH_MS: usize = 1_000;
    // Xiaomi-style guided enrollment: three independent wake-phrase slots. Keep
    // one continuous device capture so the guidance does not introduce BLE
    // start/stop races. The same three samples also seed the session-comparison
    // bank, so model implementation details do not add a fourth user step.
    const ENROLLMENT_WAKE_STEP_SECONDS: u64 = 3;
    const ENROLLMENT_WAKE_STEPS: u64 = 3;
    const ENROLLMENT_STEP_COUNT: usize = 3;
    const ENROLLMENT_WAKE_SECONDS: u64 = ENROLLMENT_WAKE_STEP_SECONDS * ENROLLMENT_WAKE_STEPS;
    const ENROLLMENT_SECONDS: u64 = ENROLLMENT_WAKE_SECONDS;
    // The host stops at the nine-second boundary, but the last BLE audio packet
    // can arrive just before the control STOP (the observed failure was 20 ms
    // short with zero missing packets). Tolerate only that bounded transport
    // tail and pad it with silence; a genuinely missing third phrase still goes
    // through the per-step speech and KWS gates below.
    const ENROLLMENT_TRANSPORT_TAIL_TOLERANCE_MS: usize = 250;
    const ENROLLMENT_MIN_SECONDS: usize = 5;
    const ENROLLMENT_FRAME_MS: usize = 100;
    const ENROLLMENT_MIN_ACTIVE_FRAMES: usize = 18;
    const TEMPLATE_WINDOW_MS: usize = 1_600;
    const ENROLLMENT_TEMPLATE_WINDOW_MS: usize = 1_000;
    const DUAL_TEMPLATE_WINDOWS_PER_BANK: usize = 3;
    const ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP: usize = 6;
    // Product sensitivity: platform DEFAULT_SCORE_MILLI is 500 (0.50). Real-owner
    // wake in mild noise often scores ~0.43–0.55; 0.50 cut too many true hits.
    // Keep below same-speaker unit-test floor and well above typical non-owner.
    const VERIFICATION_THRESHOLD: f32 = 0.42;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CaptureState {
        Idle,
        Preparing,
        Armed,
        Capturing,
        Processing,
        Complete,
        Error,
    }

    impl CaptureState {
        fn label(self) -> &'static str {
            match self {
                Self::Idle => "idle",
                Self::Preparing => "preparing",
                Self::Armed => "armed",
                Self::Capturing => "capturing",
                Self::Processing => "processing",
                Self::Complete => "complete",
                Self::Error => "error",
            }
        }
    }

    #[derive(Default)]
    struct State {
        capture: Option<CaptureState>,
        progress: u8,
        error: Option<String>,
        last_score: Option<f32>,
        template: Option<SpeakerTemplate>,
        template_checked: bool,
        enrollment_phrase: Option<String>,
        enrollment_capture_started: Option<std::time::Instant>,
        enrollment_live: EnrollmentLiveState,
        runtime: Option<Arc<SpeakerRuntime>>,
    }

    #[derive(Default)]
    struct EnrollmentLiveState {
        analyzed_bytes: usize,
        step_speech_ms: [u16; 3],
        signal_level: u8,
        last_frame_active: bool,
        last_frame_clipped: bool,
    }

    static STATE: Lazy<Mutex<State>> = Lazy::new(|| Mutex::new(State::default()));
    static INFERENCE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct StoredTemplate {
        version: u8,
        model_sha256: String,
        dimension: usize,
        embedding_base64: String,
        #[serde(default)]
        phrase: Option<String>,
        #[serde(default)]
        invalidated: bool,
        #[serde(default)]
        purpose: Option<TemplatePurpose>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum TemplatePurpose {
        WakePhrase,
        FreeSpeech,
        TargetSpeaker,
    }

    #[derive(Debug, Clone)]
    struct SpeakerTemplate {
        wake_embeddings: Vec<Vec<f32>>,
        session_embeddings: Vec<Vec<f32>>,
        target_speaker_embedding: Option<Vec<f32>>,
        phrase: Option<String>,
        invalidated: bool,
    }

    #[repr(C)]
    struct ExtractorConfig {
        model: *const c_char,
        num_threads: i32,
        debug: i32,
        provider: *const c_char,
    }

    type CreateExtractor = unsafe extern "C" fn(*const ExtractorConfig) -> *const c_void;
    type DestroyExtractor = unsafe extern "C" fn(*const c_void);
    type ExtractorDim = unsafe extern "C" fn(*const c_void) -> i32;
    type CreateStream = unsafe extern "C" fn(*const c_void) -> *const c_void;
    type AcceptWaveform = unsafe extern "C" fn(*const c_void, i32, *const f32, i32);
    type InputFinished = unsafe extern "C" fn(*const c_void);
    type IsReady = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
    type ComputeEmbedding = unsafe extern "C" fn(*const c_void, *const c_void) -> *const f32;
    type DestroyEmbedding = unsafe extern "C" fn(*const f32);
    type DestroyStream = unsafe extern "C" fn(*const c_void);

    struct SpeakerRuntime {
        _onnx: Library,
        _providers: Library,
        _sherpa: Library,
        extractor: *const c_void,
        dimension: usize,
        create_stream: CreateStream,
        accept_waveform: AcceptWaveform,
        input_finished: InputFinished,
        is_ready: IsReady,
        compute_embedding: ComputeEmbedding,
        destroy_embedding: DestroyEmbedding,
        destroy_stream: DestroyStream,
        destroy_extractor: DestroyExtractor,
    }

    unsafe impl Send for SpeakerRuntime {}
    unsafe impl Sync for SpeakerRuntime {}

    impl Drop for SpeakerRuntime {
        fn drop(&mut self) {
            unsafe { (self.destroy_extractor)(self.extractor) };
        }
    }

    impl SpeakerRuntime {
        fn load(root: &Path) -> Result<Self, String> {
            Self::load_model(root, &root.join(MODEL_NAME))
        }

        fn load_model(root: &Path, model_path: &Path) -> Result<Self, String> {
            let onnx_path = root.join("onnxruntime.dll");
            let providers_path = root.join("onnxruntime_providers_shared.dll");
            let sherpa_path = root.join("sherpa-onnx-c-api.dll");
            let model = CString::new(model_path.to_string_lossy().as_bytes())
                .map_err(|_| "voiceprint model path contains an invalid character".to_string())?;
            let provider = CString::new("cpu").expect("literal has no nul");

            unsafe {
                let onnx = Library::new(&onnx_path)
                    .map_err(|err| format!("load onnxruntime.dll failed: {err}"))?;
                let providers = Library::new(&providers_path)
                    .map_err(|err| format!("load onnxruntime providers failed: {err}"))?;
                let sherpa = Library::new(&sherpa_path)
                    .map_err(|err| format!("load sherpa-onnx C API failed: {err}"))?;
                let create_extractor: CreateExtractor = *sherpa
                    .get(b"SherpaOnnxCreateSpeakerEmbeddingExtractor\0")
                    .map_err(|err| format!("missing speaker extractor API: {err}"))?;
                let destroy_extractor: DestroyExtractor = *sherpa
                    .get(b"SherpaOnnxDestroySpeakerEmbeddingExtractor\0")
                    .map_err(|err| format!("missing speaker extractor destroy API: {err}"))?;
                let extractor_dim: ExtractorDim = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorDim\0")
                    .map_err(|err| format!("missing speaker dimension API: {err}"))?;
                let create_stream: CreateStream = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorCreateStream\0")
                    .map_err(|err| format!("missing speaker stream API: {err}"))?;
                let accept_waveform: AcceptWaveform = *sherpa
                    .get(b"SherpaOnnxOnlineStreamAcceptWaveform\0")
                    .map_err(|err| format!("missing waveform API: {err}"))?;
                let input_finished: InputFinished = *sherpa
                    .get(b"SherpaOnnxOnlineStreamInputFinished\0")
                    .map_err(|err| format!("missing input-finished API: {err}"))?;
                let is_ready: IsReady = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorIsReady\0")
                    .map_err(|err| format!("missing speaker ready API: {err}"))?;
                let compute_embedding: ComputeEmbedding = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding\0")
                    .map_err(|err| format!("missing speaker compute API: {err}"))?;
                let destroy_embedding: DestroyEmbedding = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding\0")
                    .map_err(|err| format!("missing speaker embedding destroy API: {err}"))?;
                let destroy_stream: DestroyStream = *sherpa
                    .get(b"SherpaOnnxDestroyOnlineStream\0")
                    .map_err(|err| format!("missing speaker stream destroy API: {err}"))?;
                let config = ExtractorConfig {
                    model: model.as_ptr(),
                    num_threads: 1,
                    debug: 0,
                    provider: provider.as_ptr(),
                };
                let extractor = create_extractor(&config);
                if extractor.is_null() {
                    return Err("voiceprint model initialization failed".to_string());
                }
                let dimension = extractor_dim(extractor);
                if dimension <= 0 || dimension > 4096 {
                    destroy_extractor(extractor);
                    return Err(format!(
                        "voiceprint model returned invalid dimension: {dimension}"
                    ));
                }
                Ok(Self {
                    _onnx: onnx,
                    _providers: providers,
                    _sherpa: sherpa,
                    extractor,
                    dimension: dimension as usize,
                    create_stream,
                    accept_waveform,
                    input_finished,
                    is_ready,
                    compute_embedding,
                    destroy_embedding,
                    destroy_stream,
                    destroy_extractor,
                })
            }
        }

        fn embedding(&self, pcm: &[u8]) -> Result<Vec<f32>, String> {
            let _inference_guard = INFERENCE_LOCK.lock();
            let samples = prepare_embedding_samples(pcm)?;
            let sample_count =
                i32::try_from(samples.len()).map_err(|_| "voiceprint audio is too long")?;
            unsafe {
                let stream = (self.create_stream)(self.extractor);
                if stream.is_null() {
                    return Err("create voiceprint feature stream failed".to_string());
                }
                (self.accept_waveform)(stream, SAMPLE_RATE, samples.as_ptr(), sample_count);
                (self.input_finished)(stream);
                if (self.is_ready)(self.extractor, stream) == 0 {
                    (self.destroy_stream)(stream);
                    return Err("voiceprint audio has insufficient speech".to_string());
                }
                let raw = (self.compute_embedding)(self.extractor, stream);
                if raw.is_null() {
                    (self.destroy_stream)(stream);
                    return Err("voiceprint feature extraction failed".to_string());
                }
                let mut embedding = std::slice::from_raw_parts(raw, self.dimension).to_vec();
                (self.destroy_embedding)(raw);
                (self.destroy_stream)(stream);
                normalize(&mut embedding)?;
                Ok(embedding)
            }
        }
    }

    fn prepare_embedding_samples(pcm: &[u8]) -> Result<Vec<f32>, String> {
        let samples = pcm
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
            .collect::<Vec<_>>();
        let minimum_speech_samples = SAMPLE_RATE as usize * VERIFICATION_MIN_SPEECH_MS / 1000;
        if samples.len() < minimum_speech_samples {
            return Err(format!(
                "voiceprint audio is shorter than {VERIFICATION_MIN_SPEECH_MS} ms"
            ));
        }
        Ok(samples)
    }

    fn normalize(values: &mut [f32]) -> Result<(), String> {
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= f32::EPSILON {
            return Err("voiceprint feature is invalid".to_string());
        }
        for value in values {
            *value /= norm;
        }
        Ok(())
    }

    fn cosine(left: &[f32], right: &[f32]) -> Result<f32, String> {
        if left.len() != right.len() || left.is_empty() {
            return Err("voiceprint dimensions do not match".to_string());
        }
        Ok(left.iter().zip(right).map(|(a, b)| a * b).sum())
    }

    fn frame_rms(pcm: &[u8], frame_bytes: usize) -> Vec<f32> {
        pcm.chunks(frame_bytes)
            .map(|frame| {
                let mut sum = 0.0f64;
                let mut count = 0usize;
                for sample in frame.chunks_exact(2) {
                    let value = i16::from_le_bytes([sample[0], sample[1]]) as f64;
                    sum += value * value;
                    count += 1;
                }
                if count == 0 {
                    0.0
                } else {
                    (sum / count as f64).sqrt() as f32
                }
            })
            .collect()
    }

    fn enrollment_step_for_elapsed_ms(elapsed_ms: usize) -> usize {
        (elapsed_ms / (ENROLLMENT_WAKE_STEP_SECONDS as usize * 1_000))
            .min(ENROLLMENT_STEP_COUNT - 1)
    }

    fn enrollment_step_target_ms(_step: usize) -> u16 {
        (ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP * ENROLLMENT_FRAME_MS) as u16
    }

    fn enrollment_capture_feedback(
        step: usize,
        elapsed_ms: usize,
        live: &EnrollmentLiveState,
    ) -> &'static str {
        if live.last_frame_clipped {
            "too_loud"
        } else if live.step_speech_ms[step] >= enrollment_step_target_ms(step) {
            "good"
        } else if live.last_frame_active {
            "hearing"
        } else if elapsed_ms > step * ENROLLMENT_WAKE_STEP_SECONDS as usize * 1_000 + 1_000
            && live.step_speech_ms[step] < 200
        {
            "too_quiet"
        } else {
            "waiting"
        }
    }

    /// Update the enrollment wizard with real signal evidence while the actor keeps
    /// buffering one continuous BLE session. This is deliberately lightweight: it
    /// analyzes each 100 ms frame once and never runs speaker/KWS inference on the
    /// streaming actor.
    pub fn observe_enrollment_capture(pcm: &[u8]) {
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1_000;
        let mut state = STATE.lock();
        if state.capture != Some(CaptureState::Capturing) {
            return;
        }
        let mut offset = state.enrollment_live.analyzed_bytes;
        while offset.saturating_add(frame_bytes) <= pcm.len() {
            let frame = &pcm[offset..offset + frame_bytes];
            let rms = frame_rms(frame, frame_bytes)
                .first()
                .copied()
                .unwrap_or(0.0);
            let peak = frame
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs())
                .max()
                .unwrap_or(0);
            let step = enrollment_step_for_elapsed_ms(offset / 32);
            let active = rms >= 60.0;
            if active {
                state.enrollment_live.step_speech_ms[step] = state.enrollment_live.step_speech_ms
                    [step]
                    .saturating_add(ENROLLMENT_FRAME_MS as u16);
            }
            state.enrollment_live.signal_level = ((rms / 12.0).round() as u16).min(100) as u8;
            state.enrollment_live.last_frame_active = active;
            state.enrollment_live.last_frame_clipped = peak >= 32_000;
            offset += frame_bytes;
        }
        state.enrollment_live.analyzed_bytes = offset;
    }

    fn active_speech_bounds(
        pcm: &[u8],
        frame_bytes: usize,
        min_peak_rms: f32,
    ) -> Result<(usize, usize, usize, f32, f32, f32), String> {
        let frame_rms = frame_rms(pcm, frame_bytes);
        if frame_rms.is_empty() {
            return Err("voiceprint audio is empty".to_string());
        }
        let peak_rms = frame_rms.iter().copied().fold(0.0f32, f32::max);
        if peak_rms < min_peak_rms {
            return Err("voiceprint audio contains no clear speech".to_string());
        }
        let mut sorted_rms = frame_rms.clone();
        sorted_rms.sort_by(f32::total_cmp);
        let reference_index = (sorted_rms.len() * 85 / 100).min(sorted_rms.len() - 1);
        let reference_rms = sorted_rms[reference_index];
        let active_threshold = (reference_rms * 0.25).max(16.0);
        let active = frame_rms
            .iter()
            .enumerate()
            .filter_map(|(index, rms)| (*rms >= active_threshold).then_some(index))
            .collect::<Vec<_>>();
        if active.is_empty() {
            return Err("voiceprint audio contains no active speech".to_string());
        }
        Ok((
            active[0],
            active[active.len() - 1],
            active.len(),
            peak_rms,
            reference_rms,
            active_threshold,
        ))
    }

    fn enrollment_speech_window(pcm: &[u8]) -> Result<&[u8], String> {
        let min_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_MIN_SECONDS;
        if pcm.len() < min_bytes {
            return Err("声纹录制过短，请用自然语速连续说三遍唤醒词。".to_string());
        }

        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let (first_active, last_active, active_frames, peak_rms, reference_rms, active_threshold) =
            active_speech_bounds(pcm, frame_bytes, 60.0)
                .map_err(|_| "没有检测到清晰人声，请靠近设备并连续说三遍唤醒词。")?;
        log::info!(
            "[speaker-verification] enrollment audio quality frames={} active_frames={} peak_rms={peak_rms:.1} reference_rms={reference_rms:.1} threshold_rms={active_threshold:.1}",
            pcm.len().div_ceil(frame_bytes),
            active_frames
        );
        if active_frames < ENROLLMENT_MIN_ACTIVE_FRAMES {
            return Err("有效人声太短，请用自然语速完整说三遍唤醒词。".to_string());
        }

        let first = first_active.saturating_sub(2);
        let last = (last_active + 3).min(pcm.len().div_ceil(frame_bytes));
        let start = first * frame_bytes;
        let end = (last * frame_bytes).min(pcm.len()) & !1usize;
        Ok(&pcm[start..end])
    }

    fn evenly_spaced_windows(pcm: &[u8], window_ms: usize, count: usize) -> Vec<&[u8]> {
        let window_bytes = SAMPLE_RATE as usize * 2 * window_ms / 1000;
        if count == 0 || pcm.len() <= window_bytes {
            return vec![pcm];
        }
        let last_start = pcm.len().saturating_sub(window_bytes) & !1usize;
        (0..count)
            .map(|index| {
                let start = if count == 1 {
                    last_start / 2
                } else {
                    (last_start * index / (count - 1)) & !1usize
                };
                &pcm[start..start + window_bytes]
            })
            .collect()
    }

    #[derive(Debug)]
    struct EnrollmentTemplateWindows {
        wake: Vec<Vec<u8>>,
        session: Vec<Vec<u8>>,
    }

    fn enrollment_wake_step_slices(wake_pcm: &[u8]) -> Result<Vec<&[u8]>, String> {
        let step_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_WAKE_STEP_SECONDS as usize;
        let required_bytes = step_bytes * ENROLLMENT_WAKE_STEPS as usize;
        if wake_pcm.len() < required_bytes {
            return Err("三次唤醒词录制不完整，请重新录制。".to_string());
        }
        Ok((0..ENROLLMENT_WAKE_STEPS as usize)
            .map(|index| {
                let start = index * step_bytes;
                &wake_pcm[start..start + step_bytes]
            })
            .collect())
    }

    fn normalized_enrollment_wake_pcm(pcm: &[u8]) -> Result<Cow<'_, [u8]>, String> {
        let required_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_WAKE_SECONDS as usize;
        if pcm.len() >= required_bytes {
            return Ok(Cow::Borrowed(&pcm[..required_bytes]));
        }

        let tolerance_bytes =
            SAMPLE_RATE as usize * 2 * ENROLLMENT_TRANSPORT_TAIL_TOLERANCE_MS / 1_000;
        let missing_bytes = required_bytes - pcm.len();
        if missing_bytes > tolerance_bytes {
            return Err("声纹录制提前结束，请按提示完整说三次唤醒词。".to_string());
        }

        let mut normalized = Vec::with_capacity(required_bytes);
        normalized.extend_from_slice(pcm);
        normalized.resize(required_bytes, 0);
        log::info!(
            "[speaker-verification] tolerated bounded enrollment transport tail missing_bytes={} missing_ms={}",
            missing_bytes,
            missing_bytes / 32
        );
        Ok(Cow::Owned(normalized))
    }

    fn quality_wake_step_window(pcm: &[u8], step: usize) -> Result<Vec<u8>, String> {
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let compacted = compact_active_speech_frames(pcm, frame_bytes)
            .map_err(|_| format!("第 {step} 次没有检测到清晰唤醒词，请靠近设备重新录制。"))?;
        let active_frames = compacted.len() / frame_bytes;
        if active_frames < ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP {
            return Err(format!(
                "第 {step} 次唤醒词太短，请自然、完整地说出唤醒词。"
            ));
        }

        let window_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_TEMPLATE_WINDOW_MS / 1000;
        let mut window = Vec::with_capacity(window_bytes);
        while window.len() < window_bytes {
            let remaining = window_bytes - window.len();
            window.extend_from_slice(&compacted[..compacted.len().min(remaining)]);
        }
        Ok(window)
    }

    fn compact_active_speech_frames(pcm: &[u8], frame_bytes: usize) -> Result<Vec<u8>, String> {
        let (_, _, _, _, _, active_threshold) = active_speech_bounds(pcm, frame_bytes, 20.0)?;
        let rms = frame_rms(pcm, frame_bytes);
        let mut compacted = Vec::with_capacity(pcm.len());
        for (index, frame) in pcm.chunks(frame_bytes).enumerate() {
            if rms
                .get(index)
                .is_some_and(|value| *value >= active_threshold)
            {
                compacted.extend_from_slice(frame);
            }
        }
        Ok(compacted)
    }

    fn enrollment_template_windows(pcm: &[u8]) -> Result<EnrollmentTemplateWindows, String> {
        enrollment_speech_window(pcm)?;
        let wake_pcm = normalized_enrollment_wake_pcm(pcm)?;
        let wake = enrollment_wake_step_slices(wake_pcm.as_ref())?
            .into_iter()
            .enumerate()
            .map(|(index, step_pcm)| quality_wake_step_window(step_pcm, index + 1))
            .collect::<Result<Vec<_>, _>>()?;
        // Speaker embeddings are phrase-independent. Reuse the three independently
        // quality-gated and model-padded owner samples for the session bank instead
        // of imposing a second, stricter free-speech-era gate that contradicts the
        // visible 600 ms-per-step contract.
        let session = wake.clone();
        Ok(EnrollmentTemplateWindows { wake, session })
    }

    fn verification_speech_window(pcm: &[u8]) -> Result<&[u8], String> {
        let max_bytes = DEFAULT_MAX_CANDIDATE_MS as usize * 32;
        let pcm = &pcm[..pcm.len().min(max_bytes) & !1usize];
        let minimum_bytes = SAMPLE_RATE as usize * 2 * VERIFICATION_MIN_SPEECH_MS / 1000;
        if pcm.len() < minimum_bytes {
            return Err(format!(
                "voiceprint audio is shorter than {VERIFICATION_MIN_SPEECH_MS} ms"
            ));
        }

        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let (first_active, last_active, _, _, _, _) = active_speech_bounds(pcm, frame_bytes, 20.0)?;
        let frame_count = pcm.len().div_ceil(frame_bytes);
        let mut first = first_active.saturating_sub(1);
        let mut last = (last_active + 2).min(frame_count);
        // The final analysis frame may be partial. Counting frames made a nominal
        // ten-frame window as short as 901 ms when speech ended at the live PCM
        // tail, so the embedding layer rejected an otherwise ready owner check
        // and the wake gate waited for the next 2.4 s retry snapshot. Expand by
        // real bytes instead; the caller has already established that the full
        // candidate contains at least `minimum_bytes`.
        while (last * frame_bytes)
            .min(pcm.len())
            .saturating_sub(first * frame_bytes)
            < minimum_bytes
        {
            if first > 0 {
                first -= 1;
            } else if last < frame_count {
                last += 1;
            } else {
                break;
            }
        }
        let start = first * frame_bytes;
        let end = (last * frame_bytes).min(pcm.len()) & !1usize;
        Ok(&pcm[start..end])
    }

    fn session_speaker_speech_window(pcm: &[u8]) -> Result<&[u8], String> {
        let max_bytes = DEFAULT_MAX_CANDIDATE_MS as usize * 32;
        let pcm = &pcm[..pcm.len().min(max_bytes) & !1usize];
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let (first_active, last_active, _, _, _, _) = active_speech_bounds(pcm, frame_bytes, 20.0)?;
        let frame_count = pcm.len().div_ceil(frame_bytes);
        let first = first_active.saturating_sub(1);
        let last = (last_active + 2).min(frame_count);
        let start = first * frame_bytes;
        let end = (last * frame_bytes).min(pcm.len()) & !1usize;
        let speech = &pcm[start..end];
        if speech.is_empty() {
            return Err("voiceprint audio contains no active speech".to_string());
        }
        Ok(speech)
    }

    fn session_speaker_signal_metrics(pcm: &[u8]) -> Result<(usize, f32, f32), String> {
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1_000;
        let (_, _, active_frames, peak_rms, reference_rms, _) =
            active_speech_bounds(pcm, frame_bytes, 20.0)?;
        Ok((active_frames * ENROLLMENT_FRAME_MS, peak_rms, reference_rms))
    }

    fn verification_template_windows(pcm: &[u8]) -> Result<Vec<&[u8]>, String> {
        let max_bytes = DEFAULT_MAX_CANDIDATE_MS as usize * 32;
        let raw = &pcm[..pcm.len().min(max_bytes) & !1usize];
        let speech = verification_speech_window(pcm)?;
        let mut windows = vec![raw];
        if speech.as_ptr() != raw.as_ptr() || speech.len() != raw.len() {
            windows.push(speech);
        }
        if speech.len() > SAMPLE_RATE as usize * 2 * TEMPLATE_WINDOW_MS / 1000 {
            windows.extend(evenly_spaced_windows(speech, TEMPLATE_WINDOW_MS, 2));
        }
        Ok(windows)
    }

    fn pad_session_speaker_pcm(pcm: &[u8]) -> Vec<u8> {
        let minimum_bytes = SAMPLE_RATE as usize * 2 * VERIFICATION_MIN_SPEECH_MS / 1000;
        if pcm.len() >= minimum_bytes || pcm.is_empty() {
            return pcm.to_vec();
        }
        let mut padded = Vec::with_capacity(minimum_bytes);
        while padded.len() < minimum_bytes {
            let remaining = minimum_bytes - padded.len();
            padded.extend_from_slice(&pcm[..pcm.len().min(remaining)]);
        }
        padded
    }

    fn sha256(path: &Path) -> Result<String, String> {
        let file = fs::File::open(path).map_err(|err| format!("read download failed: {err}"))?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|err| format!("hash download failed: {err}"))?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(format!("{:X}", hasher.finalize()))
    }

    fn download_verified(url: &str, destination: &Path, expected: &str) -> Result<(), String> {
        if destination.exists() && sha256(destination)? == expected {
            return Ok(());
        }
        let temp = destination.with_extension("download");
        let response = reqwest::blocking::get(url)
            .map_err(|err| format!("download voiceprint component failed: {err}"))?
            .error_for_status()
            .map_err(|err| format!("voiceprint download returned an error: {err}"))?;
        let bytes = response
            .bytes()
            .map_err(|err| format!("read voiceprint download failed: {err}"))?;
        fs::write(&temp, &bytes)
            .map_err(|err| format!("write voiceprint component failed: {err}"))?;
        let actual = sha256(&temp)?;
        if actual != expected {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "voiceprint component hash mismatch: expected={expected} actual={actual}"
            ));
        }
        fs::rename(&temp, destination)
            .map_err(|err| format!("install voiceprint component failed: {err}"))
    }

    fn extract_runtime(root: &Path, archive_path: &Path) -> Result<(), String> {
        let required = [
            "onnxruntime.dll",
            "onnxruntime_providers_shared.dll",
            "sherpa-onnx-c-api.dll",
        ];
        if required.iter().all(|name| root.join(name).exists()) {
            return Ok(());
        }
        let file =
            fs::File::open(archive_path).map_err(|err| format!("open runtime failed: {err}"))?;
        let decoder = bzip2::read::BzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive
            .entries()
            .map_err(|err| format!("read runtime archive failed: {err}"))?
        {
            let mut entry = entry.map_err(|err| format!("read runtime entry failed: {err}"))?;
            let path = entry
                .path()
                .map_err(|err| format!("read runtime path failed: {err}"))?;
            let Some(name) = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if required.contains(&name.as_str())
                && path.components().any(|part| part.as_os_str() == "lib")
            {
                entry
                    .unpack(root.join(&name))
                    .map_err(|err| format!("install runtime {name} failed: {err}"))?;
            }
        }
        if required.iter().all(|name| root.join(name).exists()) {
            Ok(())
        } else {
            Err("voiceprint runtime archive is missing required DLLs".to_string())
        }
    }

    fn ensure_runtime_assets() -> Result<std::path::PathBuf, String> {
        let root = crate::persistence::speaker_verification_root()
            .map_err(|err| format!("create voiceprint model directory failed: {err}"))?;
        let runtime_ready = [
            "onnxruntime.dll",
            "onnxruntime_providers_shared.dll",
            "sherpa-onnx-c-api.dll",
        ]
        .iter()
        .all(|name| root.join(name).exists());
        if runtime_ready {
            return Ok(root);
        }
        let archive_path = root.join(format!("sherpa-onnx-{RUNTIME_VERSION}.tar.bz2"));
        download_verified(RUNTIME_URL, &archive_path, RUNTIME_ARCHIVE_SHA256)?;
        extract_runtime(&root, &archive_path)?;
        Ok(root)
    }

    fn ensure_runtime() -> Result<Arc<SpeakerRuntime>, String> {
        if let Some(runtime) = STATE.lock().runtime.clone() {
            return Ok(runtime);
        }
        let root = ensure_runtime_assets()?;
        download_verified(MODEL_URL, &root.join(MODEL_NAME), MODEL_SHA256)?;
        let runtime = Arc::new(SpeakerRuntime::load(&root)?);
        STATE.lock().runtime = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    fn keyring_entry_for(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, account)
            .map_err(|err| format!("open system voiceprint credential failed: {err}"))
    }

    fn keyring_entry() -> Result<keyring::Entry, String> {
        keyring_entry_for(KEYRING_ACCOUNT)
    }

    fn supplemental_keyring_entry(index: usize) -> Result<keyring::Entry, String> {
        keyring_entry_for(&format!("{KEYRING_SUPPLEMENTAL_ACCOUNT_PREFIX}{index}"))
    }

    fn delete_stored_credentials() -> Result<(), String> {
        match keyring_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => {
                return Err(format!("delete system voiceprint credential failed: {err}"));
            }
        }
        for index in 0..MAX_SUPPLEMENTAL_TEMPLATES {
            match supplemental_keyring_entry(index)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(err) => {
                    return Err(format!(
                        "delete supplemental system voiceprint credential failed: {err}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn clear_stored_credentials_after_failed_enrollment() {
        if let Err(err) = delete_stored_credentials() {
            log::warn!(
                "[speaker-verification] failed to clear partial enrollment credentials: {err}"
            );
        }
    }

    fn encode_template(
        embedding: &[f32],
        phrase: &str,
        invalidated: bool,
        purpose: TemplatePurpose,
    ) -> Result<String, String> {
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for value in embedding {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let model_sha256 = match purpose {
            TemplatePurpose::WakePhrase | TemplatePurpose::FreeSpeech => MODEL_SHA256,
            TemplatePurpose::TargetSpeaker => TARGET_SPEAKER_MODEL_SHA256,
        };
        serde_json::to_string(&StoredTemplate {
            version: 4,
            model_sha256: model_sha256.to_string(),
            dimension: embedding.len(),
            embedding_base64: BASE64.encode(bytes),
            phrase: Some(phrase.to_string()),
            invalidated,
            purpose: Some(purpose),
        })
        .map_err(|err| format!("encode voiceprint template failed: {err}"))
    }

    fn decode_template(value: &str) -> Result<SpeakerTemplate, String> {
        let stored: StoredTemplate = serde_json::from_str(value)
            .map_err(|err| format!("voiceprint template damaged: {err}"))?;
        if !matches!(stored.version, 1 | 2 | 3 | 4) {
            return Err("voiceprint template is incompatible with the current model".to_string());
        }
        let expected_model_sha256 = match stored.purpose {
            Some(TemplatePurpose::TargetSpeaker) => TARGET_SPEAKER_MODEL_SHA256,
            _ => MODEL_SHA256,
        };
        if stored.model_sha256 != expected_model_sha256 {
            return Err("voiceprint template is incompatible with the current model".to_string());
        }
        if stored.version >= 2
            && stored
                .phrase
                .as_ref()
                .is_none_or(|phrase| phrase.trim().is_empty())
        {
            return Err("voiceprint template wake phrase is invalid".to_string());
        }
        let bytes = BASE64
            .decode(stored.embedding_base64)
            .map_err(|err| format!("voiceprint template damaged: {err}"))?;
        if bytes.len() != stored.dimension * 4 || stored.dimension == 0 {
            return Err("voiceprint template dimension is invalid".to_string());
        }
        let mut embedding = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<_>>();
        if embedding.iter().any(|value| !value.is_finite()) {
            return Err("voiceprint template contains a non-finite value".to_string());
        }
        if stored.purpose == Some(TemplatePurpose::TargetSpeaker) {
            if embedding.len() != TARGET_SPEAKER_EMBEDDING_DIMENSION {
                return Err("target-speaker template dimension is invalid".to_string());
            }
        } else {
            normalize(&mut embedding)?;
        }
        let (wake_embeddings, session_embeddings, target_speaker_embedding) = match stored.purpose {
            Some(TemplatePurpose::WakePhrase) => (vec![embedding], Vec::new(), None),
            Some(TemplatePurpose::FreeSpeech) => (Vec::new(), vec![embedding], None),
            Some(TemplatePurpose::TargetSpeaker) => (Vec::new(), Vec::new(), Some(embedding)),
            None => {
                // v1/v2 had one undifferentiated bank. Preserve compatibility
                // until the owner re-enrolls with the dual-template prompt.
                (vec![embedding.clone()], vec![embedding], None)
            }
        };
        Ok(SpeakerTemplate {
            wake_embeddings,
            session_embeddings,
            target_speaker_embedding,
            phrase: stored.phrase,
            invalidated: stored.invalidated,
        })
    }

    fn persist_template(template: &SpeakerTemplate) -> Result<(), String> {
        let phrase = template
            .phrase
            .as_deref()
            .ok_or_else(|| "voiceprint template wake phrase is missing".to_string())?;
        if template.wake_embeddings.is_empty() || template.session_embeddings.is_empty() {
            return Err("voiceprint template requires wake and session banks".to_string());
        }
        #[cfg(feature = "target-speaker-extraction")]
        if template.target_speaker_embedding.is_none() {
            return Err("voiceprint template requires a target-speaker bank".to_string());
        }
        let encoded = template
            .wake_embeddings
            .iter()
            .map(|embedding| {
                encode_template(
                    embedding,
                    phrase,
                    template.invalidated,
                    TemplatePurpose::WakePhrase,
                )
            })
            .chain(template.session_embeddings.iter().map(|embedding| {
                encode_template(
                    embedding,
                    phrase,
                    template.invalidated,
                    TemplatePurpose::FreeSpeech,
                )
            }))
            .chain(template.target_speaker_embedding.iter().map(|embedding| {
                encode_template(
                    embedding,
                    phrase,
                    template.invalidated,
                    TemplatePurpose::TargetSpeaker,
                )
            }))
            .collect::<Result<Vec<_>, _>>()?;
        if encoded.len() > MAX_SUPPLEMENTAL_TEMPLATES + 1 {
            return Err("voiceprint template contains too many protected entries".to_string());
        }
        delete_stored_credentials()?;
        for index in 0..MAX_SUPPLEMENTAL_TEMPLATES {
            let entry = supplemental_keyring_entry(index)?;
            if let Some(value) = encoded.get(index + 1) {
                if let Err(err) = entry.set_password(value) {
                    clear_stored_credentials_after_failed_enrollment();
                    return Err(format!(
                        "save supplemental system voiceprint credential failed: {err}"
                    ));
                }
            }
        }
        if let Err(err) = keyring_entry()?.set_password(&encoded[0]) {
            clear_stored_credentials_after_failed_enrollment();
            return Err(format!("save system voiceprint credential failed: {err}"));
        }
        Ok(())
    }

    fn load_template_locked(state: &mut State) {
        if state.template_checked {
            return;
        }
        state.template_checked = true;
        let result = keyring_entry().and_then(|entry| match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(format!("read system voiceprint credential failed: {err}")),
        });
        match result {
            Ok(Some(value)) => match decode_template(&value) {
                Ok(mut template) => {
                    for index in 0..MAX_SUPPLEMENTAL_TEMPLATES {
                        let supplemental = supplemental_keyring_entry(index).and_then(|entry| {
                            match entry.get_password() {
                                Ok(value) => Ok(Some(value)),
                                Err(keyring::Error::NoEntry) => Ok(None),
                                Err(err) => Err(format!(
                                    "read supplemental voiceprint credential failed: {err}"
                                )),
                            }
                        });
                        match supplemental {
                            Ok(Some(value)) => match decode_template(&value) {
                                Ok(extra)
                                    if extra.phrase == template.phrase
                                        && extra.invalidated == template.invalidated =>
                                {
                                    template.wake_embeddings.extend(extra.wake_embeddings);
                                    template.session_embeddings.extend(extra.session_embeddings);
                                    if let Some(target) = extra.target_speaker_embedding {
                                        if template.target_speaker_embedding.is_none() {
                                            template.target_speaker_embedding = Some(target);
                                        } else {
                                            log::warn!(
                                                "[speaker-verification] ignored duplicate target-speaker template index={index}"
                                            );
                                        }
                                    }
                                }
                                Ok(_) => log::warn!(
                                    "[speaker-verification] ignored supplemental owner template index={index}: wake phrase binding differs"
                                ),
                                Err(err) => {
                                    log::warn!(
                                        "[speaker-verification] ignored supplemental owner template index={index}: {err}"
                                    );
                                }
                            },
                            Ok(None) => {}
                            Err(err) => log::warn!(
                                "[speaker-verification] supplemental owner template unavailable index={index}: {err}"
                            ),
                        }
                    }
                    state.template = Some(template);
                }
                Err(err) => state.error = Some(err),
            },
            Ok(None) => {}
            Err(err) => state.error = Some(err),
        }
    }

    fn bind_legacy_template_locked(state: &mut State, phrase: &str) {
        let Some(template) = state.template.as_mut() else {
            return;
        };
        if template.phrase.is_some() {
            return;
        }
        template.phrase = Some(phrase.to_string());
        if let Err(err) = persist_template(template) {
            template.phrase = None;
            state.error = Some(format!("绑定旧声纹到当前唤醒词失败: {err}"));
            return;
        }
        log::info!(
            "[speaker-verification] bound legacy owner template to current wake phrase={phrase}"
        );
    }

    fn load_template_for_phrase_locked(state: &mut State, phrase: &str) {
        load_template_locked(state);
        bind_legacy_template_locked(state, phrase);
    }

    fn template_matches_phrase(template: &SpeakerTemplate, phrase: &str) -> bool {
        !template.invalidated && template.phrase.as_deref() == Some(phrase)
    }

    fn assets_ready() -> (bool, bool) {
        let Ok(root) = crate::persistence::speaker_verification_root() else {
            return (false, false);
        };
        (
            root.join("sherpa-onnx-c-api.dll").exists() && root.join("onnxruntime.dll").exists(),
            root.join(MODEL_NAME).exists(),
        )
    }

    pub fn status_for_phrase(wake_phrase: &str) -> VoiceprintStatus {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)
            .unwrap_or_else(|_| wake_phrase.trim().to_string());
        let (runtime_ready, model_ready) = assets_ready();
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        let enrolled_phrase = state
            .template
            .as_ref()
            .and_then(|template| template.phrase.clone());
        let enrolled = state
            .template
            .as_ref()
            .is_some_and(|template| template_matches_phrase(template, &phrase));
        let requires_reenrollment = state.template.as_ref().is_some_and(|template| {
            !template_matches_phrase(template, &phrase)
                || (cfg!(feature = "target-speaker-extraction")
                    && template.target_speaker_embedding.is_none())
        });
        let capture_seconds_remaining = if state.capture == Some(CaptureState::Capturing) {
            state.enrollment_capture_started.map(|started| {
                ENROLLMENT_SECONDS
                    .saturating_sub(started.elapsed().as_secs())
                    .min(u8::MAX as u64) as u8
            })
        } else {
            None
        };
        let capture_elapsed_ms =
            (state.enrollment_live.analyzed_bytes / 32).min(u32::MAX as usize) as u32;
        let capture_step_index = enrollment_step_for_elapsed_ms(capture_elapsed_ms as usize);
        let capture_step = matches!(
            state.capture,
            Some(CaptureState::Capturing | CaptureState::Processing)
        )
        .then_some((capture_step_index + 1) as u8);
        let capture_feedback = (state.capture == Some(CaptureState::Capturing)).then(|| {
            enrollment_capture_feedback(
                capture_step_index,
                capture_elapsed_ms as usize,
                &state.enrollment_live,
            )
            .to_string()
        });
        let progress = capture_seconds_remaining
            .map(|remaining| {
                let elapsed = ENROLLMENT_SECONDS.saturating_sub(remaining as u64);
                20u64.saturating_add(elapsed.saturating_mul(50) / ENROLLMENT_SECONDS) as u8
            })
            .unwrap_or(state.progress);
        VoiceprintStatus {
            available: true,
            runtime_ready,
            model_ready,
            enrolled,
            enrolled_phrase,
            requires_reenrollment,
            state: state
                .capture
                .unwrap_or(CaptureState::Idle)
                .label()
                .to_string(),
            progress,
            capture_seconds_remaining,
            capture_step,
            capture_step_count: ENROLLMENT_STEP_COUNT as u8,
            capture_elapsed_ms,
            step_speech_ms: state.enrollment_live.step_speech_ms.to_vec(),
            signal_level: state.enrollment_live.signal_level,
            capture_feedback,
            score: state.last_score,
            threshold: VERIFICATION_THRESHOLD,
            error: state.error.clone(),
            model_name: MODEL_NAME,
            runtime_version: RUNTIME_VERSION,
            local_only: true,
        }
    }

    pub fn status() -> VoiceprintStatus {
        status_for_phrase("开始录音")
    }

    pub fn is_enrolled_for_phrase(wake_phrase: &str) -> bool {
        let Ok(phrase) = crate::wake_phrase::normalize_configured_phrase(wake_phrase) else {
            return false;
        };
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        state
            .template
            .as_ref()
            .is_some_and(|template| template_matches_phrase(template, &phrase))
    }

    pub fn prepare_for_phrase(wake_phrase: &str) -> Result<(), String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let started = std::time::Instant::now();
        ensure_runtime()?;
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        if !state
            .template
            .as_ref()
            .is_some_and(|template| template_matches_phrase(template, &phrase))
        {
            return Err("voiceprint is not enrolled".to_string());
        }
        log::info!(
            "[speaker-verification] owner gate prepared phrase={} elapsed_ms={}",
            phrase,
            started.elapsed().as_millis()
        );
        Ok(())
    }

    pub(crate) fn prepare_runtime_assets() -> Result<(), String> {
        ensure_runtime_assets().map(|_| ())
    }

    pub fn start_enrollment(wake_phrase: &str) -> Result<VoiceprintStatus, String> {
        let wake_phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        log::info!(
            "[speaker-verification] enrollment requested for configured wake phrase={wake_phrase}"
        );
        {
            let mut state = STATE.lock();
            if matches!(
                state.capture,
                Some(CaptureState::Preparing | CaptureState::Armed | CaptureState::Capturing)
            ) {
                return Err("voiceprint enrollment is already active".to_string());
            }
            state.capture = Some(CaptureState::Preparing);
            state.progress = 5;
            state.error = None;
            state.last_score = None;
            state.enrollment_phrase = Some(wake_phrase.clone());
            state.enrollment_capture_started = None;
            state.enrollment_live = EnrollmentLiveState::default();
        }
        if let Err(err) = ensure_runtime() {
            mark_error(&err);
            return Err(err);
        }
        {
            let mut state = STATE.lock();
            state.capture = Some(CaptureState::Armed);
            state.progress = 20;
        }
        if let Err(err) =
            crate::embedded_ble::send_recording_control_enrollment(Duration::from_secs(4))
        {
            mark_error(&err);
            return Err(err);
        }
        Ok(status_for_phrase(&wake_phrase))
    }

    fn enrollment_capture_needs_host_stop(capture: Option<CaptureState>) -> bool {
        matches!(capture, Some(CaptureState::Armed | CaptureState::Capturing))
    }

    pub fn take_enrollment_arm() -> bool {
        let armed = {
            let mut state = STATE.lock();
            if state.capture == Some(CaptureState::Armed) {
                state.capture = Some(CaptureState::Capturing);
                state.progress = 35;
                state.enrollment_capture_started = Some(std::time::Instant::now());
                true
            } else {
                false
            }
        };
        if armed {
            // The capture clock starts only after the dedicated device session
            // actually arrives. BLE recovery or hidden-candidate preemption in
            // the Preparing/Armed phase must not shorten the owner sample.
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(ENROLLMENT_SECONDS));
                let capture_still_active = enrollment_capture_needs_host_stop(STATE.lock().capture);
                if !capture_still_active {
                    log::info!(
                        "[speaker-verification] enrollment stop timer skipped because device session already completed"
                    );
                    return;
                }
                if let Err(err) =
                    crate::embedded_ble::send_recording_control_stop(Duration::from_secs(4))
                {
                    mark_error(&format!("stop voiceprint enrollment failed: {err}"));
                }
            });
        }
        armed
    }

    pub fn begin_enrollment_processing() {
        let mut state = STATE.lock();
        if matches!(
            state.capture,
            Some(CaptureState::Armed | CaptureState::Capturing)
        ) {
            state.capture = Some(CaptureState::Processing);
            state.progress = 70;
            state.enrollment_capture_started = None;
        }
    }

    pub fn finish_enrollment(pcm: &[u8], wake_phrase: &str) -> Result<VoiceprintStatus, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        {
            let mut state = STATE.lock();
            if state.enrollment_phrase.as_deref() != Some(phrase.as_str()) {
                return Err("录制期间唤醒词已改变，请重新录制声纹。".to_string());
            }
            state.capture = Some(CaptureState::Processing);
            state.progress = 70;
            state.enrollment_capture_started = None;
        }
        let result: Result<VoiceprintStatus, String> = (|| {
            let runtime = ensure_runtime()?;
            let windows = enrollment_template_windows(pcm)?;
            // Reject silence/short speech before invoking KWS, then validate the
            // phrase before persisting either template bank.
            let wake_pcm = normalized_enrollment_wake_pcm(pcm)?;
            for (index, step_pcm) in enrollment_wake_step_slices(wake_pcm.as_ref())?
                .iter()
                .enumerate()
            {
                crate::wake_phrase::calibrate(step_pcm, &phrase).map_err(|_| {
                    format!(
                        "第 {} 次没有识别到“{}”，请按每一步提示清晰重录。",
                        index + 1,
                        phrase
                    )
                })?;
            }
            let wake_embeddings = windows
                .wake
                .iter()
                .map(|speech| {
                    runtime
                        .embedding(speech)
                        .map_err(|err| format!("唤醒词声纹特征提取失败，请重新录制：{err}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let session_embeddings = windows
                .session
                .iter()
                .map(|speech| {
                    runtime
                        .embedding(speech)
                        .map_err(|err| format!("会话声纹特征提取失败，请重新录制：{err}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            #[cfg(feature = "target-speaker-extraction")]
            let target_speaker_embedding = {
                let target_enrollment_pcm = windows.wake.concat();
                crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(
                    &target_enrollment_pcm,
                )
                .map(Some)
                .map_err(|err| format!("多人隔离声纹特征提取失败，请重新录制：{err}"))?
            };
            #[cfg(not(feature = "target-speaker-extraction"))]
            let target_speaker_embedding = None;
            let template = SpeakerTemplate {
                wake_embeddings,
                session_embeddings,
                target_speaker_embedding,
                phrase: Some(phrase.clone()),
                invalidated: false,
            };
            persist_template(&template)?;
            {
                let mut state = STATE.lock();
                state.template = Some(template);
                state.template_checked = true;
                state.capture = Some(CaptureState::Complete);
                state.progress = 100;
                state.last_score = None;
                state.error = None;
                state.enrollment_phrase = None;
                state.enrollment_capture_started = None;
            }
            Ok(status_for_phrase(&phrase))
        })();
        if let Err(err) = &result {
            mark_error(err);
        }
        result
    }

    pub fn verify(pcm: &[u8], wake_phrase: &str) -> Result<VerificationResult, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        // 小爱同学模式：未注册主人声纹 → 不做说话人校验，唤醒词命中即放行（任何人可唤醒）；
        // 注册后才执行声纹匹配（仅主人能唤醒）。
        // 兼带修复旧逻辑的坑：以前未注册时这里返回 Err，导致 gate 把"开始录音"判定为
        // voiceprint_verification_failed 而拒唤醒——没录声纹反而完全唤醒不了。
        // 未注册时直接短路返回，避免无谓加载 ONNX runtime/模型（省几百 ms 延迟 + 网络下载）。
        let template = {
            let mut state = STATE.lock();
            load_template_for_phrase_locked(&mut state, &phrase);
            state.template.clone()
        };
        let template = match template {
            Some(template) if template_matches_phrase(&template, &phrase) => template,
            None => {
                log::info!(
                    "[speaker-verification] owner not enrolled for phrase={} — open gate (any speaker may wake), pcm_ms={}",
                    phrase,
                    pcm.len() / 32
                );
                return Ok(VerificationResult {
                    matched: true,
                    score: 0.0,
                });
            }
            Some(template) => {
                log::info!(
                    "[speaker-verification] owner template inactive for phrase={} enrolled_phrase={} invalidated={} — open gate until re-enrollment",
                    phrase,
                    template.phrase.as_deref().unwrap_or("-"),
                    template.invalidated
                );
                return Ok(VerificationResult {
                    matched: true,
                    score: 0.0,
                });
            }
        };
        let runtime = ensure_runtime()?;
        let candidate_windows = verification_template_windows(pcm)?;
        let inference_started = std::time::Instant::now();
        let candidate_embeddings = candidate_windows
            .iter()
            .map(|candidate| runtime.embedding(candidate))
            .collect::<Result<Vec<_>, _>>()?;
        let inference_ms = inference_started.elapsed().as_millis();
        let score = template
            .wake_embeddings
            .iter()
            .flat_map(|enrolled| {
                candidate_embeddings
                    .iter()
                    .filter_map(|candidate| cosine(enrolled, candidate).ok())
            })
            .max_by(f32::total_cmp)
            .ok_or_else(|| "voiceprint dimensions do not match".to_string())?;
        let verdict = if score >= VERIFICATION_THRESHOLD {
            Verdict::Match
        } else {
            Verdict::NonMatch
        };
        let decision = Machine::default().step(Input {
            enabled: true,
            enrolled: true,
            origin: CandidateOrigin::Automatic,
            candidate_complete: true,
            candidate_ms: (pcm.len() / 32) as u32,
            verdict,
        });
        log::info!(
            "[speaker-verification] compared enrolled_templates={} candidate_windows={} speech_ms={} inference_ms={inference_ms} score={score:.6}",
            template.wake_embeddings.len(),
            candidate_embeddings.len(),
            candidate_windows[0].len() / 32
        );
        STATE.lock().last_score = Some(score);
        Ok(VerificationResult {
            matched: matches!(decision.action, Action::Release),
            score,
        })
    }

    #[cfg(feature = "target-speaker-extraction")]
    pub fn target_speaker_embedding_for_phrase(
        wake_phrase: &str,
    ) -> Result<Option<Vec<f32>>, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        Ok(state
            .template
            .as_ref()
            .filter(|template| template_matches_phrase(template, &phrase))
            .and_then(|template| template.target_speaker_embedding.clone()))
    }

    pub fn session_profile_from_wake(
        pcm: &[u8],
        wake_end_seconds: f32,
        wake_phrase: &str,
    ) -> Result<SessionSpeakerProfile, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let enrolled = {
            let mut state = STATE.lock();
            load_template_for_phrase_locked(&mut state, &phrase);
            state
                .template
                .as_ref()
                .filter(|template| template_matches_phrase(template, &phrase))
                .map(|template| template.session_embeddings.clone())
        };
        if let Some(mut embeddings) = enrolled {
            match verified_wake_session_embedding(pcm, wake_end_seconds) {
                Ok((verified_wake_embedding, source_speech_ms, model_input_ms, inference_ms)) => {
                    let added = super::append_verified_wake_session_exemplar(
                        &mut embeddings,
                        verified_wake_embedding,
                    );
                    log::info!(
                        "[speaker-verification] enrolled session target prepared phrase={} wake_end_ms={} speech_ms={} model_input_ms={} inference_ms={inference_ms} persisted_exemplars={} live_verified_exemplar_added={added}",
                        phrase,
                        (wake_end_seconds * 1000.0).round() as u64,
                        source_speech_ms,
                        model_input_ms,
                        embeddings.len().saturating_sub(usize::from(added)),
                    );
                }
                Err(err) => log::warn!(
                    "[speaker-verification] live verified wake exemplar unavailable; preserving enrolled session bank: {err}"
                ),
            }
            return Ok(SessionSpeakerProfile {
                embeddings: Arc::new(embeddings),
                adaptive: false,
            });
        }
        let (verified_wake_embedding, source_speech_ms, model_input_ms, inference_ms) =
            verified_wake_session_embedding(pcm, wake_end_seconds)?;
        log::info!(
            "[speaker-verification] ephemeral session target prepared phrase={} wake_end_ms={} speech_ms={} model_input_ms={} inference_ms={inference_ms}",
            phrase,
            (wake_end_seconds * 1000.0).round() as u64,
            source_speech_ms,
            model_input_ms,
        );
        Ok(SessionSpeakerProfile {
            embeddings: Arc::new(vec![verified_wake_embedding]),
            adaptive: true,
        })
    }

    fn verified_wake_session_embedding(
        pcm: &[u8],
        wake_end_seconds: f32,
    ) -> Result<(Vec<f32>, usize, usize, u128), String> {
        let wake_end_bytes =
            ((wake_end_seconds.max(0.0) * 32_000.0).round() as usize).min(pcm.len()) & !1usize;
        let profile_end_bytes = wake_end_bytes.saturating_add(250 * 32).min(pcm.len()) & !1usize;
        let profile_window_bytes = TEMPLATE_WINDOW_MS * 32;
        let profile_start = profile_end_bytes.saturating_sub(profile_window_bytes) & !1usize;
        let focused = pcm
            .get(profile_start..profile_end_bytes)
            .ok_or_else(|| "wake speaker window is outside candidate audio".to_string())?;
        let speech = session_speaker_speech_window(focused)?;
        let source_speech_ms = speech.len() / 32;
        let model_pcm = pad_session_speaker_pcm(speech);
        let model_input_ms = model_pcm.len() / 32;
        let runtime = ensure_runtime()?;
        let inference_started = std::time::Instant::now();
        let embedding = runtime.embedding(&model_pcm)?;
        Ok((
            embedding,
            source_speech_ms,
            model_input_ms,
            inference_started.elapsed().as_millis(),
        ))
    }

    pub fn observe_session_speaker(
        profile: &SessionSpeakerProfile,
        pcm: &[u8],
    ) -> Result<SessionSpeakerObservation, String> {
        let runtime = ensure_runtime()?;
        let speech = session_speaker_speech_window(pcm)?;
        let speech_span_ms = speech.len() / 32;
        let (real_speech_ms, peak_rms, reference_rms) = session_speaker_signal_metrics(speech)?;
        let model_pcm = pad_session_speaker_pcm(speech);
        let inference_started = std::time::Instant::now();
        let candidate = runtime.embedding(&model_pcm)?;
        let inference_ms = inference_started.elapsed().as_millis();
        let score = profile
            .embeddings
            .iter()
            .filter_map(|target| cosine(target, &candidate).ok())
            .max_by(f32::total_cmp)
            .ok_or_else(|| "session speaker dimensions do not match".to_string())?;
        // Keep a deliberate uncertainty band around the accepted owner threshold.
        // Repeat-padding is valid for the embedding model, but a subsecond body
        // fragment does not contain enough independent speech to exclude the
        // stabilized speaker.
        let classification =
            session_speaker_classification_for_signal(score, real_speech_ms, peak_rms);
        let transcript_hard_non_target =
            session_speaker_transcript_hard_non_target(score, real_speech_ms, peak_rms);
        log::info!(
            "[speaker-verification] local session sample real_speech_ms={real_speech_ms} speech_span_ms={speech_span_ms} model_input_ms={} peak_rms={peak_rms:.1} reference_rms={reference_rms:.1} inference_ms={inference_ms} score={score:.6} classification={classification:?} transcript_hard_non_target={transcript_hard_non_target}",
            model_pcm.len() / 32
        );
        Ok(SessionSpeakerObservation {
            classification,
            real_speech_ms,
            transcript_hard_non_target,
            embedding: candidate,
        })
    }

    pub fn classify_session_speaker(
        profile: &SessionSpeakerProfile,
        pcm: &[u8],
    ) -> Result<SessionSpeakerClassification, String> {
        observe_session_speaker(profile, pcm).map(|observation| observation.classification)
    }

    pub fn invalidate_for_phrase_change(
        previous_phrase: &str,
        next_phrase: &str,
    ) -> Result<(), String> {
        let previous = crate::wake_phrase::normalize_configured_phrase(previous_phrase)?;
        let next = crate::wake_phrase::normalize_configured_phrase(next_phrase)?;
        if previous == next {
            return Ok(());
        }
        let mut state = STATE.lock();
        if matches!(
            state.capture,
            Some(
                CaptureState::Preparing
                    | CaptureState::Armed
                    | CaptureState::Capturing
                    | CaptureState::Processing
            )
        ) {
            return Err("声纹录制进行中，完成或取消后才能更换唤醒词。".to_string());
        }
        load_template_for_phrase_locked(&mut state, &previous);
        let Some(mut template) = state.template.clone() else {
            return Ok(());
        };
        template.invalidated = true;
        persist_template(&template)?;
        state.template = Some(template);
        state.capture = Some(CaptureState::Idle);
        state.progress = 0;
        state.last_score = None;
        state.error = None;
        state.enrollment_phrase = None;
        state.enrollment_capture_started = None;
        state.enrollment_live = EnrollmentLiveState::default();
        log::info!(
            "[speaker-verification] owner template invalidated after wake phrase change previous={} next={}; re-enrollment required",
            previous,
            next
        );
        Ok(())
    }

    pub fn enrollment_should_process() -> bool {
        matches!(
            STATE.lock().capture,
            Some(CaptureState::Armed | CaptureState::Capturing | CaptureState::Processing)
        )
    }

    pub fn cancel_enrollment(wake_phrase: &str) -> Result<VoiceprintStatus, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let should_stop = {
            let mut state = STATE.lock();
            let should_stop = enrollment_capture_needs_host_stop(state.capture);
            state.capture = Some(CaptureState::Idle);
            state.progress = 0;
            state.error = None;
            state.enrollment_phrase = None;
            state.enrollment_capture_started = None;
            state.enrollment_live = EnrollmentLiveState::default();
            should_stop
        };
        if should_stop {
            crate::embedded_ble::send_recording_control_stop(Duration::from_secs(4))?;
        }
        log::info!("[speaker-verification] enrollment cancelled by owner");
        Ok(status_for_phrase(&phrase))
    }

    pub fn delete_template() -> Result<VoiceprintStatus, String> {
        delete_stored_credentials()?;
        let mut state = STATE.lock();
        state.template = None;
        state.template_checked = true;
        state.capture = Some(CaptureState::Idle);
        state.progress = 0;
        state.last_score = None;
        state.error = None;
        state.enrollment_phrase = None;
        state.enrollment_capture_started = None;
        state.enrollment_live = EnrollmentLiveState::default();
        drop(state);
        Ok(status())
    }

    fn mark_error(error: &str) {
        let mut state = STATE.lock();
        state.capture = Some(CaptureState::Error);
        state.error = Some(error.to_string());
        state.enrollment_capture_started = None;
    }

    pub fn fail_enrollment(error: &str) {
        mark_error(error);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn guided_enrollment_step_boundaries_match_the_three_visible_prompts() {
            assert_eq!(enrollment_step_for_elapsed_ms(0), 0);
            assert_eq!(enrollment_step_for_elapsed_ms(2_999), 0);
            assert_eq!(enrollment_step_for_elapsed_ms(3_000), 1);
            assert_eq!(enrollment_step_for_elapsed_ms(6_000), 2);
            assert_eq!(enrollment_step_for_elapsed_ms(9_000), 2);
            assert_eq!(enrollment_step_for_elapsed_ms(15_000), 2);
            assert_eq!(enrollment_step_target_ms(0), 600);
            assert_eq!(enrollment_step_target_ms(2), 600);
        }

        #[test]
        fn guided_enrollment_feedback_uses_real_signal_evidence() {
            let mut live = EnrollmentLiveState::default();
            assert_eq!(enrollment_capture_feedback(0, 0, &live), "waiting");
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "too_quiet");
            live.last_frame_active = true;
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "hearing");
            live.step_speech_ms[0] = enrollment_step_target_ms(0);
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "good");
            live.last_frame_clipped = true;
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "too_loud");
        }

        #[test]
        fn template_binary_round_trip_is_normalized_and_compact() {
            let mut embedding = (1..=192).map(|value| value as f32).collect::<Vec<_>>();
            normalize(&mut embedding).unwrap();
            let encoded =
                encode_template(&embedding, "小爱同学", false, TemplatePurpose::WakePhrase)
                    .unwrap();
            assert!(encoded.len() < 1800);
            let decoded = decode_template(&encoded).unwrap();
            assert!((cosine(&embedding, &decoded.wake_embeddings[0]).unwrap() - 1.0).abs() < 1e-5);
            assert!(decoded.session_embeddings.is_empty());
            assert!(decoded.target_speaker_embedding.is_none());
            assert_eq!(decoded.phrase.as_deref(), Some("小爱同学"));
            assert!(!decoded.invalidated);
        }

        #[test]
        fn template_purpose_separates_new_banks_and_maps_legacy_to_both() {
            let mut embedding = vec![1.0, 2.0, 3.0];
            normalize(&mut embedding).unwrap();
            let free = decode_template(
                &encode_template(&embedding, "开始录音", false, TemplatePurpose::FreeSpeech)
                    .unwrap(),
            )
            .unwrap();
            assert!(free.wake_embeddings.is_empty());
            assert_eq!(free.session_embeddings.len(), 1);

            let target = (0..TARGET_SPEAKER_EMBEDDING_DIMENSION)
                .map(|value| value as f32 / 100.0 - 0.5)
                .collect::<Vec<_>>();
            let decoded_target = decode_template(
                &encode_template(&target, "开始录音", false, TemplatePurpose::TargetSpeaker)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                decoded_target.target_speaker_embedding.as_deref(),
                Some(target.as_slice())
            );
            assert!(decoded_target.wake_embeddings.is_empty());
            assert!(decoded_target.session_embeddings.is_empty());

            let mut bytes = Vec::new();
            for value in &embedding {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            let legacy = serde_json::to_string(&StoredTemplate {
                version: 2,
                model_sha256: MODEL_SHA256.to_string(),
                dimension: embedding.len(),
                embedding_base64: BASE64.encode(bytes),
                phrase: Some("开始录音".into()),
                invalidated: false,
                purpose: None,
            })
            .unwrap();
            let legacy = decode_template(&legacy).unwrap();
            assert_eq!(legacy.wake_embeddings.len(), 1);
            assert_eq!(legacy.session_embeddings.len(), 1);
            assert!(
                (cosine(&legacy.wake_embeddings[0], &legacy.session_embeddings[0]).unwrap() - 1.0)
                    .abs()
                    < 1e-5
            );
        }

        #[test]
        fn corrupt_or_wrong_model_template_is_rejected() {
            assert!(decode_template("{}").is_err());
            let wrong = serde_json::to_string(&StoredTemplate {
                version: 1,
                model_sha256: "wrong".into(),
                dimension: 1,
                embedding_base64: BASE64.encode(0.5f32.to_le_bytes()),
                phrase: Some("小爱同学".into()),
                invalidated: false,
                purpose: None,
            })
            .unwrap();
            assert!(decode_template(&wrong).is_err());
        }

        #[test]
        fn voiceprint_template_only_protects_its_enrolled_phrase() {
            let template = SpeakerTemplate {
                wake_embeddings: vec![vec![1.0, 0.0]],
                session_embeddings: vec![vec![1.0, 0.0]],
                target_speaker_embedding: None,
                phrase: Some("小爱同学".into()),
                invalidated: false,
            };
            assert!(template_matches_phrase(&template, "小爱同学"));
            assert!(!template_matches_phrase(&template, "开始录音"));
            let invalidated = SpeakerTemplate {
                invalidated: true,
                ..template
            };
            assert!(!template_matches_phrase(&invalidated, "小爱同学"));
        }

        #[test]
        fn cosine_separates_aligned_and_opposed_vectors() {
            assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]).unwrap(), 1.0);
            assert_eq!(cosine(&[1.0, 0.0], &[-1.0, 0.0]).unwrap(), -1.0);
        }

        #[test]
        fn enrollment_window_keeps_repeated_phrase_span_and_drops_outer_silence() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; SAMPLE_RATE as usize];
            samples.extend(vec![800i16; frame_samples * 32]);
            samples.extend(vec![0i16; frame_samples * 2]);
            samples.extend(vec![-900i16; frame_samples * 32]);
            samples.extend(vec![0i16; SAMPLE_RATE as usize]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let window = enrollment_speech_window(&pcm).expect("speech window");
            assert!(window.len() < pcm.len());
            assert!(window.len() >= SAMPLE_RATE as usize * 2 * ENROLLMENT_MIN_SECONDS);
        }

        #[test]
        fn enrollment_builds_bounded_long_and_short_owner_templates() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![800i16; frame_samples * 90];
            samples.extend(vec![-900i16; frame_samples * 60]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let windows = enrollment_template_windows(&pcm).expect("template windows");
            assert_eq!(windows.wake.len(), DUAL_TEMPLATE_WINDOWS_PER_BANK);
            assert_eq!(windows.session.len(), DUAL_TEMPLATE_WINDOWS_PER_BANK);
            assert!(windows
                .wake
                .iter()
                .all(|window| window.len() == ENROLLMENT_TEMPLATE_WINDOW_MS * 32));
            assert!(windows
                .session
                .iter()
                .all(|window| window.len() == ENROLLMENT_TEMPLATE_WINDOW_MS * 32));
        }

        #[test]
        fn enrollment_accepts_three_minimum_length_phrases_with_volume_variation() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            // Real owner acceptance produced 600-800 ms phrases at visibly different
            // levels. Each 3-second slot must pass its own quality gate and then feed
            // both banks without a hidden second one-second-per-window requirement.
            let mut samples = vec![0i16; frame_samples * 5];
            samples.extend(vec![700i16; frame_samples * 8]);
            samples.extend(vec![0i16; frame_samples * 17]);
            samples.extend(vec![0i16; frame_samples * 10]);
            samples.extend(vec![4_000i16; frame_samples * 8]);
            samples.extend(vec![0i16; frame_samples * 12]);
            samples.extend(vec![0i16; frame_samples * 7]);
            samples.extend(vec![-700i16; frame_samples * 8]);
            samples.extend(vec![0i16; frame_samples * 15]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let windows = enrollment_template_windows(&pcm)
                .expect("three accepted phrase samples must build both template banks");
            assert_eq!(windows.wake.len(), 3);
            assert_eq!(windows.session.len(), 3);
            assert_eq!(windows.session, windows.wake);
            assert!(windows
                .wake
                .iter()
                .chain(&windows.session)
                .all(|window| window.len() >= VERIFICATION_MIN_SPEECH_MS * 32));
        }

        #[test]
        fn enrollment_accepts_a_bounded_ble_transport_tail_shortfall() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = Vec::new();
            for amplitude in [800i16, 1_200, -900] {
                samples.extend(vec![0i16; frame_samples * 8]);
                samples.extend(vec![amplitude; frame_samples * 8]);
                samples.extend(vec![0i16; frame_samples * 14]);
            }
            // Reproduce the real 2026-08-23 capture: 287,360 bytes (8.98 s)
            // instead of the nominal 288,000 bytes, with zero missing packets.
            samples.truncate(samples.len() - SAMPLE_RATE as usize * 20 / 1_000);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            assert_eq!(pcm.len(), 287_360);
            let windows = enrollment_template_windows(&pcm)
                .expect("a packet-boundary tail shortfall must not discard three valid phrases");
            assert_eq!(windows.wake.len(), 3);
        }

        #[test]
        fn enrollment_rejects_more_than_the_bounded_transport_tail() {
            let required_samples = SAMPLE_RATE as usize * ENROLLMENT_WAKE_SECONDS as usize;
            let missing_samples =
                SAMPLE_RATE as usize * (ENROLLMENT_TRANSPORT_TAIL_TOLERANCE_MS + 50) / 1_000;
            let pcm = vec![800i16; required_samples - missing_samples]
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            let error = enrollment_template_windows(&pcm)
                .expect_err("a genuinely early stop must remain rejected");
            assert!(error.contains("提前结束"), "unexpected error: {error}");
        }

        #[test]
        fn three_wake_steps_build_both_banks_without_a_fourth_prompt() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let samples = vec![800i16; frame_samples * 90];
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            let windows = enrollment_template_windows(&pcm).expect("three-step enrollment");
            assert!(windows
                .wake
                .iter()
                .all(|window| i16::from_le_bytes([window[0], window[1]]) > 0));
            assert!(windows
                .session
                .iter()
                .all(|window| i16::from_le_bytes([window[0], window[1]]) > 0));
        }

        #[test]
        fn enrollment_rejects_a_missing_wake_phrase_step() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![800i16; frame_samples * 30];
            samples.extend(vec![0i16; frame_samples * 30]);
            samples.extend(vec![-900i16; frame_samples * 30]);
            samples.extend(vec![750i16; frame_samples * 60]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            let error = enrollment_template_windows(&pcm)
                .expect_err("every displayed wake step must provide its own sample");
            assert!(error.contains("第 2 次"), "unexpected error: {error}");
        }

        #[test]
        fn completed_enrollment_cancels_late_host_stop() {
            assert!(enrollment_capture_needs_host_stop(Some(
                CaptureState::Armed
            )));
            assert!(enrollment_capture_needs_host_stop(Some(
                CaptureState::Capturing
            )));
            assert!(!enrollment_capture_needs_host_stop(Some(
                CaptureState::Complete
            )));
            assert!(!enrollment_capture_needs_host_stop(Some(
                CaptureState::Error
            )));
            assert!(!enrollment_capture_needs_host_stop(None));
        }

        #[test]
        fn enrollment_rejects_a_short_sound_inside_a_long_capture() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; frame_samples * 20];
            samples.extend(vec![900i16; frame_samples * 3]);
            samples.extend(vec![0i16; frame_samples * 20]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            assert!(enrollment_speech_window(&pcm).is_err());
        }

        #[test]
        fn verification_window_removes_outer_silence_and_keeps_model_floor() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; frame_samples * 6];
            samples.extend(vec![900i16; frame_samples * 12]);
            samples.extend(vec![0i16; frame_samples * 6]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let window = verification_speech_window(&pcm).expect("verification window");
            assert!(window.len() < pcm.len());
            assert!(window.len() >= VERIFICATION_MIN_SPEECH_MS * 32);
        }

        #[test]
        fn verification_window_keeps_model_floor_with_partial_live_tail_frame() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; frame_samples * 13];
            samples.extend(vec![900i16; frame_samples * 8]);
            samples.extend(vec![900i16; frame_samples * 8 / 10]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            assert_eq!(pcm.len() / 32, 2_180);
            let window = verification_speech_window(&pcm).expect("verification window");
            assert!(
                window.len() >= VERIFICATION_MIN_SPEECH_MS * 32,
                "partial live tail must not create a sub-model-floor owner window"
            );
        }

        #[test]
        fn session_speaker_padding_repeats_short_wake_audio_to_model_floor() {
            let short = vec![900i16; 760 * SAMPLE_RATE as usize / 1000]
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>();
            let speech = session_speaker_speech_window(&short)
                .expect("subsecond active wake must remain eligible for session tracking");
            assert_eq!(speech.len(), short.len());
            let padded = pad_session_speaker_pcm(speech);
            assert_eq!(padded.len(), VERIFICATION_MIN_SPEECH_MS * 32);
            assert_eq!(&padded[..short.len()], short.as_slice());
            assert_eq!(&padded[short.len()..], &short[..240 * 32]);
        }

        #[test]
        fn owner_verification_uses_real_pcm_without_silence_padding() {
            let pcm = vec![32u8; SAMPLE_RATE as usize * 2 * 1_100 / 1000];
            let samples = prepare_embedding_samples(&pcm).expect("1.1 second continuous speech");
            assert_eq!(samples.len(), SAMPLE_RATE as usize * 1_100 / 1000);
            // Product threshold may be looser than platform default for recall.
            assert!(VERIFICATION_THRESHOLD <= DEFAULT_SCORE_MILLI as f32 / 1000.0);
            assert!(VERIFICATION_THRESHOLD >= 0.35);
            let short_pcm = vec![32u8; SAMPLE_RATE as usize * 2 * 955 / 1000];
            assert!(prepare_embedding_samples(&short_pcm).is_err());
        }

        #[test]
        #[ignore = "requires downloaded upstream speaker fixtures"]
        fn runtime_separates_official_same_and_different_speakers() {
            let fixture = |name: &str| {
                let path = std::env::var(name).expect("fixture path env");
                let wav = fs::read(path).expect("read fixture");
                denzic_audio_v1_core::read_wav_pcm16le(&wav).expect("decode 16 kHz mono fixture")
            };
            let runtime = match std::env::var("LISTENER_VOICEPRINT_EVAL_MODEL") {
                Ok(path) => {
                    let root = evaluation_runtime_root();
                    Arc::new(
                        SpeakerRuntime::load_model(&root, Path::new(&path))
                            .expect("load requested evaluation model"),
                    )
                }
                Err(_) => ensure_runtime().expect("load verified runtime"),
            };
            let enrolled = runtime
                .embedding(&fixture("LISTENER_VOICEPRINT_SPEAKER1_A"))
                .expect("speaker1 enrollment embedding");
            let same = runtime
                .embedding(&fixture("LISTENER_VOICEPRINT_SPEAKER1_B"))
                .expect("speaker1 query embedding");
            let different = runtime
                .embedding(&fixture("LISTENER_VOICEPRINT_SPEAKER2_A"))
                .expect("speaker2 query embedding");
            let same_score = cosine(&enrolled, &same).expect("same score");
            let different_score = cosine(&enrolled, &different).expect("different score");
            println!(
                "same_score={same_score:.6} different_score={different_score:.6} threshold={VERIFICATION_THRESHOLD:.3}"
            );
            assert!(same_score >= VERIFICATION_THRESHOLD);
            assert!(different_score < VERIFICATION_THRESHOLD);
            assert!(same_score > different_score);
        }

        #[test]
        #[ignore = "requires the enrolled owner credential and a consented local WAV"]
        fn runtime_reports_enrolled_owner_window_scores() {
            let path = std::env::var("LISTENER_VOICEPRINT_OWNER_WAV")
                .expect("LISTENER_VOICEPRINT_OWNER_WAV path");
            let wav = fs::read(path).expect("read consented owner fixture");
            let pcm =
                denzic_audio_v1_core::read_wav_pcm16le(&wav).expect("decode 16 kHz mono fixture");
            let runtime = ensure_runtime().expect("load verified runtime");
            let template = {
                let mut state = STATE.lock();
                load_template_locked(&mut state);
                state.template.clone().expect("enrolled owner template")
            };
            let raw = runtime.embedding(&pcm).expect("raw embedding");
            let raw_score = template
                .session_embeddings
                .iter()
                .map(|enrolled| cosine(enrolled, &raw).expect("raw score"))
                .max_by(f32::total_cmp)
                .expect("raw score");
            let windows = verification_template_windows(&pcm).expect("verification windows");
            let mut processed_score = f32::NEG_INFINITY;
            for window in &windows {
                let candidate = runtime.embedding(window).expect("window embedding");
                for enrolled in &template.session_embeddings {
                    processed_score =
                        processed_score.max(cosine(enrolled, &candidate).expect("window score"));
                }
            }
            println!(
                "raw_score={raw_score:.6} processed_score={processed_score:.6} enrolled_templates={} candidate_windows={} threshold={VERIFICATION_THRESHOLD:.3}",
                template.session_embeddings.len(),
                windows.len()
            );
        }

        #[derive(Debug, Deserialize)]
        struct EvaluationManifest {
            models: Vec<EvaluationModel>,
            enrollment_session_wavs: Vec<String>,
            samples: Vec<EvaluationSample>,
        }

        #[derive(Debug, Deserialize)]
        struct EvaluationModel {
            name: String,
            path: String,
        }

        #[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        enum EvaluationLabel {
            Owner,
            NonOwner,
        }

        #[derive(Debug, Deserialize)]
        struct EvaluationSample {
            id: String,
            path: String,
            label: EvaluationLabel,
            quality: EvaluationQuality,
        }

        #[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        enum EvaluationQuality {
            Clean,
            Noisy,
            FarField,
        }

        impl EvaluationQuality {
            const ALL: [Self; 3] = [Self::Clean, Self::Noisy, Self::FarField];

            fn name(self) -> &'static str {
                match self {
                    Self::Clean => "clean",
                    Self::Noisy => "noisy",
                    Self::FarField => "far_field",
                }
            }
        }

        #[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        enum EvaluationDuration {
            Short,
            Medium,
            Long,
        }

        impl EvaluationDuration {
            const ALL: [Self; 3] = [Self::Short, Self::Medium, Self::Long];

            fn from_active_speech_ms(active_speech_ms: usize) -> Self {
                match active_speech_ms {
                    ..1_500 => Self::Short,
                    1_500..2_500 => Self::Medium,
                    _ => Self::Long,
                }
            }

            fn name(self) -> &'static str {
                match self {
                    Self::Short => "short_1000_1499_ms",
                    Self::Medium => "medium_1500_2499_ms",
                    Self::Long => "long_2500_plus_ms",
                }
            }
        }

        #[derive(Debug, Serialize)]
        struct EvaluationScore {
            id: String,
            label: EvaluationLabel,
            quality: EvaluationQuality,
            duration: EvaluationDuration,
            active_speech_ms: usize,
            speech_span_ms: usize,
            peak_rms: f32,
            reference_rms: f32,
            score: f32,
            inference_ms: u128,
        }

        #[derive(Debug, Clone, Copy)]
        struct EvaluationMetrics {
            threshold: f32,
            owner_samples: usize,
            non_owner_samples: usize,
            owner_recall: f32,
            non_owner_suppression: f32,
            inference_p95_ms: u128,
        }

        impl EvaluationMetrics {
            fn passes(self) -> bool {
                self.owner_recall >= 0.95
                    && self.non_owner_suppression >= 0.90
                    && self.inference_p95_ms <= 300
            }
        }

        #[derive(Debug, Serialize)]
        struct EvaluationSliceReport {
            slice: String,
            applied_threshold: f32,
            diagnostic_threshold: f32,
            owner_samples: usize,
            non_owner_samples: usize,
            owner_recall: f32,
            non_owner_suppression: f32,
            inference_p95_ms: u128,
            pass: bool,
        }

        #[derive(Debug, Serialize)]
        struct ModelEvaluationReport {
            model: String,
            model_sha256: String,
            threshold: f32,
            owner_samples: usize,
            non_owner_samples: usize,
            owner_recall: f32,
            non_owner_suppression: f32,
            inference_p95_ms: u128,
            duration_slices: Vec<EvaluationSliceReport>,
            quality_slices: Vec<EvaluationSliceReport>,
            pass: bool,
            scores: Vec<EvaluationScore>,
        }

        fn resolve_evaluation_path(base: &Path, value: &str) -> std::path::PathBuf {
            let path = std::path::PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                base.join(path)
            }
        }

        fn evaluation_runtime_root() -> std::path::PathBuf {
            match std::env::var("LISTENER_SPEAKER_EVAL_RUNTIME_ROOT") {
                Ok(path) => std::path::PathBuf::from(path),
                Err(_) => ensure_runtime_assets().expect("prepare speaker runtime assets"),
            }
        }

        fn evaluation_pcm(path: &Path) -> Vec<u8> {
            let wav = fs::read(path).unwrap_or_else(|err| {
                panic!(
                    "read consented evaluation fixture {}: {err}",
                    path.display()
                )
            });
            denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap_or_else(|err| {
                panic!(
                    "decode 16 kHz mono evaluation fixture {}: {err}",
                    path.display()
                )
            })
        }

        fn evaluation_embedding(runtime: &SpeakerRuntime, path: &Path) -> Vec<f32> {
            let pcm = evaluation_pcm(path);
            let speech = session_speaker_speech_window(&pcm).unwrap_or_else(|err| {
                panic!("quality-check evaluation fixture {}: {err}", path.display())
            });
            assert!(
                speech.len() >= VERIFICATION_MIN_SPEECH_MS * 32,
                "evaluation fixture must contain at least {} ms real speech: {}",
                VERIFICATION_MIN_SPEECH_MS,
                path.display()
            );
            runtime
                .embedding(speech)
                .unwrap_or_else(|err| panic!("embed evaluation fixture {}: {err}", path.display()))
        }

        fn evaluation_sample_score(
            runtime: &SpeakerRuntime,
            references: &[Vec<f32>],
            sample: &EvaluationSample,
            path: &Path,
        ) -> EvaluationScore {
            let pcm = evaluation_pcm(path);
            let speech = session_speaker_speech_window(&pcm).unwrap_or_else(|err| {
                panic!("quality-check evaluation fixture {}: {err}", path.display())
            });
            let (active_speech_ms, peak_rms, reference_rms) =
                session_speaker_signal_metrics(speech).unwrap_or_else(|err| {
                    panic!("measure evaluation fixture {}: {err}", path.display())
                });
            assert!(
                active_speech_ms >= VERIFICATION_MIN_SPEECH_MS,
                "evaluation fixture must contain at least {} ms real speech: {}",
                VERIFICATION_MIN_SPEECH_MS,
                path.display()
            );
            let model_pcm = pad_session_speaker_pcm(speech);
            let started = std::time::Instant::now();
            let embedding = runtime
                .embedding(&model_pcm)
                .unwrap_or_else(|err| panic!("embed evaluation fixture {}: {err}", path.display()));
            let inference_ms = started.elapsed().as_millis();
            let score = references
                .iter()
                .map(|reference| cosine(reference, &embedding).expect("matching dimensions"))
                .max_by(f32::total_cmp)
                .expect("enrollment references");
            EvaluationScore {
                id: sample.id.clone(),
                label: sample.label,
                quality: sample.quality,
                duration: EvaluationDuration::from_active_speech_ms(active_speech_ms),
                active_speech_ms,
                speech_span_ms: speech.len() / 32,
                peak_rms,
                reference_rms,
                score,
                inference_ms,
            }
        }

        fn evaluation_metrics(scores: &[&EvaluationScore], threshold: f32) -> EvaluationMetrics {
            let owner_samples = scores
                .iter()
                .filter(|sample| sample.label == EvaluationLabel::Owner)
                .count();
            let non_owner_samples = scores
                .iter()
                .filter(|sample| sample.label == EvaluationLabel::NonOwner)
                .count();
            let accepted_owner = scores
                .iter()
                .filter(|sample| {
                    sample.label == EvaluationLabel::Owner && sample.score >= threshold
                })
                .count();
            let rejected_non_owner = scores
                .iter()
                .filter(|sample| {
                    sample.label == EvaluationLabel::NonOwner && sample.score < threshold
                })
                .count();
            let mut inference_ms = scores
                .iter()
                .map(|sample| sample.inference_ms)
                .collect::<Vec<_>>();
            inference_ms.sort_unstable();
            let inference_p95_ms = if inference_ms.is_empty() {
                0
            } else {
                let index = (inference_ms.len() * 95).div_ceil(100) - 1;
                inference_ms[index]
            };
            EvaluationMetrics {
                threshold,
                owner_samples,
                non_owner_samples,
                owner_recall: if owner_samples == 0 {
                    0.0
                } else {
                    accepted_owner as f32 / owner_samples as f32
                },
                non_owner_suppression: if non_owner_samples == 0 {
                    0.0
                } else {
                    rejected_non_owner as f32 / non_owner_samples as f32
                },
                inference_p95_ms,
            }
        }

        fn calibrated_evaluation_metrics(scores: &[&EvaluationScore]) -> EvaluationMetrics {
            (0..=1_000)
                .map(|step| evaluation_metrics(scores, step as f32 / 1_000.0))
                .max_by(|left, right| {
                    let left_pass = left.owner_recall >= 0.95 && left.non_owner_suppression >= 0.90;
                    let right_pass =
                        right.owner_recall >= 0.95 && right.non_owner_suppression >= 0.90;
                    left_pass
                        .cmp(&right_pass)
                        .then_with(|| {
                            (left.owner_recall + left.non_owner_suppression)
                                .total_cmp(&(right.owner_recall + right.non_owner_suppression))
                        })
                        .then_with(|| left.owner_recall.total_cmp(&right.owner_recall))
                        .then_with(|| right.threshold.total_cmp(&left.threshold))
                })
                .expect("threshold sweep")
        }

        fn evaluation_slice_report(
            name: String,
            scores: &[&EvaluationScore],
            applied_threshold: f32,
        ) -> EvaluationSliceReport {
            let applied = evaluation_metrics(scores, applied_threshold);
            let diagnostic = calibrated_evaluation_metrics(scores);
            let pass =
                applied.owner_samples >= 5 && applied.non_owner_samples >= 5 && applied.passes();
            EvaluationSliceReport {
                slice: name,
                applied_threshold,
                diagnostic_threshold: diagnostic.threshold,
                owner_samples: applied.owner_samples,
                non_owner_samples: applied.non_owner_samples,
                owner_recall: applied.owner_recall,
                non_owner_suppression: applied.non_owner_suppression,
                inference_p95_ms: applied.inference_p95_ms,
                pass,
            }
        }

        fn synthetic_evaluation_score(
            id: &str,
            label: EvaluationLabel,
            score: f32,
            inference_ms: u128,
        ) -> EvaluationScore {
            EvaluationScore {
                id: id.to_string(),
                label,
                quality: EvaluationQuality::Clean,
                duration: EvaluationDuration::Medium,
                active_speech_ms: 2_000,
                speech_span_ms: 2_000,
                peak_rms: 900.0,
                reference_rms: 600.0,
                score,
                inference_ms,
            }
        }

        #[test]
        fn evaluation_duration_buckets_keep_short_and_tail_risk_visible() {
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(1_000),
                EvaluationDuration::Short
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(1_499),
                EvaluationDuration::Short
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(1_500),
                EvaluationDuration::Medium
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(2_499),
                EvaluationDuration::Medium
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(2_500),
                EvaluationDuration::Long
            );
        }

        #[test]
        fn session_speaker_exclusion_uses_active_speech_not_pause_spanning_duration() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1_000;
            let mut samples = vec![900i16; frame_samples * 3];
            samples.extend(vec![0i16; frame_samples * 6]);
            samples.extend(vec![-900i16; frame_samples * 3]);
            let pcm = samples
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>();
            let speech = session_speaker_speech_window(&pcm).expect("speech span");
            assert_eq!(speech.len() / 32, 1_200);
            let (real_speech_ms, _, _) =
                session_speaker_signal_metrics(speech).expect("active speech metrics");
            assert_eq!(real_speech_ms, 600);
            assert!(matches!(
                session_speaker_classification_for_evidence(0.20, real_speech_ms),
                SessionSpeakerClassification::Uncertain { .. }
            ));
        }

        #[test]
        fn evaluation_calibration_prefers_a_threshold_that_meets_both_hard_gates() {
            let mut scores = Vec::new();
            for index in 0..20 {
                scores.push(synthetic_evaluation_score(
                    &format!("owner-{index}"),
                    EvaluationLabel::Owner,
                    if index == 0 { 0.48 } else { 0.70 },
                    (index + 1) as u128,
                ));
                scores.push(synthetic_evaluation_score(
                    &format!("non-owner-{index}"),
                    EvaluationLabel::NonOwner,
                    if index < 2 { 0.52 } else { 0.30 },
                    (index + 1) as u128,
                ));
            }
            let refs = scores.iter().collect::<Vec<_>>();
            let metrics = calibrated_evaluation_metrics(&refs);
            assert!(metrics.threshold > 0.52 && metrics.threshold <= 0.70);
            assert_eq!(metrics.owner_recall, 0.95);
            assert_eq!(metrics.non_owner_suppression, 1.0);
            assert_eq!(metrics.inference_p95_ms, 19);
            assert!(metrics.passes());
        }

        #[test]
        fn evaluation_slice_requires_five_samples_per_label() {
            let scores = vec![
                synthetic_evaluation_score("owner", EvaluationLabel::Owner, 0.8, 10),
                synthetic_evaluation_score("non-owner", EvaluationLabel::NonOwner, 0.2, 10),
            ];
            let refs = scores.iter().collect::<Vec<_>>();
            let report = evaluation_slice_report("quality:clean".to_string(), &refs, 0.5);
            assert_eq!(report.owner_recall, 1.0);
            assert_eq!(report.non_owner_suppression, 1.0);
            assert!(!report.pass);
        }

        #[test]
        #[ignore = "requires a consented, owner-labeled Listener speaker corpus"]
        fn runtime_evaluates_listener_labeled_speaker_corpus() {
            let manifest_path = std::path::PathBuf::from(
                std::env::var("LISTENER_SPEAKER_EVAL_MANIFEST")
                    .expect("LISTENER_SPEAKER_EVAL_MANIFEST path"),
            );
            let output_path = std::path::PathBuf::from(
                std::env::var("LISTENER_SPEAKER_EVAL_OUTPUT")
                    .expect("LISTENER_SPEAKER_EVAL_OUTPUT path"),
            );
            let manifest: EvaluationManifest = serde_json::from_slice(
                &fs::read(&manifest_path).expect("read speaker evaluation manifest"),
            )
            .expect("parse speaker evaluation manifest");
            let base = manifest_path.parent().unwrap_or_else(|| Path::new("."));
            assert!(
                manifest.enrollment_session_wavs.len() >= DUAL_TEMPLATE_WINDOWS_PER_BANK,
                "evaluation requires at least three consented free-speech enrollment references"
            );
            let owner_samples = manifest
                .samples
                .iter()
                .filter(|sample| sample.label == EvaluationLabel::Owner)
                .count();
            let non_owner_samples = manifest.samples.len().saturating_sub(owner_samples);
            assert!(
                owner_samples >= 20,
                "evaluation requires at least 20 owner samples"
            );
            assert!(
                non_owner_samples >= 20,
                "evaluation requires at least 20 non-owner samples"
            );

            let runtime_root = evaluation_runtime_root();
            let mut reports = Vec::new();
            for model in &manifest.models {
                let model_path = resolve_evaluation_path(base, &model.path);
                let runtime = SpeakerRuntime::load_model(&runtime_root, &model_path)
                    .unwrap_or_else(|err| panic!("load evaluation model {}: {err}", model.name));
                let references = manifest
                    .enrollment_session_wavs
                    .iter()
                    .map(|path| {
                        evaluation_embedding(&runtime, &resolve_evaluation_path(base, path))
                    })
                    .collect::<Vec<_>>();
                let scores = manifest
                    .samples
                    .iter()
                    .map(|sample| {
                        evaluation_sample_score(
                            &runtime,
                            &references,
                            sample,
                            &resolve_evaluation_path(base, &sample.path),
                        )
                    })
                    .collect::<Vec<_>>();
                let all_scores = scores.iter().collect::<Vec<_>>();
                let overall = calibrated_evaluation_metrics(&all_scores);
                let duration_slices = EvaluationDuration::ALL
                    .into_iter()
                    .map(|duration| {
                        let slice = scores
                            .iter()
                            .filter(|sample| sample.duration == duration)
                            .collect::<Vec<_>>();
                        evaluation_slice_report(
                            format!("duration:{}", duration.name()),
                            &slice,
                            overall.threshold,
                        )
                    })
                    .collect::<Vec<_>>();
                let quality_slices = EvaluationQuality::ALL
                    .into_iter()
                    .map(|quality| {
                        let slice = scores
                            .iter()
                            .filter(|sample| sample.quality == quality)
                            .collect::<Vec<_>>();
                        evaluation_slice_report(
                            format!("quality:{}", quality.name()),
                            &slice,
                            overall.threshold,
                        )
                    })
                    .collect::<Vec<_>>();
                let pass = overall.owner_samples == owner_samples
                    && overall.non_owner_samples == non_owner_samples
                    && overall.passes()
                    && duration_slices.iter().all(|slice| slice.pass)
                    && quality_slices.iter().all(|slice| slice.pass);
                reports.push(ModelEvaluationReport {
                    model: model.name.clone(),
                    model_sha256: sha256(&model_path).expect("hash evaluation model"),
                    threshold: overall.threshold,
                    owner_samples: overall.owner_samples,
                    non_owner_samples: overall.non_owner_samples,
                    owner_recall: overall.owner_recall,
                    non_owner_suppression: overall.non_owner_suppression,
                    inference_p95_ms: overall.inference_p95_ms,
                    duration_slices,
                    quality_slices,
                    pass,
                    scores,
                });
            }
            assert!(
                !reports.is_empty(),
                "evaluation manifest contains no models"
            );
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).expect("create speaker evaluation output directory");
            }
            fs::write(
                &output_path,
                serde_json::to_vec_pretty(&reports).expect("encode speaker evaluation report"),
            )
            .expect("write speaker evaluation report");
            assert!(
                reports.iter().any(|report| report.pass),
                "no speaker model meets owner recall, non-owner suppression, and latency gates"
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use platform::prepare_runtime_assets;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
pub(crate) use platform::target_speaker_embedding_for_phrase;
#[cfg(target_os = "windows")]
pub use platform::{
    begin_enrollment_processing, cancel_enrollment, delete_template, enrollment_should_process,
    fail_enrollment, finish_enrollment, invalidate_for_phrase_change, is_enrolled_for_phrase,
    observe_enrollment_capture, observe_session_speaker, prepare_for_phrase,
    session_profile_from_wake, start_enrollment, status_for_phrase, take_enrollment_arm, verify,
};

#[cfg(not(target_os = "windows"))]
pub fn status() -> VoiceprintStatus {
    VoiceprintStatus {
        available: false,
        runtime_ready: false,
        model_ready: false,
        enrolled: false,
        enrolled_phrase: None,
        requires_reenrollment: false,
        state: "unavailable".into(),
        progress: 0,
        capture_seconds_remaining: None,
        capture_step: None,
        capture_step_count: 3,
        capture_elapsed_ms: 0,
        step_speech_ms: vec![0; 3],
        signal_level: 0,
        capture_feedback: None,
        score: None,
        threshold: 0.5,
        error: None,
        model_name: "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
        runtime_version: "1.13.1",
        local_only: true,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_enrolled_for_phrase(_wake_phrase: &str) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn take_enrollment_arm() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn begin_enrollment_processing() {}

#[cfg(not(target_os = "windows"))]
pub fn observe_enrollment_capture(_pcm: &[u8]) {}

#[cfg(not(target_os = "windows"))]
pub fn enrollment_should_process() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn start_enrollment(_wake_phrase: &str) -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn cancel_enrollment(_wake_phrase: &str) -> Result<VoiceprintStatus, String> {
    Ok(status())
}

#[cfg(not(target_os = "windows"))]
pub fn delete_template() -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn finish_enrollment(_pcm: &[u8], _wake_phrase: &str) -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn fail_enrollment(_error: &str) {}

#[cfg(not(target_os = "windows"))]
pub fn verify(_pcm: &[u8], _wake_phrase: &str) -> Result<VerificationResult, String> {
    Err("voiceprint verification is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn session_profile_from_wake(
    _pcm: &[u8],
    _wake_end_seconds: f32,
    _wake_phrase: &str,
) -> Result<SessionSpeakerProfile, String> {
    Err("session speaker tracking is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn classify_session_speaker(
    _profile: &SessionSpeakerProfile,
    _pcm: &[u8],
) -> Result<SessionSpeakerClassification, String> {
    Err("session speaker tracking is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn observe_session_speaker(
    _profile: &SessionSpeakerProfile,
    _pcm: &[u8],
) -> Result<SessionSpeakerObservation, String> {
    Err("session speaker tracking is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn prepare_for_phrase(_wake_phrase: &str) -> Result<(), String> {
    Err("voiceprint verification is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn status_for_phrase(_wake_phrase: &str) -> VoiceprintStatus {
    status()
}

#[cfg(not(target_os = "windows"))]
pub fn invalidate_for_phrase_change(
    _previous_phrase: &str,
    _next_phrase: &str,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn prepare_runtime_assets() -> Result<(), String> {
    Err("voiceprint verification is currently available on Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        classification: SessionSpeakerClassification,
        real_speech_ms: usize,
        embedding: [f32; 2],
    ) -> SessionSpeakerObservation {
        SessionSpeakerObservation {
            classification,
            real_speech_ms,
            transcript_hard_non_target: false,
            embedding: embedding.to_vec(),
        }
    }

    #[test]
    fn public_status_has_explicit_local_privacy_contract() {
        assert!(status_for_phrase("开始录音").local_only);
        assert!(status_for_phrase("开始录音").threshold > 0.0);
    }

    #[test]
    fn session_speaker_score_has_a_mixed_speech_uncertainty_band() {
        assert!(matches!(
            session_speaker_classification_for_score(0.55),
            SessionSpeakerClassification::Target { .. }
        ));
        assert!(matches!(
            session_speaker_classification_for_score(0.20),
            SessionSpeakerClassification::NonTarget { .. }
        ));
        for score in [0.294403, 0.307526, 0.331096, 0.336520] {
            assert!(matches!(
                session_speaker_classification_for_score(score),
                SessionSpeakerClassification::Uncertain { .. }
            ));
        }
        assert!(matches!(
            session_speaker_classification_for_score(0.38),
            SessionSpeakerClassification::Uncertain { .. }
        ));
    }

    #[test]
    fn weak_target_band_is_uncertain_not_target() {
        // F3 fixture（2026-08-09 12:47:04）：第二个人的声音得分 0.426–0.58。
        // [0.42, 0.55) 弱 Target 带必须判 Uncertain（不刷新端点时钟、不冻结），
        // ≥0.55 才是确信 Target。
        for score in [0.42, 0.426, 0.47, 0.50, 0.54] {
            assert!(
                matches!(
                    session_speaker_classification_for_score(score),
                    SessionSpeakerClassification::Uncertain { .. }
                ),
                "score {score} must be Uncertain (weak-target band)"
            );
        }
        for score in [0.55, 0.58, 0.72] {
            assert!(
                matches!(
                    session_speaker_classification_for_score(score),
                    SessionSpeakerClassification::Target { .. }
                ),
                "score {score} must be confident Target"
            );
        }
        assert!(matches!(
            session_speaker_classification_for_score(0.41),
            SessionSpeakerClassification::Uncertain { .. }
        ));
        assert!(matches!(
            session_speaker_classification_for_score(0.34),
            SessionSpeakerClassification::Uncertain { .. }
        ));
    }

    #[test]
    fn short_body_speech_cannot_be_confidently_excluded_after_repeat_padding() {
        assert!(matches!(
            session_speaker_classification_for_evidence(0.20, 900),
            SessionSpeakerClassification::Uncertain { score } if score == 0.20
        ));
        assert!(matches!(
            session_speaker_classification_for_evidence(0.20, 1_000),
            SessionSpeakerClassification::NonTarget { score } if score == 0.20
        ));
        assert!(matches!(
            session_speaker_classification_for_evidence(0.55, 700),
            SessionSpeakerClassification::Target { score } if score == 0.55
        ));
    }

    #[test]
    fn flat_idle_noise_cannot_become_session_identity_evidence() {
        // Installed session b6c7bd31 ended with two flat idle windows at
        // peak_rms=70.7. The relative VAD counted all 1,200 ms as active and
        // their low cosine scores became NonTarget, cutting the transcript.
        for score in [0.176_024_97, 0.179_590_54, 0.80] {
            assert!(matches!(
                session_speaker_classification_for_signal(score, 1_200, 70.7),
                SessionSpeakerClassification::Uncertain { score: actual }
                    if actual == score
            ));
        }
        assert!(matches!(
            session_speaker_classification_for_signal(0.18, 1_200, 1_500.0),
            SessionSpeakerClassification::NonTarget { score } if score == 0.18
        ));
        assert!(matches!(
            session_speaker_classification_for_signal(0.60, 1_200, 1_500.0),
            SessionSpeakerClassification::Target { score } if score == 0.60
        ));
    }

    #[test]
    fn short_high_energy_extreme_mismatch_is_transcript_only_evidence() {
        assert!(session_speaker_transcript_hard_non_target(
            0.077, 600, 771.7
        ));
        assert!(!session_speaker_transcript_hard_non_target(
            0.077, 599, 771.7
        ));
        assert!(!session_speaker_transcript_hard_non_target(
            0.077, 800, 70.7
        ));
        assert!(!session_speaker_transcript_hard_non_target(
            0.172, 800, 771.7
        ));
    }

    #[test]
    fn enrolled_session_adds_one_verified_wake_exemplar_without_becoming_adaptive() {
        let mut embeddings = vec![vec![1.0, 0.0], vec![0.9, 0.1], vec![0.8, 0.2]];
        assert!(append_verified_wake_session_exemplar(
            &mut embeddings,
            vec![0.7, 0.3],
        ));
        assert_eq!(embeddings.len(), SESSION_SPEAKER_MAX_EMBEDDINGS);
        assert_eq!(embeddings.last(), Some(&vec![0.7, 0.3]));

        assert!(
            !append_verified_wake_session_exemplar(&mut embeddings, vec![0.6, 0.4]),
            "the session bank must stay bounded"
        );
        let profile = SessionSpeakerProfile {
            embeddings: Arc::new(embeddings),
            adaptive: false,
        };
        assert!(!profile.is_adaptive());
    }

    #[test]
    fn verified_wake_exemplar_rejects_empty_or_wrong_dimension_banks() {
        let mut empty = Vec::new();
        assert!(!append_verified_wake_session_exemplar(
            &mut empty,
            vec![1.0, 0.0],
        ));

        let mut enrolled = vec![vec![1.0, 0.0]];
        assert!(!append_verified_wake_session_exemplar(
            &mut enrolled,
            vec![1.0],
        ));
        assert_eq!(enrolled.len(), 1);
    }

    #[test]
    fn session_profile_adapts_only_after_consecutive_high_confidence_and_stable_coverage() {
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        gate.note(
            2_600,
            observation(
                SessionSpeakerClassification::Target { score: 0.52 },
                1_000,
                [0.9, 0.1],
            ),
        );
        assert_eq!(gate.promote_covered(&mut profile, Some(3_000)), 0);
        assert_eq!(profile.embeddings.len(), 1);

        gate.note(
            3_000,
            observation(
                SessionSpeakerClassification::Target { score: 0.51 },
                1_200,
                [0.8, 0.2],
            ),
        );
        assert_eq!(gate.promote_covered(&mut profile, Some(2_800)), 1);
        assert_eq!(profile.embeddings.len(), 2);
        assert_eq!(gate.promote_covered(&mut profile, Some(3_000)), 1);
        assert_eq!(profile.embeddings.len(), 3);
    }

    #[test]
    fn unenrolled_session_bootstraps_from_three_consecutive_full_target_windows_without_cloud_coverage(
    ) {
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        for (audio_end_ms, score, embedding) in [
            (3_000, 0.483, [0.90, 0.10]),
            (3_400, 0.458, [0.85, 0.15]),
            (3_800, 0.435, [0.80, 0.20]),
        ] {
            gate.note(
                audio_end_ms,
                observation(
                    SessionSpeakerClassification::Target { score },
                    1_200,
                    embedding,
                ),
            );
        }

        assert_eq!(gate.promote_covered(&mut profile, None), 3);
        assert_eq!(profile.embeddings.len(), 4);
        assert_eq!(gate.promote_covered(&mut profile, None), 0);
    }

    #[test]
    fn uncertain_or_confirmed_non_target_prevents_local_body_bootstrap() {
        let target = |score, embedding| {
            observation(
                SessionSpeakerClassification::Target { score },
                1_200,
                embedding,
            )
        };
        let mut interrupted_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut interrupted = SessionSpeakerAdaptationGate::default();
        interrupted.note(3_000, target(0.48, [0.90, 0.10]));
        interrupted.note(
            3_400,
            observation(
                SessionSpeakerClassification::Uncertain { score: 0.38 },
                1_200,
                [0.75, 0.25],
            ),
        );
        interrupted.note(3_800, target(0.47, [0.88, 0.12]));
        interrupted.note(4_200, target(0.46, [0.86, 0.14]));
        assert_eq!(
            interrupted.promote_covered(&mut interrupted_profile, None),
            0
        );

        let mut switched_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut switched = SessionSpeakerAdaptationGate::default();
        for audio_end_ms in [3_000, 3_400] {
            switched.note(
                audio_end_ms,
                observation(
                    SessionSpeakerClassification::NonTarget { score: 0.20 },
                    1_200,
                    [0.0, 1.0],
                ),
            );
        }
        for (index, audio_end_ms) in [3_800, 4_200, 4_600].into_iter().enumerate() {
            switched.note(audio_end_ms, target(0.48 - index as f32 * 0.01, [0.9, 0.1]));
        }
        assert_eq!(switched.promote_covered(&mut switched_profile, None), 0);

        let mut enrolled_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: false,
        };
        let mut enrolled = SessionSpeakerAdaptationGate::default();
        for audio_end_ms in [3_000, 3_400, 3_800] {
            enrolled.note(audio_end_ms, target(0.60, [0.9, 0.1]));
        }
        assert_eq!(enrolled.promote_covered(&mut enrolled_profile, None), 0);
        assert_eq!(enrolled_profile.embeddings.len(), 1);
    }

    #[test]
    fn short_target_window_does_not_break_body_bootstrap_streak() {
        // 2026-08-07 07:23 「你看一」事故：800/600ms 的 Target 短窗把 bootstrap
        // 连击清零，body exemplar 从未建立，后段声纹漂移冻结在 8 字。
        // 短 Target 窗不算一格，但不得清空连击（LST-REC-025 只让
        // Uncertain/NonTarget 打断）。
        let target_full = |score, embedding| {
            observation(
                SessionSpeakerClassification::Target { score },
                1_200,
                embedding,
            )
        };
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        gate.note(3_000, target_full(0.48, [0.90, 0.10]));
        // 短 Target 窗（不足 1000ms）插在全窗之间：连击必须保留。
        gate.note(
            3_200,
            observation(
                SessionSpeakerClassification::Target { score: 0.47 },
                600,
                [0.88, 0.12],
            ),
        );
        gate.note(3_400, target_full(0.46, [0.86, 0.14]));
        gate.note(3_800, target_full(0.45, [0.84, 0.16]));
        assert_eq!(gate.promote_covered(&mut profile, None), 3);
        assert_eq!(profile.embeddings.len(), 4);
    }

    #[test]
    fn uncertain_low_short_and_enrolled_observations_cannot_pollute_profiles() {
        let mut adaptive_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        gate.note(
            1_000,
            observation(
                SessionSpeakerClassification::Target { score: 0.60 },
                900,
                [0.9, 0.1],
            ),
        );
        gate.note(
            1_400,
            observation(
                SessionSpeakerClassification::Uncertain { score: 0.38 },
                1_200,
                [0.8, 0.2],
            ),
        );
        gate.note(
            1_800,
            observation(
                SessionSpeakerClassification::Target { score: 0.49 },
                1_200,
                [0.7, 0.3],
            ),
        );
        gate.note(
            2_200,
            observation(
                SessionSpeakerClassification::NonTarget { score: 0.20 },
                1_200,
                [0.0, 1.0],
            ),
        );
        assert_eq!(gate.promote_covered(&mut adaptive_profile, Some(3_000)), 0);
        assert_eq!(adaptive_profile.embeddings.len(), 1);

        let mut enrolled_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: false,
        };
        let candidates = [observation(
            SessionSpeakerClassification::Target { score: 0.90 },
            1_200,
            [0.9, 0.1],
        )];
        assert_eq!(
            adapt_session_speaker_profile(&mut enrolled_profile, &candidates),
            0
        );
        assert_eq!(enrolled_profile.embeddings.len(), 1);
    }

    #[test]
    fn session_profile_adaptation_is_bounded_to_three_body_exemplars() {
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let candidates = (0..8)
            .map(|index| {
                observation(
                    SessionSpeakerClassification::Target { score: 0.75 },
                    1_200,
                    [0.9 - index as f32 * 0.01, 0.1 + index as f32 * 0.01],
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(adapt_session_speaker_profile(&mut profile, &candidates), 3);
        assert_eq!(profile.embeddings.len(), 4);
        assert_eq!(adapt_session_speaker_profile(&mut profile, &candidates), 0);
    }
}
