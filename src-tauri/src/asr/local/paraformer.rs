#[cfg(target_os = "windows")]
mod imp {
    use libloading::Library;
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::fs;
    use std::io::{BufReader, Read, Write};
    use std::path::Path;

    const SAMPLE_RATE: i32 = 16_000;
    const MODEL_ARCHIVE: &str = "sherpa-onnx-paraformer-zh-small-2024-03-09.tar.bz2";
    const MODEL_DIR: &str = "sherpa-onnx-paraformer-zh-small-2024-03-09";
    const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-paraformer-zh-small-2024-03-09.tar.bz2";
    const ARCHIVE_SHA256: &str = "DA92B3DB5218C5BE53AAD53E57D1B6E63E7FC98A0E054FBDD6DBE18E9C6B1450";
    const MODEL_SHA256: &str = "3EF6C19369B912F7CAF3CEF8E545C5CCD1A33D9D7EC792A46668DC41C4B229EC";
    const TOKENS_SHA256: &str = "4B2D964E18B9CF139B473003B6698FB2ED9A2A5EC55B93DAA677B28F578897AA";

    // 单麦场景下唤醒鲁棒性的关键:候选 WAV 转写前先做离线语音增强(GTCRN,
    // ~48K 参数,开销可忽略),把电视/环境底噪压下去,paraformer 才转得出"开始录音"。
    // 这是 OR 门里 KWS 漏掉时的兜底路径(见 dictation.rs 本地确认)。严格可选:
    // 模型或符号不可用时回退到原始音频,绝不阻塞唤醒。
    const DENOISER_MODEL: &str = "gtcrn_simple.onnx";
    const DENOISER_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/gtcrn_simple.onnx";
    const DENOISER_SHA256: &str =
        "E77603AC0C23DAC3227DD2D7135B3A585CBEE2679048AECFA886657D3AE1B534";

    #[repr(C)]
    struct FeatureConfig {
        sample_rate: i32,
        feature_dim: i32,
    }

    #[repr(C)]
    struct TransducerConfig {
        encoder: *const c_char,
        decoder: *const c_char,
        joiner: *const c_char,
    }

    #[repr(C)]
    struct ParaformerConfig {
        model: *const c_char,
    }

    #[repr(C)]
    struct SingleModelConfig {
        model: *const c_char,
    }

    #[repr(C)]
    struct WhisperConfig {
        encoder: *const c_char,
        decoder: *const c_char,
        language: *const c_char,
        task: *const c_char,
        tail_paddings: i32,
        enable_token_timestamps: i32,
        enable_segment_timestamps: i32,
    }

    #[repr(C)]
    struct SenseVoiceConfig {
        model: *const c_char,
        language: *const c_char,
        use_itn: i32,
    }

    #[repr(C)]
    struct MoonshineConfig {
        preprocessor: *const c_char,
        encoder: *const c_char,
        uncached_decoder: *const c_char,
        cached_decoder: *const c_char,
        merged_decoder: *const c_char,
    }

    #[repr(C)]
    struct TwoModelConfig {
        encoder: *const c_char,
        decoder: *const c_char,
    }

    #[repr(C)]
    struct CanaryConfig {
        encoder: *const c_char,
        decoder: *const c_char,
        src_lang: *const c_char,
        tgt_lang: *const c_char,
        use_pnc: i32,
    }

    #[repr(C)]
    struct FunAsrNanoConfig {
        encoder_adaptor: *const c_char,
        llm: *const c_char,
        embedding: *const c_char,
        tokenizer: *const c_char,
        system_prompt: *const c_char,
        user_prompt: *const c_char,
        max_new_tokens: i32,
        temperature: f32,
        top_p: f32,
        seed: i32,
        language: *const c_char,
        itn: i32,
        hotwords: *const c_char,
    }

