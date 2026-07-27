#[derive(Debug, Clone)]
pub struct Match {
    pub end_seconds: f32,
}

use pinyin::ToPinyin;

const MAX_KEYWORD_DETECTION_LOOKBACK_SECONDS: f32 = 0.8;

fn absolute_keyword_end_seconds(
    segment_start_seconds: f32,
    token_end_seconds: Option<f32>,
    accepted_seconds: f32,
) -> f32 {
    let accepted_seconds = if accepted_seconds.is_finite() {
        accepted_seconds.max(0.0)
    } else {
        0.0
    };
    let Some(token_end_seconds) = token_end_seconds else {
        return accepted_seconds;
    };
    if !segment_start_seconds.is_finite() || !token_end_seconds.is_finite() {
        return accepted_seconds;
    }
    let native_end = segment_start_seconds.max(0.0) + token_end_seconds.max(0.0);
    let recent_detection_floor =
        (accepted_seconds - MAX_KEYWORD_DETECTION_LOOKBACK_SECONDS).max(0.0);
    native_end.max(recent_detection_floor).min(accepted_seconds)
}

fn normalized_phrase_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalPhraseRelation {
    ExactStart,
    PhoneticStart,
    PresentLater,
    Absent,
}

fn phonetic_phrase_units(value: &str) -> Vec<String> {
    normalized_phrase_text(value)
        .chars()
        .map(|ch| {
            ch.to_pinyin()
                .map(|value| value.plain().to_string())
                .unwrap_or_else(|| ch.to_lowercase().collect())
        })
        .collect()
}

pub fn local_transcript_phrase_relation(transcript: &str, phrase: &str) -> LocalPhraseRelation {
    let phrase = normalized_phrase_text(phrase);
    if phrase.is_empty() {
        return LocalPhraseRelation::Absent;
    }
    let transcript = normalized_phrase_text(transcript);
    if transcript.starts_with(&phrase) {
        LocalPhraseRelation::ExactStart
    } else {
        let phrase_units = phonetic_phrase_units(&phrase);
        let transcript_units = phonetic_phrase_units(&transcript);
        if transcript_units.len() >= phrase_units.len()
            && transcript_units[..phrase_units.len()] == phrase_units
        {
            LocalPhraseRelation::PhoneticStart
        } else if transcript.contains(&phrase) {
            LocalPhraseRelation::PresentLater
        } else {
            LocalPhraseRelation::Absent
        }
    }
}

pub fn local_transcript_matches_phrase(transcript: &str, phrase: &str) -> bool {
    matches!(
        local_transcript_phrase_relation(transcript, phrase),
        LocalPhraseRelation::ExactStart | LocalPhraseRelation::PhoneticStart
    )
}

#[cfg(test)]
mod phrase_confirmation_tests {
    #[test]
    fn keyword_boundary_is_absolute_across_sherpa_silence_segments() {
        assert_eq!(
            super::absolute_keyword_end_seconds(0.0, Some(0.68), 1.11),
            0.68
        );
        assert_eq!(
            super::absolute_keyword_end_seconds(2.56, Some(4.32), 7.56),
            6.88
        );
        let reset_boundary = super::absolute_keyword_end_seconds(0.0, Some(0.48), 4.72);
        assert!((reset_boundary - 3.92).abs() < 0.000_1);
    }

    #[test]
    fn keyword_boundary_falls_back_and_never_exceeds_accepted_pcm() {
        assert_eq!(super::absolute_keyword_end_seconds(1.0, None, 3.0), 3.0);
        assert_eq!(
            super::absolute_keyword_end_seconds(f32::NAN, Some(1.0), 3.0),
            3.0
        );
        assert_eq!(
            super::absolute_keyword_end_seconds(2.5, Some(2.0), 4.0),
            4.0
        );
    }

    #[test]
    fn local_transcript_requires_the_configured_phrase_at_utterance_start() {
        assert!(super::local_transcript_matches_phrase(
            "开始录音，今天测试。",
            "开始录音"
        ));
        assert!(super::local_transcript_matches_phrase(
            " 开始 录音。今天测试",
            "开始录音"
        ));
        assert!(!super::local_transcript_matches_phrase(
            "今天开始录音测试",
            "开始录音"
        ));
        assert!(!super::local_transcript_matches_phrase(
            "开始录像，今天测试",
            "开始录音"
        ));
        assert!(super::local_transcript_matches_phrase(
            "开使录因，今天测试",
            "开始录音"
        ));
        assert!(!super::local_transcript_matches_phrase(
            "今天开使录因测试",
            "开始录音"
        ));
        assert!(!super::local_transcript_matches_phrase(
            "开始录，今天测试",
            "开始录音"
        ));
        assert!(!super::local_transcript_matches_phrase("普通说话", ""));
    }

