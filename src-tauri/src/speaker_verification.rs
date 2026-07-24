use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceprintStatus {
    pub available: bool,
    pub runtime_ready: bool,
    pub model_ready: bool,
    pub enrolled: bool,
    pub state: String,
    pub progress: u8,
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

#[cfg(target_os = "windows")]
mod platform {
    use super::{VerificationResult, VoiceprintStatus};
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use denzic_speaker_verification_v1_core::{
        Action, CandidateOrigin, Input, Machine, Verdict, DEFAULT_MAX_CANDIDATE_MS,
        DEFAULT_SCORE_MILLI,
    };
    use libloading::Library;
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::ffi::{c_char, c_void, CString};
    use std::fs;
    use std::io::{BufReader, Read};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    const RUNTIME_VERSION: &str = "1.13.1";
    const MODEL_NAME: &str = "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";
    const MODEL_SHA256: &str = "F682B514C05D947EE3FA91CD6EC6C5C7543479A128373FA29B1FAEDCCD21FD11";
    const RUNTIME_ARCHIVE_SHA256: &str =
        "6760B0E25EAAD0DADFFBA9029B1270778E0DBFD43F314CF070B2F9C1DCB4AF25";
    const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";
    const RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.1/sherpa-onnx-v1.13.1-win-x64-shared-MD-Release-no-tts.tar.bz2";
    const KEYRING_SERVICE: &str = "com.listener.type.voiceprint";
    const KEYRING_ACCOUNT: &str = "owner-template-v1";
    const SAMPLE_RATE: i32 = 16_000;
    const ENROLLMENT_SECONDS: u64 = 7;
    const ENROLLMENT_SEGMENTS: usize = 3;
    const ENROLLMENT_MIN_PAIR_SCORE: f32 = 0.45;
    const VERIFICATION_THRESHOLD: f32 = DEFAULT_SCORE_MILLI as f32 / 1000.0;

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
        runtime: Option<Arc<SpeakerRuntime>>,
    }

    static STATE: Lazy<Mutex<State>> = Lazy::new(|| Mutex::new(State::default()));

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct StoredTemplate {
        version: u8,
        model_sha256: String,
        dimension: usize,
        embedding_base64: String,
    }

    #[derive(Debug, Clone)]
    struct SpeakerTemplate {
        embedding: Vec<f32>,
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
            let model_path = root.join(MODEL_NAME);
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
            let samples = pcm
                .chunks_exact(2)
                .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
                .collect::<Vec<_>>();
            if samples.len() < SAMPLE_RATE as usize {
                return Err("voiceprint audio is shorter than one second".to_string());
            }
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

    fn ensure_runtime() -> Result<Arc<SpeakerRuntime>, String> {
        if let Some(runtime) = STATE.lock().runtime.clone() {
            return Ok(runtime);
        }
        let root = crate::persistence::speaker_verification_root()
            .map_err(|err| format!("create voiceprint model directory failed: {err}"))?;
        let archive_path = root.join(format!("sherpa-onnx-{RUNTIME_VERSION}.tar.bz2"));
        download_verified(RUNTIME_URL, &archive_path, RUNTIME_ARCHIVE_SHA256)?;
        download_verified(MODEL_URL, &root.join(MODEL_NAME), MODEL_SHA256)?;
        extract_runtime(&root, &archive_path)?;
        let runtime = Arc::new(SpeakerRuntime::load(&root)?);
        STATE.lock().runtime = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    fn keyring_entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
            .map_err(|err| format!("open system voiceprint credential failed: {err}"))
    }

    fn encode_template(embedding: &[f32]) -> Result<String, String> {
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for value in embedding {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        serde_json::to_string(&StoredTemplate {
            version: 1,
            model_sha256: MODEL_SHA256.to_string(),
            dimension: embedding.len(),
            embedding_base64: BASE64.encode(bytes),
        })
        .map_err(|err| format!("encode voiceprint template failed: {err}"))
    }

    fn decode_template(value: &str) -> Result<SpeakerTemplate, String> {
        let stored: StoredTemplate = serde_json::from_str(value)
            .map_err(|err| format!("voiceprint template damaged: {err}"))?;
        if stored.version != 1 || stored.model_sha256 != MODEL_SHA256 {
            return Err("voiceprint template is incompatible with the current model".to_string());
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
        normalize(&mut embedding)?;
        Ok(SpeakerTemplate { embedding })
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
                Ok(template) => state.template = Some(template),
                Err(err) => state.error = Some(err),
            },
            Ok(None) => {}
            Err(err) => state.error = Some(err),
        }
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

    pub fn status() -> VoiceprintStatus {
        let (runtime_ready, model_ready) = assets_ready();
        let mut state = STATE.lock();
        load_template_locked(&mut state);
        VoiceprintStatus {
            available: true,
            runtime_ready,
            model_ready,
            enrolled: state.template.is_some(),
            state: state
                .capture
                .unwrap_or(CaptureState::Idle)
                .label()
                .to_string(),
            progress: state.progress,
            score: state.last_score,
            threshold: VERIFICATION_THRESHOLD,
            error: state.error.clone(),
            model_name: MODEL_NAME,
            runtime_version: RUNTIME_VERSION,
            local_only: true,
        }
    }

    pub fn is_enrolled() -> bool {
        let mut state = STATE.lock();
        load_template_locked(&mut state);
        state.template.is_some()
    }

    pub fn start_enrollment() -> Result<VoiceprintStatus, String> {
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
        if let Err(err) = crate::embedded_ble::send_recording_control_toggle(Duration::from_secs(4))
        {
            mark_error(&err);
            return Err(err);
        }
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(ENROLLMENT_SECONDS));
            if let Err(err) =
                crate::embedded_ble::send_recording_control_stop(Duration::from_secs(4))
            {
                mark_error(&format!("stop voiceprint enrollment failed: {err}"));
            }
        });
        Ok(status())
    }

    pub fn take_enrollment_arm() -> bool {
        let mut state = STATE.lock();
        if state.capture == Some(CaptureState::Armed) {
            state.capture = Some(CaptureState::Capturing);
            state.progress = 35;
            true
        } else {
            false
        }
    }

    pub fn finish_enrollment(pcm: &[u8]) -> Result<VoiceprintStatus, String> {
        {
            let mut state = STATE.lock();
            state.capture = Some(CaptureState::Processing);
            state.progress = 70;
        }
        let result = (|| {
            let runtime = ensure_runtime()?;
            let sample_count = pcm.len() / 2;
            let segment_samples = sample_count / ENROLLMENT_SEGMENTS;
            if segment_samples < SAMPLE_RATE as usize {
                return Err(
                    "voiceprint enrollment needs at least 3 seconds of clear speech".to_string(),
                );
            }
            let mut embeddings = Vec::with_capacity(ENROLLMENT_SEGMENTS);
            for index in 0..ENROLLMENT_SEGMENTS {
                let start = index * segment_samples * 2;
                let end = if index + 1 == ENROLLMENT_SEGMENTS {
                    pcm.len()
                } else {
                    (index + 1) * segment_samples * 2
                };
                embeddings.push(runtime.embedding(&pcm[start..end])?);
            }
            let mut min_pair_score = 1.0f32;
            for left in 0..embeddings.len() {
                for right in (left + 1)..embeddings.len() {
                    min_pair_score =
                        min_pair_score.min(cosine(&embeddings[left], &embeddings[right])?);
                }
            }
            if min_pair_score < ENROLLMENT_MIN_PAIR_SCORE {
                return Err(format!(
                    "voiceprint segments are inconsistent; retry with one speaker in a quiet room (score {min_pair_score:.3})"
                ));
            }
            let mut average = vec![0.0f32; embeddings[0].len()];
            for embedding in &embeddings {
                for (target, value) in average.iter_mut().zip(embedding) {
                    *target += *value;
                }
            }
            normalize(&mut average)?;
            let encoded = encode_template(&average)?;
            keyring_entry()?
                .set_password(&encoded)
                .map_err(|err| format!("save system voiceprint credential failed: {err}"))?;
            {
                let mut state = STATE.lock();
                state.template = Some(SpeakerTemplate { embedding: average });
                state.template_checked = true;
                state.capture = Some(CaptureState::Complete);
                state.progress = 100;
                state.last_score = Some(min_pair_score);
                state.error = None;
            }
            Ok(status())
        })();
        if let Err(err) = &result {
            mark_error(err);
        }
        result
    }

    pub fn verify(pcm: &[u8]) -> Result<VerificationResult, String> {
        let runtime = ensure_runtime()?;
        let template = {
            let mut state = STATE.lock();
            load_template_locked(&mut state);
            state
                .template
                .clone()
                .ok_or_else(|| "voiceprint is not enrolled".to_string())?
        };
        let max_bytes = DEFAULT_MAX_CANDIDATE_MS as usize * 32;
        let candidate = &pcm[..pcm.len().min(max_bytes)];
        let embedding = runtime.embedding(candidate)?;
        let score = cosine(&template.embedding, &embedding)?;
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
            candidate_ms: (candidate.len() / 32) as u32,
            verdict,
        });
        STATE.lock().last_score = Some(score);
        Ok(VerificationResult {
            matched: matches!(decision.action, Action::Release),
            score,
        })
    }

    pub fn delete_template() -> Result<VoiceprintStatus, String> {
        let entry = keyring_entry()?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => return Err(format!("delete system voiceprint credential failed: {err}")),
        }
        let mut state = STATE.lock();
        state.template = None;
        state.template_checked = true;
        state.capture = Some(CaptureState::Idle);
        state.progress = 0;
        state.last_score = None;
        state.error = None;
        drop(state);
        Ok(status())
    }

    fn mark_error(error: &str) {
        let mut state = STATE.lock();
        state.capture = Some(CaptureState::Error);
        state.error = Some(error.to_string());
    }

    pub fn fail_enrollment(error: &str) {
        mark_error(error);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn template_binary_round_trip_is_normalized_and_compact() {
            let mut embedding = (1..=192).map(|value| value as f32).collect::<Vec<_>>();
            normalize(&mut embedding).unwrap();
            let encoded = encode_template(&embedding).unwrap();
            assert!(encoded.len() < 1800);
            let decoded = decode_template(&encoded).unwrap();
            assert!((cosine(&embedding, &decoded.embedding).unwrap() - 1.0).abs() < 1e-5);
        }

        #[test]
        fn corrupt_or_wrong_model_template_is_rejected() {
            assert!(decode_template("{}").is_err());
            let wrong = serde_json::to_string(&StoredTemplate {
                version: 1,
                model_sha256: "wrong".into(),
                dimension: 1,
                embedding_base64: BASE64.encode(0.5f32.to_le_bytes()),
            })
            .unwrap();
            assert!(decode_template(&wrong).is_err());
        }

        #[test]
        fn cosine_separates_aligned_and_opposed_vectors() {
            assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]).unwrap(), 1.0);
            assert_eq!(cosine(&[1.0, 0.0], &[-1.0, 0.0]).unwrap(), -1.0);
        }

        #[test]
        #[ignore = "requires downloaded upstream speaker fixtures"]
        fn runtime_separates_official_same_and_different_speakers() {
            let fixture = |name: &str| {
                let path = std::env::var(name).expect("fixture path env");
                let wav = fs::read(path).expect("read fixture");
                denzic_audio_v1_core::read_wav_pcm16le(&wav).expect("decode 16 kHz mono fixture")
            };
            let runtime = ensure_runtime().expect("load verified runtime");
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
    }
}

#[cfg(target_os = "windows")]
pub use platform::{
    delete_template, fail_enrollment, finish_enrollment, is_enrolled, start_enrollment, status,
    take_enrollment_arm, verify,
};

#[cfg(not(target_os = "windows"))]
pub fn status() -> VoiceprintStatus {
    VoiceprintStatus {
        available: false,
        runtime_ready: false,
        model_ready: false,
        enrolled: false,
        state: "unavailable".into(),
        progress: 0,
        score: None,
        threshold: 0.5,
        error: None,
        model_name: "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
        runtime_version: "1.13.1",
        local_only: true,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_enrolled() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn take_enrollment_arm() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn start_enrollment() -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn delete_template() -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn finish_enrollment(_pcm: &[u8]) -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn fail_enrollment(_error: &str) {}

#[cfg(not(target_os = "windows"))]
pub fn verify(_pcm: &[u8]) -> Result<VerificationResult, String> {
    Err("voiceprint verification is currently available on Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_status_has_explicit_local_privacy_contract() {
        assert!(status().local_only);
        assert!(status().threshold > 0.0);
    }
}