    #[repr(C)]
    struct Qwen3AsrConfig {
        conv_frontend: *const c_char,
        encoder: *const c_char,
        decoder: *const c_char,
        tokenizer: *const c_char,
        max_total_len: i32,
        max_new_tokens: i32,
        temperature: f32,
        top_p: f32,
        seed: i32,
        hotwords: *const c_char,
    }

    #[repr(C)]
    struct CohereConfig {
        encoder: *const c_char,
        decoder: *const c_char,
        language: *const c_char,
        use_punct: i32,
        use_itn: i32,
    }

    #[repr(C)]
    struct OfflineModelConfig {
        transducer: TransducerConfig,
        paraformer: ParaformerConfig,
        nemo_ctc: SingleModelConfig,
        whisper: WhisperConfig,
        tdnn: SingleModelConfig,
        tokens: *const c_char,
        num_threads: i32,
        debug: i32,
        provider: *const c_char,
        model_type: *const c_char,
        modeling_unit: *const c_char,
        bpe_vocab: *const c_char,
        telespeech_ctc: *const c_char,
        sense_voice: SenseVoiceConfig,
        moonshine: MoonshineConfig,
        fire_red_asr: TwoModelConfig,
        dolphin: SingleModelConfig,
        zipformer_ctc: SingleModelConfig,
        canary: CanaryConfig,
        wenet_ctc: SingleModelConfig,
        omnilingual: SingleModelConfig,
        medasr: SingleModelConfig,
        funasr_nano: FunAsrNanoConfig,
        fire_red_asr_ctc: SingleModelConfig,
        qwen3_asr: Qwen3AsrConfig,
        cohere_transcribe: CohereConfig,
    }

    #[repr(C)]
    struct LanguageModelConfig {
        model: *const c_char,
        scale: f32,
    }

    #[repr(C)]
    struct HomophoneReplacerConfig {
        dict_dir: *const c_char,
        lexicon: *const c_char,
        rule_fsts: *const c_char,
    }

    #[repr(C)]
    struct OfflineRecognizerConfig {
        feat_config: FeatureConfig,
        model_config: OfflineModelConfig,
        lm_config: LanguageModelConfig,
        decoding_method: *const c_char,
        max_active_paths: i32,
        hotwords_file: *const c_char,
        hotwords_score: f32,
        rule_fsts: *const c_char,
        rule_fars: *const c_char,
        blank_penalty: f32,
        hr: HomophoneReplacerConfig,
    }

    // --- sherpa-onnx v1.13.1 离线语音增强 C API (c-api.h 4015-4131) ---
    // 字段顺序/对齐必须与头文件逐字段一致(见 FFI 布局测试)。
    #[repr(C)]
    struct OfflineSpeechDenoiserGtcrnModelConfig {
        model: *const c_char,
    }
    #[repr(C)]
    struct OfflineSpeechDenoiserDpdfNetModelConfig {
        model: *const c_char,
    }
    #[repr(C)]
    struct OfflineSpeechDenoiserModelConfig {
        gtcrn: OfflineSpeechDenoiserGtcrnModelConfig,
        num_threads: i32,
        debug: i32,
        provider: *const c_char,
        dpdfnet: OfflineSpeechDenoiserDpdfNetModelConfig,
    }
    #[repr(C)]
    struct OfflineSpeechDenoiserConfig {
        model: OfflineSpeechDenoiserModelConfig,
    }
    #[repr(C)]
    struct DenoisedAudio {
        samples: *const f32,
        n: i32,
        sample_rate: i32,
    }