    #[test]
    fn local_transcript_relation_diagnoses_without_exposing_text() {
        assert_eq!(
            super::local_transcript_phrase_relation("开始录音，今天测试。", "开始录音"),
            super::LocalPhraseRelation::ExactStart
        );
        assert_eq!(
            super::local_transcript_phrase_relation("请说开始录音", "开始录音"),
            super::LocalPhraseRelation::PresentLater
        );
        assert_eq!(
            super::local_transcript_phrase_relation("开使录因，今天测试", "开始录音"),
            super::LocalPhraseRelation::PhoneticStart
        );
        assert_eq!(
            super::local_transcript_phrase_relation("普通说话", "开始录音"),
            super::LocalPhraseRelation::Absent
        );
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::Match;
    use libloading::Library;
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    use pinyin::ToPinyin;
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::fs;
    use std::io::{BufReader, Read};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const MODEL_ARCHIVE: &str = "sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20.tar.bz2";
    const MODEL_DIR: &str = "sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20";
    const MODEL_SHA256: &str = "68447F4FBC67E70EEE3A93961F36E81E98F47AEF73CE7E7CA00885C6CD3616A6";
    const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20.tar.bz2";
    const ENCODER: &str = "encoder-epoch-13-avg-2-chunk-8-left-64.int8.onnx";
    const DECODER: &str = "decoder-epoch-13-avg-2-chunk-8-left-64.onnx";
    const JOINER: &str = "joiner-epoch-13-avg-2-chunk-8-left-64.int8.onnx";
    const TOKENS: &str = "tokens.txt";
    const SAMPLE_RATE: i32 = 16_000;
    const STREAM_GAIN_WARMUP_BYTES: usize = SAMPLE_RATE as usize * 2 * 4 / 5;
    const FINAL_PADDING_SAMPLES: usize = SAMPLE_RATE as usize;
    const CONTINUOUS_SPEECH_TRAILING_BLANKS: i32 = 0;
    const KEYWORD_SCORE: f32 = 1.5;
    const KEYWORD_THRESHOLD: f32 = 0.25;
    /// Product-sensitive live defaults. Raised for everyday recall after owner
    /// reports misses at 3.0/0.08 on real device audio (2026-07-27).
    const BOOTSTRAP_KEYWORD_SCORE: f32 = 3.5;
    const BOOTSTRAP_KEYWORD_THRESHOLD: f32 = 0.05;
    const CALIBRATION_FILE: &str = "calibration.json";
    const CALIBRATION_CANDIDATES: &[(f32, f32)] = &[
        (KEYWORD_SCORE, KEYWORD_THRESHOLD),
        (1.5, 0.20),
        (2.0, 0.20),
        (2.0, 0.15),
        (2.5, 0.12),
        (3.0, 0.10),
        (3.0, 0.08),
        (3.5, 0.05),
        (4.0, 0.04),
    ];
    /// Offline second-pass cascade when streaming KWS misses (full-buffer gain).
    const RECALL_CASCADE: &[(f32, f32)] = &[
        (BOOTSTRAP_KEYWORD_SCORE, BOOTSTRAP_KEYWORD_THRESHOLD),
        (4.0, 0.04),
        (4.0, 0.03),
        (4.5, 0.03),
    ];

    #[repr(C)]
    struct TransducerConfig {
        encoder: *const c_char,
        decoder: *const c_char,
        joiner: *const c_char,
    }
    #[repr(C)]
    struct ParaformerConfig {
        encoder: *const c_char,
        decoder: *const c_char,
    }
    #[repr(C)]
    struct SingleModelConfig {
        model: *const c_char,
    }
    #[repr(C)]
    struct OnlineModelConfig {
        transducer: TransducerConfig,
        paraformer: ParaformerConfig,
        zipformer2_ctc: SingleModelConfig,
        tokens: *const c_char,
        num_threads: i32,
        provider: *const c_char,
        debug: i32,
        model_type: *const c_char,
        modeling_unit: *const c_char,
        bpe_vocab: *const c_char,
        tokens_buf: *const c_char,
        tokens_buf_size: i32,
        nemo_ctc: SingleModelConfig,
        t_one_ctc: SingleModelConfig,
    }
    #[repr(C)]
    struct FeatureConfig {
        sample_rate: i32,
        feature_dim: i32,
    }
    #[repr(C)]
    struct KeywordConfig {
        feat_config: FeatureConfig,
        model_config: OnlineModelConfig,
        max_active_paths: i32,
        num_trailing_blanks: i32,
        keywords_score: f32,
        keywords_threshold: f32,
        keywords_file: *const c_char,
        keywords_buf: *const c_char,
        keywords_buf_size: i32,
    }
    #[repr(C)]
    struct KeywordResult {
        keyword: *const c_char,
        tokens: *const c_char,
        tokens_arr: *const *const c_char,
        count: i32,
        timestamps: *const f32,
        start_time: f32,
        json: *const c_char,
    }

    type CreateSpotter = unsafe extern "C" fn(*const KeywordConfig) -> *const c_void;
    type DestroySpotter = unsafe extern "C" fn(*const c_void);
    type CreateStream = unsafe extern "C" fn(*const c_void) -> *const c_void;
    type AcceptWaveform = unsafe extern "C" fn(*const c_void, i32, *const f32, i32);
    type InputFinished = unsafe extern "C" fn(*const c_void);
    type IsReady = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
    type Decode = unsafe extern "C" fn(*const c_void, *const c_void);
    type GetResult = unsafe extern "C" fn(*const c_void, *const c_void) -> *const KeywordResult;
    type DestroyResult = unsafe extern "C" fn(*const KeywordResult);
    type DestroyStream = unsafe extern "C" fn(*const c_void);

    struct Runtime {
        _onnx: Library,
        _providers: Library,
        _sherpa: Library,
        spotter: *const c_void,
        create_stream: CreateStream,
        accept_waveform: AcceptWaveform,
        input_finished: InputFinished,
        is_ready: IsReady,
        decode: Decode,
        get_result: GetResult,
        destroy_result: DestroyResult,
        destroy_stream: DestroyStream,
        destroy_spotter: DestroySpotter,
    }
    unsafe impl Send for Runtime {}
    unsafe impl Sync for Runtime {}
    impl Drop for Runtime {
        fn drop(&mut self) {
            unsafe { (self.destroy_spotter)(self.spotter) };
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct StoredCalibration {
        version: u8,
        model_sha256: String,
        phrase: String,
        score: f32,
        threshold: f32,
    }

    static CACHE: Lazy<Mutex<Option<(String, u32, u32, Arc<Runtime>)>>> =
        Lazy::new(|| Mutex::new(None));

    /// Live KWS always uses the product-sensitive bootstrap (score 3.0 / threshold 0.08).
    ///
    /// Historical enrollment `calibrate()` walked candidates from *strictest* → most
    /// sensitive and pinned the first match (often 1.5 / 0.25). That reduced false
    /// starts on clean enrollment audio but **tanked everyday wake recall**. Owner
    /// contract: primary automatic wake is the sensitive path; voiceprint (when
    /// enrolled) and local confirmation remain the false-start gates — not a strict
    /// KWS pin.
    fn configured_keyword_values(phrase: &str) -> (f32, f32) {
        let _ = phrase;
        #[cfg(test)]
        if let (Ok(score), Ok(threshold)) = (
            std::env::var("LISTENER_KWS_SCORE"),
            std::env::var("LISTENER_KWS_THRESHOLD"),
        ) {
            return (
                score.parse().expect("LISTENER_KWS_SCORE float"),
                threshold.parse().expect("LISTENER_KWS_THRESHOLD float"),
            );
        }
        (BOOTSTRAP_KEYWORD_SCORE, BOOTSTRAP_KEYWORD_THRESHOLD)
    }

    fn is_less_sensitive_than_bootstrap(score: f32, threshold: f32) -> bool {
        score + f32::EPSILON < BOOTSTRAP_KEYWORD_SCORE
            || threshold > BOOTSTRAP_KEYWORD_THRESHOLD + f32::EPSILON
    }

    fn clear_runtime_cache() {
        *CACHE.lock() = None;
    }

    fn calibration_path() -> Result<PathBuf, String> {
        let root = crate::persistence::speaker_verification_root()
            .map_err(|err| format!("create wake phrase calibration directory failed: {err}"))?
            .join("keyword-spotting");
        fs::create_dir_all(&root).map_err(|err| err.to_string())?;
        Ok(root.join(CALIBRATION_FILE))
    }

    fn read_calibration(phrase: &str) -> Option<StoredCalibration> {
        let path = calibration_path().ok()?;
        let value = fs::read_to_string(path).ok()?;
        let calibration: StoredCalibration = match serde_json::from_str(&value) {
            Ok(calibration) => calibration,
            Err(err) => {
                log::warn!("[wake-phrase] ignored invalid local calibration: {err}");
                return None;
            }
        };
        (calibration.version == 1
            && calibration.model_sha256 == MODEL_SHA256
            && calibration.phrase == phrase
            && CALIBRATION_CANDIDATES.contains(&(calibration.score, calibration.threshold)))
        .then_some(calibration)
    }

    fn save_calibration(phrase: &str, score: f32, threshold: f32) -> Result<(), String> {
        let value = serde_json::to_vec_pretty(&StoredCalibration {
            version: 1,
            model_sha256: MODEL_SHA256.to_string(),
            phrase: phrase.to_string(),
            score,
            threshold,
        })
        .map_err(|err| err.to_string())?;
        fs::write(calibration_path()?, value).map_err(|err| err.to_string())?;
        // Spotter embeds keywords_score/threshold; force rebuild on next prepare.
        clear_runtime_cache();
        Ok(())
    }

    /// Ensure on-disk calibration matches product-sensitive bootstrap.
    ///
    /// Returns `true` when the file was created or upgraded (stale strict pins).
    pub fn persist_bootstrap_calibration_if_missing(phrase: &str) -> Result<bool, String> {
        if let Some(existing) = read_calibration(phrase) {
            if !is_less_sensitive_than_bootstrap(existing.score, existing.threshold) {
                return Ok(false);
            }
            save_calibration(phrase, BOOTSTRAP_KEYWORD_SCORE, BOOTSTRAP_KEYWORD_THRESHOLD)?;
            log::info!(
                "[wake-phrase] upgraded less-sensitive calibration to bootstrap phrase={} was_score={:.1} was_threshold={:.2} score={:.1} threshold={:.2}",
                phrase,
                existing.score,
                existing.threshold,
                BOOTSTRAP_KEYWORD_SCORE,
                BOOTSTRAP_KEYWORD_THRESHOLD
            );
            return Ok(true);
        }
        save_calibration(phrase, BOOTSTRAP_KEYWORD_SCORE, BOOTSTRAP_KEYWORD_THRESHOLD)?;
        log::info!(
            "[wake-phrase] verified bootstrap calibration saved phrase={} score={:.1} threshold={:.2}",
            phrase,
            BOOTSTRAP_KEYWORD_SCORE,
            BOOTSTRAP_KEYWORD_THRESHOLD
        );
        Ok(true)
    }

    fn sha256(path: &Path) -> Result<String, String> {
        let mut reader = BufReader::new(fs::File::open(path).map_err(|e| e.to_string())?);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(format!("{:X}", hasher.finalize()))
    }

    fn model_root() -> Result<PathBuf, String> {
        let root = crate::persistence::speaker_verification_root()
            .map_err(|e| format!("create wake phrase model directory failed: {e}"))?
            .join("keyword-spotting");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let archive = root.join(MODEL_ARCHIVE);
        if !archive.exists() || sha256(&archive)? != MODEL_SHA256 {
            let temp = archive.with_extension("download");
            let bytes = reqwest::blocking::get(MODEL_URL)
                .map_err(|e| format!("download wake phrase model failed: {e}"))?
                .error_for_status()
                .map_err(|e| format!("wake phrase model download failed: {e}"))?
                .bytes()
                .map_err(|e| e.to_string())?;
            fs::write(&temp, &bytes).map_err(|e| e.to_string())?;
            if sha256(&temp)? != MODEL_SHA256 {
                let _ = fs::remove_file(&temp);
                return Err("wake phrase model hash mismatch".into());
            }
            fs::rename(temp, &archive).map_err(|e| e.to_string())?;
        }
        let model = root.join(MODEL_DIR);
        if ![ENCODER, DECODER, JOINER, TOKENS]
            .iter()
            .all(|name| model.join(name).exists())
        {
            let decoder =
                bzip2::read::BzDecoder::new(fs::File::open(&archive).map_err(|e| e.to_string())?);
            let mut tar = tar::Archive::new(decoder);
            tar.unpack(&root).map_err(|e| e.to_string())?;
        }
        Ok(model)
    }

    fn phrase_tokens_with_tone(phrase: &str, with_tone: bool) -> Result<Vec<String>, String> {
        let initials = [
            "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x",
            "r", "z", "c", "s", "y", "w",
        ];
        let mut tokens = Vec::new();
        for ch in phrase.chars() {
            let pinyin = ch
                .to_pinyin()
                .ok_or_else(|| "当前唤醒词仅支持中文汉字".to_string())?;
            let pinyin = if with_tone {
                pinyin.with_tone()
            } else {
                pinyin.plain()
            };
            let initial = initials
                .iter()
                .find(|initial| pinyin.starts_with(**initial))
                .copied()
                .unwrap_or("");
            if !initial.is_empty() {
                tokens.push(initial.to_string());
            }
            let final_part = &pinyin[initial.len()..];
            if final_part.is_empty() {
                return Err("无法生成唤醒词拼音".into());
            }
            tokens.push(final_part.to_string());
        }
        Ok(tokens)
    }

    fn keyword_entry(
        phrase: &str,
        label: &str,
        score: f32,
        threshold: f32,
        with_tone: bool,
    ) -> Result<String, String> {
        Ok(format!(
            "{} :{:.1} #{:.2} @{}",
            phrase_tokens_with_tone(phrase, with_tone)?.join(" "),
            score,
            threshold,
            label
        ))
    }

    fn keyword_tokens(phrase: &str, score: f32, threshold: f32, emit_variants: bool) -> Result<String, String> {
        let phrase = phrase.trim().replace(char::is_whitespace, "");
        if phrase.is_empty() || phrase.chars().count() > 16 {
            return Err("唤醒词应为 1 到 16 个汉字".into());
        }
        let mut keywords = vec![
            keyword_entry(&phrase, &phrase, score, threshold, true)?,
            keyword_entry(&phrase, &phrase, score, threshold, false)?,
        ];
        if emit_variants && phrase.chars().count() >= 4 {
            let leading_trimmed = phrase.chars().skip(1).collect::<String>();
            let trailing_trimmed = phrase
                .chars()
                .take(phrase.chars().count() - 1)
                .collect::<String>();
            let short_threshold = (threshold * 1.5).min(0.35);
            keywords.push(keyword_entry(
                &leading_trimmed,
                &phrase,
                score,
                short_threshold,
                true,
            )?);
            keywords.push(keyword_entry(
                &trailing_trimmed,
                &phrase,
                score,
                short_threshold,
                true,
            )?);
        }
        Ok(keywords.join("\n"))
    }

    fn cstring(path: &Path) -> Result<CString, String> {
        CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "模型路径无效".into())
    }

    fn load_with_config(phrase: &str, score: f32, threshold: f32, emit_variants: bool) -> Result<Arc<Runtime>, String> {
        if let Some((cached_phrase, cached_score, cached_threshold, runtime)) =
            CACHE.lock().as_ref()
        {
            if cached_phrase == phrase
                && *cached_score == score.to_bits()
                && *cached_threshold == threshold.to_bits()
            {
                return Ok(Arc::clone(runtime));
            }
        }
        let model = model_root()?;
        let dll_root = model
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| "wake phrase runtime path is invalid".to_string())?;
        let encoder = cstring(&model.join(ENCODER))?;
        let decoder = cstring(&model.join(DECODER))?;
        let joiner = cstring(&model.join(JOINER))?;
        let tokens = cstring(&model.join(TOKENS))?;
        let provider = CString::new("cpu").unwrap();
        let modeling_unit = CString::new("cjkchar").unwrap();
        let keyword =
            CString::new(keyword_tokens(phrase, score, threshold, emit_variants)?).map_err(|_| "唤醒词无效")?;
        unsafe {
            let onnx = Library::new(dll_root.join("onnxruntime.dll")).map_err(|e| e.to_string())?;
            let providers = Library::new(dll_root.join("onnxruntime_providers_shared.dll"))
                .map_err(|e| e.to_string())?;
            let sherpa =
                Library::new(dll_root.join("sherpa-onnx-c-api.dll")).map_err(|e| e.to_string())?;
            macro_rules! sym {
                ($name:literal, $ty:ty) => {
                    *sherpa
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|e| e.to_string())?
                };
            }
            let create: CreateSpotter = sym!("SherpaOnnxCreateKeywordSpotter", CreateSpotter);
            let destroy_spotter = sym!("SherpaOnnxDestroyKeywordSpotter", DestroySpotter);
            let create_stream = sym!("SherpaOnnxCreateKeywordStream", CreateStream);
            let accept_waveform = sym!("SherpaOnnxOnlineStreamAcceptWaveform", AcceptWaveform);
            let input_finished = sym!("SherpaOnnxOnlineStreamInputFinished", InputFinished);
            let is_ready = sym!("SherpaOnnxIsKeywordStreamReady", IsReady);
            let decode = sym!("SherpaOnnxDecodeKeywordStream", Decode);
            let get_result = sym!("SherpaOnnxGetKeywordResult", GetResult);
            let destroy_result = sym!("SherpaOnnxDestroyKeywordResult", DestroyResult);
            let destroy_stream = sym!("SherpaOnnxDestroyOnlineStream", DestroyStream);
            let config = KeywordConfig {
                feat_config: FeatureConfig {
                    sample_rate: SAMPLE_RATE,
                    feature_dim: 80,
                },
                model_config: OnlineModelConfig {
                    transducer: TransducerConfig {
                        encoder: encoder.as_ptr(),
                        decoder: decoder.as_ptr(),
                        joiner: joiner.as_ptr(),
                    },
                    paraformer: ParaformerConfig {
                        encoder: std::ptr::null(),
                        decoder: std::ptr::null(),
                    },
                    zipformer2_ctc: SingleModelConfig {
                        model: std::ptr::null(),
                    },
                    tokens: tokens.as_ptr(),
                    num_threads: 1,
                    provider: provider.as_ptr(),
                    debug: 0,
                    model_type: std::ptr::null(),
                    modeling_unit: modeling_unit.as_ptr(),
                    bpe_vocab: std::ptr::null(),
                    tokens_buf: std::ptr::null(),
                    tokens_buf_size: 0,
                    nemo_ctc: SingleModelConfig {
                        model: std::ptr::null(),
                    },
                    t_one_ctc: SingleModelConfig {
                        model: std::ptr::null(),
                    },
                },
                max_active_paths: 4,
                num_trailing_blanks: CONTINUOUS_SPEECH_TRAILING_BLANKS,
                keywords_score: score,
                keywords_threshold: threshold,
                keywords_file: std::ptr::null(),
                keywords_buf: keyword.as_ptr(),
                keywords_buf_size: keyword.as_bytes().len() as i32,
            };
            let spotter = create(&config);
            if spotter.is_null() {
                return Err("初始化本地唤醒词模型失败".into());
            }
            let runtime = Arc::new(Runtime {
                _onnx: onnx,
                _providers: providers,
                _sherpa: sherpa,
                spotter,
                create_stream,
                accept_waveform,
                input_finished,
                is_ready,
                decode,
                get_result,
                destroy_result,
                destroy_stream,
                destroy_spotter,
            });
            *CACHE.lock() = Some((
                phrase.to_string(),
                score.to_bits(),
                threshold.to_bits(),
                Arc::clone(&runtime),
            ));
            Ok(runtime)
        }
    }

