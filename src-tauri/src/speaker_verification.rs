use serde::Serialize;

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
    const KEYRING_SUPPLEMENTAL_ACCOUNT_PREFIX: &str = "owner-template-v2-";
    const MAX_SUPPLEMENTAL_TEMPLATES: usize = 3;
    const SAMPLE_RATE: i32 = 16_000;
    const VERIFICATION_MIN_SPEECH_MS: usize = 1_000;
    const ENROLLMENT_SECONDS: u64 = 7;
    const ENROLLMENT_MIN_SECONDS: usize = 3;
    const ENROLLMENT_FRAME_MS: usize = 100;
    const ENROLLMENT_MIN_ACTIVE_FRAMES: usize = 18;
    const TEMPLATE_WINDOW_MS: usize = 1_600;
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
        runtime: Option<Arc<SpeakerRuntime>>,
    }

    static STATE: Lazy<Mutex<State>> = Lazy::new(|| Mutex::new(State::default()));

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
    }

    #[derive(Debug, Clone)]
    struct SpeakerTemplate {
        embeddings: Vec<Vec<f32>>,
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

    fn enrollment_template_windows(pcm: &[u8]) -> Result<Vec<&[u8]>, String> {
        let speech = enrollment_speech_window(pcm)?;
        let mut windows = Vec::with_capacity(MAX_SUPPLEMENTAL_TEMPLATES + 1);
        windows.push(speech);
        for window in evenly_spaced_windows(speech, TEMPLATE_WINDOW_MS, MAX_SUPPLEMENTAL_TEMPLATES)
        {
            if window.len() >= SAMPLE_RATE as usize * 2 * VERIFICATION_MIN_SPEECH_MS / 1000 {
                windows.push(window);
            }
        }
        windows.truncate(MAX_SUPPLEMENTAL_TEMPLATES + 1);
        Ok(windows)
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
        let minimum_frames = VERIFICATION_MIN_SPEECH_MS.div_ceil(ENROLLMENT_FRAME_MS);
        while last.saturating_sub(first) < minimum_frames {
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
    ) -> Result<String, String> {
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for value in embedding {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        serde_json::to_string(&StoredTemplate {
            version: 2,
            model_sha256: MODEL_SHA256.to_string(),
            dimension: embedding.len(),
            embedding_base64: BASE64.encode(bytes),
            phrase: Some(phrase.to_string()),
            invalidated,
        })
        .map_err(|err| format!("encode voiceprint template failed: {err}"))
    }

    fn decode_template(value: &str) -> Result<SpeakerTemplate, String> {
        let stored: StoredTemplate = serde_json::from_str(value)
            .map_err(|err| format!("voiceprint template damaged: {err}"))?;
        if !matches!(stored.version, 1 | 2) || stored.model_sha256 != MODEL_SHA256 {
            return Err("voiceprint template is incompatible with the current model".to_string());
        }
        if stored.version == 2
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
        normalize(&mut embedding)?;
        Ok(SpeakerTemplate {
            embeddings: vec![embedding],
            phrase: stored.phrase,
            invalidated: stored.invalidated,
        })
    }

    fn persist_template(template: &SpeakerTemplate) -> Result<(), String> {
        let phrase = template
            .phrase
            .as_deref()
            .ok_or_else(|| "voiceprint template wake phrase is missing".to_string())?;
        let encoded = template
            .embeddings
            .iter()
            .map(|embedding| encode_template(embedding, phrase, template.invalidated))
            .collect::<Result<Vec<_>, _>>()?;
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
                                    template.embeddings.extend(extra.embeddings)
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
        let requires_reenrollment = state.template.is_some() && !enrolled;
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
            progress: state.progress,
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
        Ok(status_for_phrase(&wake_phrase))
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

    pub fn finish_enrollment(pcm: &[u8], wake_phrase: &str) -> Result<VoiceprintStatus, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        {
            let mut state = STATE.lock();
            if state.enrollment_phrase.as_deref() != Some(phrase.as_str()) {
                return Err("录制期间唤醒词已改变，请重新录制声纹。".to_string());
            }
            state.capture = Some(CaptureState::Processing);
            state.progress = 70;
        }
        let result: Result<VoiceprintStatus, String> = (|| {
            let runtime = ensure_runtime()?;
            let windows = enrollment_template_windows(pcm)?;
            let embeddings = windows
                .iter()
                .map(|speech| {
                    runtime
                        .embedding(speech)
                        .map_err(|err| format!("声纹特征提取失败，请重新说三遍唤醒词：{err}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let template = SpeakerTemplate {
                embeddings,
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
        let candidate_embeddings = candidate_windows
            .iter()
            .map(|candidate| runtime.embedding(candidate))
            .collect::<Result<Vec<_>, _>>()?;
        let score = template
            .embeddings
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
            "[speaker-verification] compared enrolled_templates={} candidate_windows={} speech_ms={} score={score:.6}",
            template.embeddings.len(),
            candidate_embeddings.len(),
            candidate_windows[0].len() / 32
        );
        STATE.lock().last_score = Some(score);
        Ok(VerificationResult {
            matched: matches!(decision.action, Action::Release),
            score,
        })
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
        log::info!(
            "[speaker-verification] owner template invalidated after wake phrase change previous={} next={}; re-enrollment required",
            previous,
            next
        );
        Ok(())
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
            let encoded = encode_template(&embedding, "小爱同学", false).unwrap();
            assert!(encoded.len() < 1800);
            let decoded = decode_template(&encoded).unwrap();
            assert!((cosine(&embedding, &decoded.embeddings[0]).unwrap() - 1.0).abs() < 1e-5);
            assert_eq!(decoded.phrase.as_deref(), Some("小爱同学"));
            assert!(!decoded.invalidated);
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
            })
            .unwrap();
            assert!(decode_template(&wrong).is_err());
        }

        #[test]
        fn voiceprint_template_only_protects_its_enrolled_phrase() {
            let template = SpeakerTemplate {
                embeddings: vec![vec![1.0, 0.0]],
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
            samples.extend(vec![800i16; frame_samples * 10]);
            samples.extend(vec![0i16; frame_samples * 2]);
            samples.extend(vec![-900i16; frame_samples * 10]);
            samples.extend(vec![0i16; SAMPLE_RATE as usize]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let window = enrollment_speech_window(&pcm).expect("speech window");
            assert!(window.len() < pcm.len());
            assert!(window.len() >= SAMPLE_RATE as usize * 2 * 2);
        }

        #[test]
        fn enrollment_builds_bounded_long_and_short_owner_templates() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; frame_samples * 5];
            samples.extend(vec![800i16; frame_samples * 45]);
            samples.extend(vec![0i16; frame_samples * 5]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let windows = enrollment_template_windows(&pcm).expect("template windows");
            assert_eq!(windows.len(), MAX_SUPPLEMENTAL_TEMPLATES + 1);
            assert!(windows[0].len() > windows[1].len());
            assert!(windows[1..]
                .iter()
                .all(|window| window.len() == TEMPLATE_WINDOW_MS * 32));
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
                .embeddings
                .iter()
                .map(|enrolled| cosine(enrolled, &raw).expect("raw score"))
                .max_by(f32::total_cmp)
                .expect("raw score");
            let windows = verification_template_windows(&pcm).expect("verification windows");
            let mut processed_score = f32::NEG_INFINITY;
            for window in &windows {
                let candidate = runtime.embedding(window).expect("window embedding");
                for enrolled in &template.embeddings {
                    processed_score =
                        processed_score.max(cosine(enrolled, &candidate).expect("window score"));
                }
            }
            println!(
                "raw_score={raw_score:.6} processed_score={processed_score:.6} enrolled_templates={} candidate_windows={} threshold={VERIFICATION_THRESHOLD:.3}",
                template.embeddings.len(),
                windows.len()
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use platform::prepare_runtime_assets;
#[cfg(target_os = "windows")]
pub use platform::{
    delete_template, fail_enrollment, finish_enrollment, invalidate_for_phrase_change,
    is_enrolled_for_phrase, prepare_for_phrase, start_enrollment, status_for_phrase,
    take_enrollment_arm, verify,
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
pub fn start_enrollment(_wake_phrase: &str) -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
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

    #[test]
    fn public_status_has_explicit_local_privacy_contract() {
        assert!(status_for_phrase("开始录音").local_only);
        assert!(status_for_phrase("开始录音").threshold > 0.0);
    }
}