    type CreateRecognizer = unsafe extern "C" fn(*const OfflineRecognizerConfig) -> *const c_void;
    type DestroyRecognizer = unsafe extern "C" fn(*const c_void);
    type CreateStream = unsafe extern "C" fn(*const c_void) -> *const c_void;
    type DestroyStream = unsafe extern "C" fn(*const c_void);
    type AcceptWaveform = unsafe extern "C" fn(*const c_void, i32, *const f32, i32);
    type DecodeStream = unsafe extern "C" fn(*const c_void, *const c_void);
    type GetResultJson = unsafe extern "C" fn(*const c_void) -> *const c_char;
    type DestroyResultJson = unsafe extern "C" fn(*const c_char);
    // Denoiser 用裸指针存储,在 helper 单线程请求循环中使用(与 recognizer 同生命周期)。
    type CreateDenoiser = unsafe extern "C" fn(*const OfflineSpeechDenoiserConfig) -> *const c_void;
    type DestroyDenoiser = unsafe extern "C" fn(*const c_void);
    type DenoiserRun =
        unsafe extern "C" fn(*const c_void, *const f32, i32, i32) -> *const DenoisedAudio;
    type DestroyDenoisedAudio = unsafe extern "C" fn(*const DenoisedAudio);

    #[derive(Deserialize)]
    struct RecognitionResult {
        text: String,
    }

    // 降噪器句柄 + 它需要的功能指针打包在一起;整体可选(None=降噪不可用)。
    // 裸指针在 helper 单线程请求循环里使用,不跨线程。
    struct DenoiserBundle {
        handle: *const c_void,
        run: DenoiserRun,
        destroy: DestroyDenoiser,
        destroy_audio: DestroyDenoisedAudio,
    }

    pub struct ParaformerRuntime {
        _onnx: Library,
        _providers: Library,
        _sherpa: Library,
        recognizer: *const c_void,
        create_stream: CreateStream,
        destroy_stream: DestroyStream,
        accept_waveform: AcceptWaveform,
        decode_stream: DecodeStream,
        get_result_json: GetResultJson,
        destroy_result_json: DestroyResultJson,
        destroy_recognizer: DestroyRecognizer,
        denoiser: Option<DenoiserBundle>,
    }

    impl Drop for ParaformerRuntime {
        fn drop(&mut self) {
            unsafe {
                if let Some(denoiser) = self.denoiser.take() {
                    (denoiser.destroy)(denoiser.handle);
                }
                (self.destroy_recognizer)(self.recognizer);
            }
        }
    }