    fn load(phrase: &str) -> Result<Arc<Runtime>, String> {
        let (score, threshold) = configured_keyword_values(phrase);
        load_with_config(phrase, score, threshold, true)
    }

    fn normalization_gain(pcm: &[u8]) -> f32 {
        let mut absolute = pcm
            .chunks_exact(2)
            .map(|value| i16::from_le_bytes([value[0], value[1]]).unsigned_abs() as f32 / 32768.0)
            .collect::<Vec<_>>();
        absolute.sort_by(f32::total_cmp);
        let reference = if absolute.is_empty() {
            0.0
        } else {
            absolute[(absolute.len() * 95 / 100).min(absolute.len() - 1)]
        };
        let gain = if reference > f32::EPSILON {
            (0.65 / reference).clamp(1.0, 48.0)
        } else {
            1.0
        };
        gain
    }

    fn samples_with_gain(pcm: &[u8], gain: f32) -> Vec<f32> {
        pcm.chunks_exact(2)
            .map(|value| {
                (i16::from_le_bytes([value[0], value[1]]) as f32 / 32768.0 * gain).clamp(-1.0, 1.0)
            })
            .collect()
    }

    fn normalized_kws_samples(pcm: &[u8]) -> (Vec<f32>, f32) {
        let gain = normalization_gain(pcm);
        (samples_with_gain(pcm, gain), gain)
    }