    impl ParaformerRuntime {
        pub fn load_cached() -> Result<Self, String> {
            let root = crate::persistence::speaker_verification_root()
                .map_err(|err| format!("resolve local speech model directory: {err}"))?;
            let model_dir = root.join(MODEL_DIR);
            let model_path = model_dir.join("model.int8.onnx");
            let tokens_path = model_dir.join("tokens.txt");
            verify_file(&model_path, MODEL_SHA256)?;
            verify_file(&tokens_path, TOKENS_SHA256)?;

            let model = path_cstring(&model_path, "Paraformer model")?;
            let tokens = path_cstring(&tokens_path, "Paraformer tokens")?;
            let provider = CString::new("cpu").expect("literal has no nul");
            let decoding = CString::new("greedy_search").expect("literal has no nul");
            let onnx_path = root.join("onnxruntime.dll");
            let providers_path = root.join("onnxruntime_providers_shared.dll");
            let sherpa_path = root.join("sherpa-onnx-c-api.dll");

            unsafe {
                let onnx = Library::new(&onnx_path)
                    .map_err(|err| format!("load local ASR onnxruntime failed: {err}"))?;
                let providers = Library::new(&providers_path)
                    .map_err(|err| format!("load local ASR providers failed: {err}"))?;
                let sherpa = Library::new(&sherpa_path)
                    .map_err(|err| format!("load local ASR C API failed: {err}"))?;
                let create_recognizer: CreateRecognizer = *sherpa
                    .get(b"SherpaOnnxCreateOfflineRecognizer\0")
                    .map_err(|err| format!("missing offline recognizer API: {err}"))?;
                let destroy_recognizer: DestroyRecognizer = *sherpa
                    .get(b"SherpaOnnxDestroyOfflineRecognizer\0")
                    .map_err(|err| format!("missing offline recognizer destroy API: {err}"))?;
                let create_stream: CreateStream =
                    *sherpa
                        .get(b"SherpaOnnxCreateOfflineStream\0")
                        .map_err(|err| format!("missing offline stream API: {err}"))?;
                let destroy_stream: DestroyStream = *sherpa
                    .get(b"SherpaOnnxDestroyOfflineStream\0")
                    .map_err(|err| format!("missing offline stream destroy API: {err}"))?;
                let accept_waveform: AcceptWaveform = *sherpa
                    .get(b"SherpaOnnxAcceptWaveformOffline\0")
                    .map_err(|err| format!("missing offline waveform API: {err}"))?;
                let decode_stream: DecodeStream =
                    *sherpa
                        .get(b"SherpaOnnxDecodeOfflineStream\0")
                        .map_err(|err| format!("missing offline decode API: {err}"))?;
                let get_result_json: GetResultJson = *sherpa
                    .get(b"SherpaOnnxGetOfflineStreamResultAsJson\0")
                    .map_err(|err| format!("missing offline result API: {err}"))?;
                let destroy_result_json: DestroyResultJson = *sherpa
                    .get(b"SherpaOnnxDestroyOfflineStreamResultJson\0")
                    .map_err(|err| format!("missing offline result destroy API: {err}"))?;

                let mut config: OfflineRecognizerConfig = std::mem::zeroed();
                config.feat_config.sample_rate = SAMPLE_RATE;
                config.feat_config.feature_dim = 80;
                config.model_config.paraformer.model = model.as_ptr();
                config.model_config.tokens = tokens.as_ptr();
                config.model_config.num_threads = 2;
                config.model_config.provider = provider.as_ptr();
                config.decoding_method = decoding.as_ptr();
                let recognizer = create_recognizer(&config);
                if recognizer.is_null() {
                    return Err("initialize cached Paraformer model failed".to_string());
                }
                // 语音增强器严格可选:模型文件缺失或符号不可用都返回 None,
                // transcribe_wav 会回退到原始音频,唤醒不依赖它。
                let denoiser = (|| -> Option<DenoiserBundle> {
                    let model_path = root.join(DENOISER_MODEL);
                    if !model_path.exists() {
                        log::info!(
                            "[paraformer] GTCRN denoiser model absent; wake confirmation runs without speech enhancement"
                        );
                        return None;
                    }
                    let create: CreateDenoiser = *sherpa
                        .get(b"SherpaOnnxCreateOfflineSpeechDenoiser\0")
                        .ok()?;
                    let destroy: DestroyDenoiser = *sherpa
                        .get(b"SherpaOnnxDestroyOfflineSpeechDenoiser\0")
                        .ok()?;
                    let run: DenoiserRun =
                        *sherpa.get(b"SherpaOnnxOfflineSpeechDenoiserRun\0").ok()?;
                    let destroy_audio: DestroyDenoisedAudio =
                        *sherpa.get(b"SherpaOnnxDestroyDenoisedAudio\0").ok()?;
                    let model_c = CString::new(model_path.to_string_lossy().as_bytes()).ok()?;
                    let provider_c = CString::new("cpu").expect("literal has no nul");
                    let config = OfflineSpeechDenoiserConfig {
                        model: OfflineSpeechDenoiserModelConfig {
                            gtcrn: OfflineSpeechDenoiserGtcrnModelConfig {
                                model: model_c.as_ptr(),
                            },
                            num_threads: 1,
                            debug: 0,
                            provider: provider_c.as_ptr(),
                            dpdfnet: OfflineSpeechDenoiserDpdfNetModelConfig {
                                model: std::ptr::null(),
                            },
                        },
                    };
                    let handle = create(&config);
                    if handle.is_null() {
                        log::warn!(
                            "[paraformer] GTCRN denoiser failed to initialize; wake confirmation runs without speech enhancement"
                        );
                        return None;
                    }
                    log::info!(
                        "[paraformer] GTCRN speech enhancer loaded for wake confirmation denoising"
                    );
                    Some(DenoiserBundle {
                        handle,
                        run,
                        destroy,
                        destroy_audio,
                    })
                })();
                Ok(Self {
                    _onnx: onnx,
                    _providers: providers,
                    _sherpa: sherpa,
                    recognizer,
                    create_stream,
                    destroy_stream,
                    accept_waveform,
                    decode_stream,
                    get_result_json,
                    destroy_result_json,
                    destroy_recognizer,
                    denoiser,
                })
            }
        }