    #[derive(Default)]
    struct StreamingNormalizer {
        warmup: Vec<u8>,
        gain: Option<f32>,
        accepted_bytes: usize,
        emitted_bytes: usize,
    }

    impl StreamingNormalizer {
        fn accept(&mut self, pcm: &[u8]) -> Result<Vec<f32>, String> {
            if pcm.len() % 2 != 0 {
                return Err("唤醒词 PCM16 数据长度无效".into());
            }
            self.accepted_bytes = self.accepted_bytes.saturating_add(pcm.len());
            if let Some(gain) = self.gain {
                self.emitted_bytes = self.emitted_bytes.saturating_add(pcm.len());
                return Ok(samples_with_gain(pcm, gain));
            }

            let needed = STREAM_GAIN_WARMUP_BYTES.saturating_sub(self.warmup.len());
            let split = needed.min(pcm.len());
            self.warmup.extend_from_slice(&pcm[..split]);
            if self.warmup.len() < STREAM_GAIN_WARMUP_BYTES {
                return Ok(Vec::new());
            }

            let gain = normalization_gain(&self.warmup);
            self.gain = Some(gain);
            let mut samples = samples_with_gain(&self.warmup, gain);
            self.emitted_bytes = self.emitted_bytes.saturating_add(self.warmup.len());
            self.warmup.clear();
            if split < pcm.len() {
                samples.extend(samples_with_gain(&pcm[split..], gain));
                self.emitted_bytes = self
                    .emitted_bytes
                    .saturating_add(pcm.len().saturating_sub(split));
            }
            Ok(samples)
        }