        // 候选整段离线降噪;任何环节失败都回退原始样本,绝不阻断转写。
        fn denoise_samples(&self, samples: Vec<f32>) -> Vec<f32> {
            let Some(denoiser) = self.denoiser.as_ref() else {
                return samples;
            };
            let n = match i32::try_from(samples.len()) {
                Ok(n) if n > 0 => n,
                _ => return samples,
            };
            unsafe {
                let audio = (denoiser.run)(denoiser.handle, samples.as_ptr(), n, SAMPLE_RATE);
                if audio.is_null() {
                    return samples;
                }
                let out = &*audio;
                let count = out.n;
                let ptr = out.samples;
                // 必须在 destroy 之前把数据拷出来(它释放的就是这片内存)。
                let denoised = if ptr.is_null() || count <= 0 {
                    None
                } else {
                    Some(std::slice::from_raw_parts(ptr, count as usize).to_vec())
                };
                (denoiser.destroy_audio)(audio);
                denoised.unwrap_or(samples)
            }
        }

        pub fn transcribe_wav(&self, path: &Path) -> Result<String, String> {
            let wav = fs::read(path)
                .map_err(|err| format!("read local wake confirmation WAV failed: {err}"))?;
            let pcm = denzic_audio_v1_core::read_wav_pcm16le(&wav)
                .map_err(|err| format!("decode local wake confirmation WAV failed: {err}"))?;
            let samples = pcm
                .chunks_exact(2)
                .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
                .collect::<Vec<_>>();
            // 候选降噪(见 denoise_samples),电视/底噪压下去 paraformer 才转得出唤醒词。
            let samples = self.denoise_samples(samples);
            let sample_count =
                i32::try_from(samples.len()).map_err(|_| "local wake candidate is too long")?;

            unsafe {
                let stream = (self.create_stream)(self.recognizer);
                if stream.is_null() {
                    return Err("create local wake confirmation stream failed".to_string());
                }
                let result = (|| {
                    (self.accept_waveform)(stream, SAMPLE_RATE, samples.as_ptr(), sample_count);
                    (self.decode_stream)(self.recognizer, stream);
                    let raw = (self.get_result_json)(stream);
                    if raw.is_null() {
                        return Err("local wake confirmation returned no result".to_string());
                    }
                    let json = CStr::from_ptr(raw).to_string_lossy().into_owned();
                    (self.destroy_result_json)(raw);
                    let result: RecognitionResult = serde_json::from_str(&json)
                        .map_err(|err| format!("decode local wake confirmation result: {err}"))?;
                    Ok(result.text)
                })();
                (self.destroy_stream)(stream);
                result
            }
        }
    }