        fn finish(&mut self) -> Vec<f32> {
            if self.warmup.is_empty() {
                return Vec::new();
            }
            let gain = normalization_gain(&self.warmup);
            self.gain = Some(gain);
            self.emitted_bytes = self.emitted_bytes.saturating_add(self.warmup.len());
            let samples = samples_with_gain(&self.warmup, gain);
            self.warmup.clear();
            samples
        }
    }

    pub struct StreamingDetector {
        runtime: Arc<Runtime>,
        stream: *const c_void,
        normalizer: StreamingNormalizer,
        found: Option<Match>,
        finished: bool,
    }

    unsafe impl Send for StreamingDetector {}

    impl StreamingDetector {
        pub fn new(phrase: &str) -> Result<Self, String> {
            let runtime = load(phrase)?;
            let stream = unsafe { (runtime.create_stream)(runtime.spotter) };
            if stream.is_null() {
                return Err("创建唤醒词流失败".into());
            }
            Ok(Self {
                runtime,
                stream,
                normalizer: StreamingNormalizer::default(),
                found: None,
                finished: false,
            })
        }

        fn new_with_config(phrase: &str, score: f32, threshold: f32, emit_variants: bool) -> Result<Self, String> {
            let runtime = load_with_config(phrase, score, threshold, emit_variants)?;
            let stream = unsafe { (runtime.create_stream)(runtime.spotter) };
            if stream.is_null() {
                return Err("创建唤醒词流失败".into());
            }
            Ok(Self {
                runtime,
                stream,
                normalizer: StreamingNormalizer::default(),
                found: None,
                finished: false,
            })
        }

        /// 严格模式:不生成"去首字/去末字"前缀变体,只认完整唤醒词。
        /// 用于 A-desktop 录音期续唤——前缀变体会让"开始录像/录入/路演"等近似音
        /// 误命中(TTS 实测 4/10),严格模式 TTS 实测 0/10 且正样本仍命中。代价是
        /// 对吞字/含糊的容忍度降低,故仅用于录音期续唤,唤醒候选阶段仍用 new()。
        /// 注:0/10 是 TTS(机械声)数据,真人连读/方言可能不同,上线前需实测。
        pub fn new_strict(phrase: &str) -> Result<Self, String> {
            let (score, threshold) = configured_keyword_values(phrase);
            let runtime = load_with_config(phrase, score, threshold, false)?;
            let stream = unsafe { (runtime.create_stream)(runtime.spotter) };
            if stream.is_null() {
                return Err("创建唤醒词流失败".into());
            }
            Ok(Self {
                runtime,
                stream,
                normalizer: StreamingNormalizer::default(),
                found: None,
                finished: false,
            })
        }

        pub fn accept_pcm(&mut self, pcm: &[u8]) -> Result<Option<Match>, String> {
            if self.finished {
                return Err("唤醒词流已经结束".into());
            }
            if self.found.is_some() {
                return Ok(self.found.clone());
            }
            let samples = self.normalizer.accept(pcm)?;
            self.accept_samples(&samples);
            self.decode_ready();
            Ok(self.found.clone())
        }

        /// Offline path: feed already gain-normalized samples (full-buffer p95).
        fn accept_pre_normalized_samples(&mut self, samples: &[f32]) {
            if self.finished || self.found.is_some() || samples.is_empty() {
                return;
            }
            // Keep boundary math consistent with streaming path (bytes of PCM16).
            self.normalizer.accepted_bytes = self
                .normalizer
                .accepted_bytes
                .saturating_add(samples.len().saturating_mul(2));
            self.accept_samples(samples);
            self.decode_ready();
        }

        fn finish_pre_normalized(&mut self) -> Result<Option<Match>, String> {
            if self.finished {
                return Ok(self.found.clone());
            }
            let padding = vec![0.0; FINAL_PADDING_SAMPLES];
            self.accept_samples(&padding);
            unsafe { (self.runtime.input_finished)(self.stream) };
            self.finished = true;
            self.decode_ready();
            Ok(self.found.clone())
        }

        pub fn finish(&mut self) -> Result<Option<Match>, String> {
            if self.finished {
                return Ok(self.found.clone());
            }
            let samples = self.normalizer.finish();
            self.accept_samples(&samples);
            let padding = vec![0.0; FINAL_PADDING_SAMPLES];
            self.accept_samples(&padding);
            unsafe { (self.runtime.input_finished)(self.stream) };
            self.finished = true;
            self.decode_ready();
            Ok(self.found.clone())
        }

        fn accept_samples(&self, samples: &[f32]) {
            if samples.is_empty() || self.found.is_some() {
                return;
            }
            unsafe {
                (self.runtime.accept_waveform)(
                    self.stream,
                    SAMPLE_RATE,
                    samples.as_ptr(),
                    samples.len() as i32,
                );
            }
        }

        fn decode_ready(&mut self) {
            if self.found.is_some() {
                return;
            }
            unsafe {
                while (self.runtime.is_ready)(self.runtime.spotter, self.stream) != 0 {
                    (self.runtime.decode)(self.runtime.spotter, self.stream);
                    let result = (self.runtime.get_result)(self.runtime.spotter, self.stream);
                    if result.is_null() {
                        continue;
                    }
                    let value = &*result;
                    if !value.keyword.is_null()
                        && !CStr::from_ptr(value.keyword).to_bytes().is_empty()
                    {
                        let accepted_seconds = self.normalizer.accepted_bytes as f32 / 32_000.0;
                        let token_end_seconds = if value.count > 0 && !value.timestamps.is_null() {
                            Some(*value.timestamps.add(value.count as usize - 1))
                        } else {
                            None
                        };
                        let end_seconds = super::absolute_keyword_end_seconds(
                            value.start_time,
                            token_end_seconds,
                            accepted_seconds,
                        );
                        if let Some(token_end_seconds) = token_end_seconds {
                            log::info!(
                                "[wake-phrase] keyword audio boundary segment_start_s={:.3} token_end_s={:.3} absolute_end_s={:.3} accepted_pcm_s={:.3}",
                                value.start_time,
                                token_end_seconds,
                                end_seconds,
                                accepted_seconds
                            );
                        } else {
                            log::warn!(
                                "[wake-phrase] keyword result missing token timestamps; using accepted PCM boundary absolute_end_s={:.3}",
                                end_seconds
                            );
                        }
                        self.found = Some(Match { end_seconds });
                    }
                    (self.runtime.destroy_result)(result);
                    if self.found.is_some() {
                        break;
                    }
                }
            }
        }
    }

    impl Drop for StreamingDetector {
        fn drop(&mut self) {
            unsafe { (self.runtime.destroy_stream)(self.stream) };
        }
    }

    fn detect_with_config(
        pcm: &[u8],
        phrase: &str,
        score: f32,
        threshold: f32,
    ) -> Result<Option<Match>, String> {
        // Full-buffer gain: streaming warmup often locks on quiet pre-roll and
        // under-amplifies the later wake phrase on device candidates.
        let gain = normalization_gain(pcm);
        log::info!(
            "[wake-phrase] offline detect pcm_bytes={} gain={gain:.2} score={score:.1} threshold={threshold:.2}",
            pcm.len(),
        );
        let samples = samples_with_gain(pcm, gain);
        let mut detector = StreamingDetector::new_with_config(phrase, score, threshold, true)?;
        detector.accept_pre_normalized_samples(&samples);
        detector.finish_pre_normalized()
    }

    pub fn detect(pcm: &[u8], phrase: &str) -> Result<Option<Match>, String> {
        let (score, threshold) = configured_keyword_values(phrase);
        detect_with_config(pcm, phrase, score, threshold)
    }

    /// When the live streaming detector misses, re-run offline over the whole
    /// candidate with full-buffer gain and a short sensitive cascade.
    pub fn detect_with_recall_cascade(pcm: &[u8], phrase: &str) -> Result<Option<Match>, String> {
        if pcm.len() < SAMPLE_RATE as usize {
            return Ok(None);
        }
        let mut tried = std::collections::BTreeSet::new();
        for &(score, threshold) in RECALL_CASCADE {
            let key = ((score * 100.0) as i32, (threshold * 1000.0) as i32);
            if !tried.insert(key) {
                continue;
            }
            match detect_with_config(pcm, phrase, score, threshold)? {
                Some(found) => {
                    log::info!(
                        "[wake-phrase] offline recall cascade hit phrase={} score={score:.1} threshold={threshold:.2} end_s={:.3} pcm_ms={}",
                        phrase,
                        found.end_seconds,
                        pcm.len() / 32
                    );
                    return Ok(Some(found));
                }
                None => {}
            }
        }
        log::info!(
            "[wake-phrase] offline recall cascade miss phrase={} pcm_ms={} configs={}",
            phrase,
            pcm.len() / 32,
            RECALL_CASCADE.len()
        );
        Ok(None)
    }

    fn select_calibration<F>(mut detects: F) -> Result<Option<(f32, f32)>, String>
    where
        F: FnMut(f32, f32) -> Result<bool, String>,
    {
        for &(score, threshold) in CALIBRATION_CANDIDATES {
            if detects(score, threshold)? {
                return Ok(Some((score, threshold)));
            }
        }
        Ok(None)
    }

    pub fn calibrate(pcm: &[u8], phrase: &str) -> Result<(), String> {
        // Prove the enrollment audio contains the configured phrase under *some*
        // candidate (including strict). Runtime still pins product-sensitive bootstrap
        // so everyday wake is not locked to the strictest enrollment match.
        let detected_with = select_calibration(|score, threshold| {
            detect_with_config(pcm, phrase, score, threshold).map(|result| result.is_some())
        })?
        .ok_or_else(|| "没有在声纹样本中识别到唤醒词，请用自然语速清晰重复三遍".to_string())?;
        save_calibration(phrase, BOOTSTRAP_KEYWORD_SCORE, BOOTSTRAP_KEYWORD_THRESHOLD)?;
        log::info!(
            "[wake-phrase] enrollment phrase verified (matched_at score={:.1} threshold={:.2}); runtime calibration pinned to bootstrap phrase={} score={:.1} threshold={:.2}",
            detected_with.0,
            detected_with.1,
            phrase,
            BOOTSTRAP_KEYWORD_SCORE,
            BOOTSTRAP_KEYWORD_THRESHOLD
        );
        Ok(())
    }