    pub fn prepare_assets() -> Result<(), String> {
        crate::speaker_verification::prepare_runtime_assets()?;
        let root = crate::persistence::speaker_verification_root()
            .map_err(|err| format!("resolve local speech model directory: {err}"))?;
        // 降噪模型严格可选:下载/校验失败只记日志,不阻塞 Paraformer 资产就绪。
        let denoiser_path = root.join(DENOISER_MODEL);
        if !file_matches(&denoiser_path, DENOISER_SHA256) {
            if let Err(err) = download_verified(DENOISER_URL, &denoiser_path, DENOISER_SHA256) {
                log::warn!(
                    "[paraformer] optional GTCRN denoiser model unavailable; wake confirmation will run without speech enhancement: {err}"
                );
            }
        }
        let model_dir = root.join(MODEL_DIR);
        let model_path = model_dir.join("model.int8.onnx");
        let tokens_path = model_dir.join("tokens.txt");
        if file_matches(&model_path, MODEL_SHA256) && file_matches(&tokens_path, TOKENS_SHA256) {
            return Ok(());
        }

        let archive_path = root.join(MODEL_ARCHIVE);
        download_verified(MODEL_URL, &archive_path, ARCHIVE_SHA256)?;
        fs::create_dir_all(&model_dir)
            .map_err(|err| format!("create Paraformer model directory failed: {err}"))?;
        extract_model(&archive_path, &model_dir)?;
        verify_file(&model_path, MODEL_SHA256)?;
        verify_file(&tokens_path, TOKENS_SHA256)
    }

    fn path_cstring(path: &Path, label: &str) -> Result<CString, String> {
        CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| format!("{label} path contains an invalid character"))
    }

    fn sha256(path: &Path) -> Result<String, String> {
        let file = fs::File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|err| format!("hash {}: {err}", path.display()))?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(format!("{:X}", hasher.finalize()))
    }

    fn file_matches(path: &Path, expected: &str) -> bool {
        path.exists() && sha256(path).is_ok_and(|actual| actual == expected)
    }

    fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
        let actual = sha256(path)?;
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "local ASR component hash mismatch for {}: expected={expected} actual={actual}",
                path.display()
            ))
        }
    }

    fn download_verified(url: &str, destination: &Path, expected: &str) -> Result<(), String> {
        if file_matches(destination, expected) {
            return Ok(());
        }
        let temp = destination.with_extension("download");
        let mut response = reqwest::blocking::get(url)
            .map_err(|err| format!("download Paraformer model failed: {err}"))?
            .error_for_status()
            .map_err(|err| format!("Paraformer model download returned an error: {err}"))?;
        let mut output = fs::File::create(&temp)
            .map_err(|err| format!("create Paraformer model download failed: {err}"))?;
        std::io::copy(&mut response, &mut output)
            .map_err(|err| format!("write Paraformer model download failed: {err}"))?;
        output
            .flush()
            .map_err(|err| format!("flush Paraformer model download failed: {err}"))?;
        drop(output);
        verify_file(&temp, expected).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;
        if destination.exists() {
            fs::remove_file(destination)
                .map_err(|err| format!("replace Paraformer model archive failed: {err}"))?;
        }
        fs::rename(&temp, destination)
            .map_err(|err| format!("install Paraformer model archive failed: {err}"))
    }

    fn extract_model(archive_path: &Path, model_dir: &Path) -> Result<(), String> {
        let file = fs::File::open(archive_path)
            .map_err(|err| format!("open Paraformer model archive failed: {err}"))?;
        let decoder = bzip2::read::BzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        let mut extracted_model = false;
        let mut extracted_tokens = false;
        for entry in archive
            .entries()
            .map_err(|err| format!("read Paraformer model archive failed: {err}"))?
        {
            let mut entry =
                entry.map_err(|err| format!("read Paraformer model entry failed: {err}"))?;
            let path = entry
                .path()
                .map_err(|err| format!("read Paraformer model path failed: {err}"))?;
            let Some(name) = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if name == "model.int8.onnx" || name == "tokens.txt" {
                entry
                    .unpack(model_dir.join(&name))
                    .map_err(|err| format!("extract Paraformer {name} failed: {err}"))?;
                extracted_model |= name == "model.int8.onnx";
                extracted_tokens |= name == "tokens.txt";
            }
        }
        if extracted_model && extracted_tokens {
            Ok(())
        } else {
            Err("Paraformer archive is missing model.int8.onnx or tokens.txt".to_string())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{
            prepare_assets, CohereConfig, DenoisedAudio, FunAsrNanoConfig, OfflineModelConfig,
            OfflineRecognizerConfig, OfflineSpeechDenoiserConfig,
            OfflineSpeechDenoiserGtcrnModelConfig, OfflineSpeechDenoiserModelConfig,
            ParaformerRuntime, Qwen3AsrConfig, WhisperConfig,
        };

        #[test]
        fn sherpa_offline_ffi_layout_matches_v1_13_1_x64_header() {
            assert_eq!(std::mem::size_of::<WhisperConfig>(), 48);
            assert_eq!(std::mem::size_of::<FunAsrNanoConfig>(), 88);
            assert_eq!(std::mem::size_of::<Qwen3AsrConfig>(), 64);
            assert_eq!(std::mem::size_of::<CohereConfig>(), 32);
            assert_eq!(std::mem::size_of::<OfflineModelConfig>(), 504);
            assert_eq!(std::mem::size_of::<OfflineRecognizerConfig>(), 608);
            assert_eq!(std::mem::offset_of!(OfflineModelConfig, paraformer), 24);
            assert_eq!(std::mem::offset_of!(OfflineModelConfig, tokens), 96);
            assert_eq!(
                std::mem::offset_of!(OfflineModelConfig, cohere_transcribe),
                472
            );
            assert_eq!(
                std::mem::offset_of!(OfflineRecognizerConfig, model_config),
                8
            );
            assert_eq!(
                std::mem::offset_of!(OfflineRecognizerConfig, decoding_method),
                528
            );
            assert_eq!(std::mem::offset_of!(OfflineRecognizerConfig, hr), 584);
            // 离线语音增强 C API 布局(对齐 c-api.h v1.13.1,4015-4131)。
            assert_eq!(
                std::mem::size_of::<OfflineSpeechDenoiserGtcrnModelConfig>(),
                8
            );
            assert_eq!(std::mem::size_of::<OfflineSpeechDenoiserModelConfig>(), 32);
            assert_eq!(std::mem::size_of::<OfflineSpeechDenoiserConfig>(), 32);
            assert_eq!(std::mem::size_of::<DenoisedAudio>(), 16);
            assert_eq!(
                std::mem::offset_of!(OfflineSpeechDenoiserModelConfig, gtcrn),
                0
            );
            assert_eq!(
                std::mem::offset_of!(OfflineSpeechDenoiserModelConfig, num_threads),
                8
            );
            assert_eq!(
                std::mem::offset_of!(OfflineSpeechDenoiserModelConfig, debug),
                12
            );
            assert_eq!(
                std::mem::offset_of!(OfflineSpeechDenoiserModelConfig, provider),
                16
            );
            assert_eq!(
                std::mem::offset_of!(OfflineSpeechDenoiserModelConfig, dpdfnet),
                24
            );
            assert_eq!(std::mem::offset_of!(OfflineSpeechDenoiserConfig, model), 0);
            assert_eq!(std::mem::offset_of!(DenoisedAudio, samples), 0);
            assert_eq!(std::mem::offset_of!(DenoisedAudio, n), 8);
            assert_eq!(std::mem::offset_of!(DenoisedAudio, sample_rate), 12);
        }

        #[test]
        #[ignore = "downloads the verified Paraformer model and requires a consented WAV fixture"]
        fn cached_runtime_transcribes_real_wav() {
            let path =
                std::env::var("LISTENER_PARAFORMER_TEST_WAV").expect("fixture path environment");
            prepare_assets().expect("prepare verified Paraformer assets");
            let runtime = ParaformerRuntime::load_cached().expect("load cached Paraformer");
            let transcript = runtime
                .transcribe_wav(std::path::Path::new(&path))
                .expect("transcribe fixture");
            assert!(
                crate::wake_phrase::local_transcript_matches_phrase(&transcript, "开始录音"),
                "unexpected transcript: {transcript:?}"
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::{prepare_assets, ParaformerRuntime};