    pub fn prepare(phrase: &str) -> Result<(), String> {
        let _ = persist_bootstrap_calibration_if_missing(phrase);
        let (score, threshold) = configured_keyword_values(phrase);
        log::info!(
            "[wake-phrase] runtime KWS config phrase={} score={:.1} threshold={:.2}",
            phrase,
            score,
            threshold
        );
        load(phrase).map(|_| ())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn wav_pcm(wav: &[u8]) -> &[u8] {
            let data = wav
                .windows(4)
                .position(|window| window == b"data")
                .expect("data chunk");
            let size = u32::from_le_bytes(wav[data + 4..data + 8].try_into().unwrap()) as usize;
            &wav[data + 8..data + 8 + size]
        }

        #[test]
        fn chinese_phrase_is_tokenized_for_phone_pinyin_model() {
            assert_eq!(CONTINUOUS_SPEECH_TRAILING_BLANKS, 0);
            assert_eq!(
                keyword_tokens("开始录音", KEYWORD_SCORE, KEYWORD_THRESHOLD, true).expect("tokens"),
                concat!(
                    "k āi sh ǐ l ù y īn :1.5 #0.25 @开始录音\n",
                    "k ai sh i l u y in :1.5 #0.25 @开始录音\n",
                    "sh ǐ l ù y īn :1.5 #0.35 @开始录音\n",
                    "k āi sh ǐ l ù :1.5 #0.35 @开始录音"
                )
            );
            assert_eq!(
                keyword_tokens("录音", KEYWORD_SCORE, KEYWORD_THRESHOLD, true).expect("tokens"),
                concat!("l ù y īn :1.5 #0.25 @录音\n", "l u y in :1.5 #0.25 @录音")
            );
        }

        #[test]
        fn calibration_selects_the_strictest_matching_candidate() {
            let selected =
                select_calibration(|score, threshold| Ok(score >= 2.0 && threshold <= 0.15))
                    .expect("calibration");
            assert_eq!(selected, Some((2.0, 0.15)));
            assert_eq!(
                (BOOTSTRAP_KEYWORD_SCORE, BOOTSTRAP_KEYWORD_THRESHOLD),
                *CALIBRATION_CANDIDATES.last().expect("bootstrap candidate")
            );
        }

        #[test]
        fn runtime_keyword_values_use_product_sensitive_bootstrap() {
            // Live path must not inherit a strict enrollment pin such as 1.5/0.25.
            let (score, threshold) = configured_keyword_values("开始录音");
            assert_eq!(score, BOOTSTRAP_KEYWORD_SCORE);
            assert_eq!(threshold, BOOTSTRAP_KEYWORD_THRESHOLD);
            assert_eq!(BOOTSTRAP_KEYWORD_SCORE, 3.5);
            assert!((BOOTSTRAP_KEYWORD_THRESHOLD - 0.05).abs() < f32::EPSILON);
            assert!(is_less_sensitive_than_bootstrap(1.5, 0.25));
            assert!(is_less_sensitive_than_bootstrap(3.0, 0.10));
            assert!(is_less_sensitive_than_bootstrap(3.0, 0.08));
            assert!(!is_less_sensitive_than_bootstrap(
                BOOTSTRAP_KEYWORD_SCORE,
                BOOTSTRAP_KEYWORD_THRESHOLD
            ));
            assert!(!RECALL_CASCADE.is_empty());
        }

        #[test]
        fn calibration_rejects_a_sample_without_the_phrase() {
            let selected = select_calibration(|_, _| Ok(false)).expect("calibration");
            assert_eq!(selected, None);
        }

        #[test]
        fn quiet_device_pcm_receives_bounded_keyword_gain() {
            let pcm = (0..SAMPLE_RATE)
                .flat_map(|index| {
                    let sample = if index % 2 == 0 { 100i16 } else { -100i16 };
                    sample.to_le_bytes()
                })
                .collect::<Vec<_>>();
            let (samples, gain) = normalized_kws_samples(&pcm);
            assert_eq!(gain, 48.0);
            assert!(samples.iter().all(|sample| sample.abs() <= 1.0));
            assert!(samples.iter().any(|sample| sample.abs() > 0.1));
        }

        #[test]
        fn streaming_normalization_is_chunk_boundary_invariant_and_one_pass() {
            let pcm = (0..(SAMPLE_RATE + 4_000))
                .flat_map(|index| {
                    let amplitude = 200 + index % 2_000;
                    let sample = if index % 2 == 0 {
                        amplitude as i16
                    } else {
                        -(amplitude as i16)
                    };
                    sample.to_le_bytes()
                })
                .collect::<Vec<_>>();

            let mut whole = StreamingNormalizer::default();
            let mut whole_samples = whole.accept(&pcm).expect("whole candidate");
            whole_samples.extend(whole.finish());

            let mut chunked = StreamingNormalizer::default();
            let mut chunked_samples = Vec::new();
            let chunk_sizes = [320usize, 1_024, 640, 2_048];
            let mut offset = 0usize;
            let mut chunk_index = 0usize;
            while offset < pcm.len() {
                let end = (offset + chunk_sizes[chunk_index % chunk_sizes.len()]).min(pcm.len());
                chunked_samples.extend(chunked.accept(&pcm[offset..end]).expect("chunk"));
                offset = end;
                chunk_index += 1;
            }
            chunked_samples.extend(chunked.finish());

            assert_eq!(chunked_samples, whole_samples);
            assert_eq!(whole.accepted_bytes, pcm.len());
            assert_eq!(whole.emitted_bytes, pcm.len());
            assert_eq!(chunked.accepted_bytes, pcm.len());
            assert_eq!(chunked.emitted_bytes, pcm.len());
        }

        #[test]
        fn short_stream_flushes_every_accepted_pcm16_sample_once() {
            let pcm = vec![7u8; STREAM_GAIN_WARMUP_BYTES / 2];
            let mut normalizer = StreamingNormalizer::default();
            assert!(normalizer.accept(&pcm).expect("short chunk").is_empty());
            let samples = normalizer.finish();
            assert_eq!(samples.len() * 2, pcm.len());
            assert_eq!(normalizer.accepted_bytes, pcm.len());
            assert_eq!(normalizer.emitted_bytes, pcm.len());
        }

        #[test]
        #[ignore = "downloads and executes the official sherpa-onnx KWS model"]
        fn official_chinese_fixture_triggers_expected_keyword() {
            let model = model_root().expect("model");
            let wav = fs::read(model.join("test_wavs").join("zh_5.wav")).expect("fixture");
            let result = detect(wav_pcm(&wav), "周望军").expect("detect");
            assert!(result.is_some());
        }

        #[test]
        #[ignore = "requires a consented local calibration WAV"]
        fn operator_calibration_wav_triggers_configured_keyword() {
            let path =
                std::env::var("LISTENER_WAKE_PHRASE_WAV").expect("LISTENER_WAKE_PHRASE_WAV path");
            let phrase =
                std::env::var("LISTENER_WAKE_PHRASE").unwrap_or_else(|_| "开始录音".to_string());
            let wav = fs::read(path).expect("calibration wav");
            let pcm = wav_pcm(&wav);
            assert!(detect(pcm, &phrase).expect("whole detect").is_some());
            let mut prefixed_pcm = vec![0u8; SAMPLE_RATE as usize * 2 * 3];
            prefixed_pcm.extend_from_slice(pcm);
            let prefixed_match = detect(&prefixed_pcm, &phrase)
                .expect("prefixed detect")
                .expect("prefixed phrase match");
            assert!(
                prefixed_match.end_seconds >= 3.0,
                "prefixed wake boundary must be absolute, got {}",
                prefixed_match.end_seconds
            );
            let mut detector = StreamingDetector::new(&phrase).expect("streaming detector");
            let mut result = None;
            let started = std::time::Instant::now();
            let mut fed_bytes = 0usize;
            for chunk in pcm.chunks(320) {
                fed_bytes += chunk.len();
                result = detector.accept_pcm(chunk).expect("streaming detect");
                if result.is_some() {
                    break;
                }
            }
            assert!(
                result.is_some(),
                "streaming detector must trigger before the candidate ends"
            );
            println!(
                "streaming_wake_live_match fed_pcm_ms={} compute_ms={}",
                fed_bytes / 32,
                started.elapsed().as_millis()
            );
        }

        #[test]
        #[ignore = "requires a consented local non-keyword WAV"]
        fn operator_non_keyword_wav_does_not_trigger() {
            let paths = std::env::var("LISTENER_NON_WAKE_PHRASE_WAVS")
                .or_else(|_| std::env::var("LISTENER_NON_WAKE_PHRASE_WAV"))
                .expect("LISTENER_NON_WAKE_PHRASE_WAVS paths");
            let phrase =
                std::env::var("LISTENER_WAKE_PHRASE").unwrap_or_else(|_| "开始录音".to_string());
            let mut total_pcm_ms = 0usize;
            for path in paths.split(';').filter(|path| !path.is_empty()) {
                let wav = fs::read(path).expect("non-keyword wav");
                let pcm = wav_pcm(&wav);
                total_pcm_ms += pcm.len() / 32;
                assert!(
                    detect(pcm, &phrase).expect("whole detect").is_none(),
                    "whole detector false-triggered for {path}"
                );
                let mut detector = StreamingDetector::new(&phrase).expect("streaming detector");
                for chunk in pcm.chunks(320) {
                    assert!(
                        detector
                            .accept_pcm(chunk)
                            .expect("streaming detect")
                            .is_none(),
                        "streaming detector false-triggered before terminal for {path}"
                    );
                }
                assert!(
                    detector
                        .finish()
                        .expect("finish streaming detect")
                        .is_none(),
                    "streaming detector false-triggered at terminal for {path}"
                );
            }
            println!("streaming_wake_negative_matrix total_pcm_ms={total_pcm_ms}");
        }

        /// A-desktop 诊断闭环: 扫描 LISTENER_WAKE_DIAG_DIR 下 wake_*.wav(应命中)
        /// 和 neg_*.wav(不应命中),逐个打印 streaming 命中。设 LISTENER_WAKE_THRESHOLD
        /// 可测调高阈值能否在保正样本命中的前提下压下"开始+两字"类误命中。
        #[test]
        #[ignore = "diagnostic: scan LISTENER_WAKE_DIAG_DIR wake_/neg_ wavs"]
        fn diagnostic_scan_negative_clips() {
            let phrase = std::env::var("LISTENER_WAKE_PHRASE")
                .unwrap_or_else(|_| "开始录音".to_string());
            let dir = std::env::var("LISTENER_WAKE_DIAG_DIR")
                .unwrap_or_else(|_| "target/wake_diag".to_string());
            let threshold = std::env::var("LISTENER_WAKE_THRESHOLD")
                .ok()
                .and_then(|t| t.parse::<f32>().ok());
            // score 固定为产品 BOOTSTRAP_KEYWORD_SCORE=3.0,只扫 threshold,保证数据点可比;
            // baseline(threshold 未设)用 BOOTSTRAP_KEYWORD_THRESHOLD=0.08 对齐产品默认。
            let effective_threshold = threshold.unwrap_or(0.08);
            let make_detector = || -> Result<StreamingDetector, String> {
                StreamingDetector::new_with_config(&phrase, 3.0, effective_threshold, false)
            };
            let mut wake_total = 0usize;
            let mut wake_hit = 0usize;
            let mut neg_total = 0usize;
            let mut neg_hit = 0usize;
            for entry in fs::read_dir(&dir).expect("diag dir") {
                let path = entry.expect("entry").path();
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let is_wake = name.starts_with("wake_");
                let is_neg = name.starts_with("neg_");
                if (!is_wake && !is_neg) || path.extension().and_then(|e| e.to_str()) != Some("wav") {
                    continue;
                }
                let wav = fs::read(&path).expect("wav");
                let pcm = wav_pcm(&wav);
                let mut streaming_hit = false;
                let mut detector = make_detector().expect("detector");
                for chunk in pcm.chunks(320) {
                    if detector.accept_pcm(chunk).expect("accept").is_some() {
                        streaming_hit = true;
                        break;
                    }
                }
                if is_wake {
                    wake_total += 1;
                    if streaming_hit {
                        wake_hit += 1;
                    }
                } else {
                    neg_total += 1;
                    if streaming_hit {
                        neg_hit += 1;
                    }
                }
                println!(
                    "diag {} kind={} streaming_hit={} pcm_ms={} threshold={:?}",
                    name,
                    if is_wake { "wake" } else { "neg" },
                    streaming_hit,
                    pcm.len() / 32,
                    threshold
                );
            }
            println!(
                "summary wake_hit={}/{} neg_false_trigger={}/{} threshold={:?}",
                wake_hit, wake_total, neg_hit, neg_total, threshold
            );
        }

        /// A-desktop 可行性的代码级保证:new_strict 对完整唤醒词命中,
        /// 对"开始录像/录入/路演"近似音不命中(TTS 数据,真人待实测)。
        #[test]
        #[ignore = "diagnostic: needs LISTENER_WAKE_DIAG_DIR wavs"]
        fn strict_detector_suppresses_prefix_variant_false_triggers() {
            let phrase = std::env::var("LISTENER_WAKE_PHRASE")
                .unwrap_or_else(|_| "开始录音".to_string());
            let dir = std::env::var("LISTENER_WAKE_DIAG_DIR")
                .unwrap_or_else(|_| "target/wake_diag".to_string());
            let wake_wav = fs::read(format!("{}/wake_huihui.wav", dir)).expect("wake wav");
            let wake_pcm = wav_pcm(&wake_wav);
            let mut wake_hit = false;
            let mut wake_det = StreamingDetector::new_strict(&phrase).expect("strict detector");
            for chunk in wake_pcm.chunks(320) {
                if wake_det.accept_pcm(chunk).expect("accept").is_some() {
                    wake_hit = true;
                    break;
                }
            }
            assert!(wake_hit, "new_strict 必须识别完整唤醒词 {}", phrase);
            for name in &["neg_kaishi_luxiang", "neg_kaishi_luru", "neg_kaishi_luyan"] {
                let path = format!("{}/{}.wav", dir, name);
                if !std::path::Path::new(&path).exists() {
                    continue;
                }
                let neg_wav = fs::read(&path).expect("neg wav");
                let neg_pcm = wav_pcm(&neg_wav);
                let mut det = StreamingDetector::new_strict(&phrase).expect("strict detector");
                let mut hit = false;
                for chunk in neg_pcm.chunks(320) {
                    if det.accept_pcm(chunk).expect("accept").is_some() {
                        hit = true;
                        break;
                    }
                }
                assert!(!hit, "new_strict 对近似音 {} 不应命中", name);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use platform::*;

#[cfg(not(target_os = "windows"))]
pub fn detect(_pcm: &[u8], _phrase: &str) -> Result<Option<Match>, String> {
    Err("当前平台暂不支持本地唤醒词".into())
}

#[cfg(not(target_os = "windows"))]
pub fn detect_with_recall_cascade(_pcm: &[u8], _phrase: &str) -> Result<Option<Match>, String> {
    Err("当前平台暂不支持本地唤醒词".into())
}

#[cfg(not(target_os = "windows"))]
pub struct StreamingDetector;

#[cfg(not(target_os = "windows"))]
impl StreamingDetector {
    pub fn new(_phrase: &str) -> Result<Self, String> {
        Err("当前平台暂不支持本地唤醒词".into())
    }

    pub fn accept_pcm(&mut self, _pcm: &[u8]) -> Result<Option<Match>, String> {
        Err("当前平台暂不支持本地唤醒词".into())
    }

    pub fn finish(&mut self) -> Result<Option<Match>, String> {
        Err("当前平台暂不支持本地唤醒词".into())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn calibrate(_pcm: &[u8], _phrase: &str) -> Result<(), String> {
    Err("当前平台暂不支持本地唤醒词".into())
}

#[cfg(not(target_os = "windows"))]
pub fn persist_bootstrap_calibration_if_missing(_phrase: &str) -> Result<bool, String> {
    Err("当前平台暂不支持本地唤醒词".into())
}

#[cfg(not(target_os = "windows"))]
pub fn prepare(_phrase: &str) -> Result<(), String> {
    Err("当前平台暂不支持本地唤醒词".into())
}
