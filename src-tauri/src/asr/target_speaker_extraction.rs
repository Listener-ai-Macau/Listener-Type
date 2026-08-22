//! Local target-speaker extraction for enrolled-owner dictation.
//!
//! The cloud transcript remains the low-latency preview source. This module
//! removes physically overlapping non-owner speech from fixed three-second
//! chunks before a second, authoritative ASR stream sees them. Identity
//! gating alone cannot do that: once two voices share the same samples, a
//! diarizer can label the mixture but cannot remove the unwanted words.

use crate::asr::AudioConsumer;
use kaldi_native_fbank::{
    istft_compute,
    online::{FeatureComputer, OnlineFeature},
    stft_compute, FbankComputer, FbankOptions, IstftOptions, StftOptions, StftResult,
};
use once_cell::sync::OnceCell;
use ort::{session::Session, value::TensorRef};
use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::sync::mpsc;

pub(crate) const SAMPLE_RATE: usize = 16_000;
pub(crate) const CHUNK_SAMPLES: usize = SAMPLE_RATE * 3;
pub(crate) const CHUNK_BYTES: usize = CHUNK_SAMPLES * 2;
const FFT_SIZE: usize = 512;
const HOP_SAMPLES: usize = 128;
const FFT_BINS: usize = FFT_SIZE / 2 + 1;
const STFT_FRAMES: usize = 376;
const ENROLLMENT_FRAMES: usize = 300;
const FBANK_BINS: usize = 80;
pub(crate) const SPEAKER_EMBEDDING_DIMENSION: usize = 192;
const STRONG_INTERFERENCE_RESIDUAL_RATIO: f64 = 0.03;
const SEPARATOR_MODEL_FILE_NAME: &str = "wesep-bsrnn-voicefilter-3s-int8.onnx";
const ENCODER_MODEL_FILE_NAME: &str = "wesep-speaker-encoder-int8.onnx";
pub(crate) const SEPARATOR_MODEL_SHA256: &str =
    "1BFA3C60EA58288DE6947C62D6A49FBA9AEEE20BFBCE0B0415504F506F3F20B4";
pub(crate) const ENCODER_MODEL_SHA256: &str =
    "9CEC30564E3A87746BDD44108779EDDF5323CD9ED4BD0966B64883A6EEBBF46E";

static ENCODER_SESSION: OnceCell<Mutex<Session>> = OnceCell::new();
static SEPARATOR_SESSION: OnceCell<Mutex<Session>> = OnceCell::new();

fn ensure_ort_environment() -> Result<(), String> {
    static ENVIRONMENT: OnceCell<()> = OnceCell::new();
    ENVIRONMENT
        .get_or_try_init(|| {
            let runtime = std::env::var("LISTENER_ONNX_RUNTIME_DLL")
                .ok()
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_file())
                .map(Ok)
                .unwrap_or_else(crate::wake_phrase::onnx_runtime_dll_path)?;
            let builder = ort::init_from(&runtime).map_err(|err| {
                format!(
                    "load target-speaker ONNX Runtime {} failed: {err}",
                    runtime.display()
                )
            })?;
            let _ = builder.with_name("listener-target-speaker").commit();
            Ok::<(), String>(())
        })
        .map(|_| ())
}

fn load_session(
    slot: &'static OnceCell<Mutex<Session>>,
    environment_variable: &str,
    model_file_name: &str,
    label: &str,
) -> Result<&'static Mutex<Session>, String> {
    slot.get_or_try_init(|| {
        ensure_ort_environment()?;
        let model = model_path(environment_variable, model_file_name)?;
        let session = Session::builder()
            .map_err(|err| format!("build target-speaker {label} session failed: {err}"))?
            .with_intra_threads(8)
            .map_err(|err| format!("configure target-speaker {label} threads failed: {err}"))?
            .with_inter_threads(1)
            .map_err(|err| format!("configure target-speaker {label} scheduler failed: {err}"))?
            .commit_from_file(&model)
            .map_err(|err| {
                format!(
                    "load target-speaker {label} model {} failed: {err}",
                    model.display()
                )
            })?;
        Ok(Mutex::new(session))
    })
}

fn encoder_session() -> Result<&'static Mutex<Session>, String> {
    load_session(
        &ENCODER_SESSION,
        "LISTENER_TARGET_SPEAKER_ENCODER_MODEL",
        ENCODER_MODEL_FILE_NAME,
        "encoder",
    )
}

fn separator_session() -> Result<&'static Mutex<Session>, String> {
    load_session(
        &SEPARATOR_SESSION,
        "LISTENER_TARGET_SPEAKER_MODEL",
        SEPARATOR_MODEL_FILE_NAME,
        "separator",
    )
}

pub(crate) fn warm_up() -> Result<(), String> {
    encoder_session()?;
    separator_session().map(|_| ())
}

fn model_path(
    environment_variable: &str,
    model_file_name: &str,
) -> Result<std::path::PathBuf, String> {
    if let Ok(path) = std::env::var(environment_variable) {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    let development = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources/models")
        .join(model_file_name);
    if development.is_file() {
        return Ok(development);
    }
    let executable = std::env::current_exe()
        .map_err(|err| format!("resolve target-speaker executable path failed: {err}"))?;
    let root = executable
        .parent()
        .ok_or_else(|| "target-speaker executable has no parent directory".to_string())?;
    for candidate in [
        root.join("resources/models").join(model_file_name),
        root.join("models").join(model_file_name),
        root.join(model_file_name),
    ] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "target-speaker model is missing beside {}",
        executable.display()
    ))
}

fn pcm16le_to_f32(pcm: &[u8], scale: f32) -> Vec<f32> {
    pcm.chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 * scale)
        .collect()
}

fn enrollment_fbank(pcm: &[u8]) -> Result<Vec<f32>, String> {
    if pcm.len() < SAMPLE_RATE {
        return Err("target-speaker enrollment is too short".to_string());
    }
    let mut opts = FbankOptions::default();
    opts.frame_opts.samp_freq = SAMPLE_RATE as f32;
    opts.frame_opts.frame_length_ms = 25.0;
    opts.frame_opts.frame_shift_ms = 10.0;
    opts.frame_opts.dither = 0.0;
    opts.frame_opts.preemph_coeff = 0.97;
    opts.frame_opts.remove_dc_offset = true;
    opts.frame_opts.window_type = "hamming".to_string();
    opts.frame_opts.round_to_power_of_two = true;
    opts.frame_opts.snip_edges = true;
    opts.mel_opts.num_bins = FBANK_BINS;
    opts.mel_opts.low_freq = 20.0;
    opts.mel_opts.high_freq = 0.0;
    opts.use_energy = false;
    opts.raw_energy = true;
    opts.use_log_fbank = true;
    opts.use_power = true;

    let computer = FbankComputer::new(opts)
        .map_err(|err| format!("create target-speaker fbank failed: {err}"))?;
    let mut online = OnlineFeature::new(FeatureComputer::Fbank(computer));
    // torchaudio.compliance.kaldi.fbank receives the conventional int16-scale
    // waveform even though its tensor element type is float.
    let samples = pcm16le_to_f32(pcm, 1.0);
    online.accept_waveform(SAMPLE_RATE as f32, &samples);
    online.input_finished();
    let frame_count = online.features.len().min(ENROLLMENT_FRAMES);
    if frame_count == 0 {
        return Err("target-speaker enrollment produced no fbank frames".to_string());
    }

    let mut means = [0.0f32; FBANK_BINS];
    for frame in online.features.iter().take(frame_count) {
        for (mean, value) in means.iter_mut().zip(frame) {
            *mean += *value;
        }
    }
    for mean in &mut means {
        *mean /= frame_count as f32;
    }

    let mut output = vec![0.0f32; ENROLLMENT_FRAMES * FBANK_BINS];
    for (frame_index, frame) in online.features.iter().take(frame_count).enumerate() {
        for bin in 0..FBANK_BINS {
            output[frame_index * FBANK_BINS + bin] = frame[bin] - means[bin];
        }
    }
    Ok(output)
}

fn stft_options() -> StftOptions {
    let mut opts = StftOptions::default();
    opts.frame_opts.window_type = "hann".to_string();
    opts.n_fft = FFT_SIZE;
    opts.hop_length = HOP_SAMPLES;
    opts.win_length = FFT_SIZE;
    opts.center = true;
    opts.pad_mode = "reflect".to_string();
    opts.normalized = false;
    opts
}

fn stft_to_model_input(stft: &StftResult) -> Result<Vec<f32>, String> {
    if stft.n_fft != FFT_SIZE || stft.num_frames != STFT_FRAMES {
        return Err(format!(
            "unexpected target-speaker STFT shape: bins={} frames={}",
            stft.n_fft / 2 + 1,
            stft.num_frames
        ));
    }
    let plane = FFT_BINS * STFT_FRAMES;
    let mut input = vec![0.0f32; plane * 2];
    for frame in 0..STFT_FRAMES {
        for bin in 0..FFT_BINS {
            let source = frame * FFT_BINS + bin;
            let destination = bin * STFT_FRAMES + frame;
            input[destination] = stft.real[source];
            input[plane + destination] = stft.imag[source];
        }
    }
    Ok(input)
}

fn model_output_to_stft(output: &[f32]) -> Result<StftResult, String> {
    let plane = FFT_BINS * STFT_FRAMES;
    if output.len() != plane * 2 {
        return Err(format!(
            "unexpected target-speaker output elements: {}",
            output.len()
        ));
    }
    let mut real = vec![0.0f32; plane];
    let mut imag = vec![0.0f32; plane];
    for frame in 0..STFT_FRAMES {
        for bin in 0..FFT_BINS {
            let source = bin * STFT_FRAMES + frame;
            let destination = frame * FFT_BINS + bin;
            real[destination] = output[source];
            imag[destination] = output[plane + source];
        }
    }
    Ok(StftResult {
        real,
        imag,
        num_frames: STFT_FRAMES,
        n_fft: FFT_SIZE,
    })
}

fn samples_to_pcm(samples: &[f32], valid_samples: usize) -> Vec<u8> {
    samples
        .iter()
        .take(valid_samples)
        .flat_map(|sample| {
            let value = (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
            value.to_le_bytes()
        })
        .collect()
}

pub(crate) struct TargetSpeakerExtractor {
    speaker_embedding: Vec<f32>,
}

pub(crate) struct ExtractedChunk {
    pub(crate) pcm: Vec<u8>,
    pub(crate) residual_energy: f64,
    pub(crate) mixture_energy: f64,
}

impl TargetSpeakerExtractor {
    pub(crate) fn new(enrollment_pcm: &[u8]) -> Result<Self, String> {
        Self::from_embedding(speaker_embedding_from_enrollment_pcm(enrollment_pcm)?)
    }

    pub(crate) fn from_embedding(speaker_embedding: Vec<f32>) -> Result<Self, String> {
        if speaker_embedding.len() != SPEAKER_EMBEDDING_DIMENSION
            || speaker_embedding.iter().any(|value| !value.is_finite())
        {
            return Err(format!(
                "invalid target-speaker embedding dimension={} expected={SPEAKER_EMBEDDING_DIMENSION}",
                speaker_embedding.len()
            ));
        }
        Ok(Self { speaker_embedding })
    }

    pub(crate) fn extract_chunk(&self, pcm: &[u8]) -> Result<Vec<u8>, String> {
        self.extract_chunk_with_metrics(pcm)
            .map(|result| result.pcm)
    }

    pub(crate) fn extract_chunk_with_metrics(&self, pcm: &[u8]) -> Result<ExtractedChunk, String> {
        if pcm.is_empty() || pcm.len() > CHUNK_BYTES || pcm.len() % 2 != 0 {
            return Err(format!(
                "invalid target-speaker PCM chunk bytes={}",
                pcm.len()
            ));
        }
        let valid_samples = pcm.len() / 2;
        let mut samples = pcm16le_to_f32(pcm, 1.0 / 32_768.0);
        samples.resize(CHUNK_SAMPLES, 0.0);
        let stft = stft_compute(&stft_options(), &samples)
            .map_err(|err| format!("target-speaker STFT failed: {err}"))?;
        let mix_stft = stft_to_model_input(&stft)?;
        let mix_tensor = TensorRef::from_array_view((
            [1usize, 2usize, FFT_BINS, STFT_FRAMES],
            mix_stft.as_slice(),
        ))
        .map_err(|err| format!("create target-speaker mixture tensor failed: {err}"))?;
        let speaker_tensor = TensorRef::from_array_view((
            [1usize, SPEAKER_EMBEDDING_DIMENSION],
            self.speaker_embedding.as_slice(),
        ))
        .map_err(|err| format!("create target-speaker embedding tensor failed: {err}"))?;
        let started = Instant::now();
        let output = {
            let mut session = separator_session()?.lock();
            let outputs = session
                .run(ort::inputs![
                    "mix_stft_ri" => mix_tensor,
                    "speaker_logits" => speaker_tensor
                ])
                .map_err(|err| format!("run target-speaker model failed: {err}"))?;
            let (shape, values) = outputs["target_stft_ri"]
                .try_extract_tensor::<f32>()
                .map_err(|err| format!("decode target-speaker output failed: {err}"))?;
            if shape.as_ref() != [1_i64, 2_i64, FFT_BINS as i64, STFT_FRAMES as i64] {
                return Err(format!("unexpected target-speaker output shape: {shape:?}"));
            }
            values.to_vec()
        };
        let target_stft = model_output_to_stft(&output)?;
        let waveform = istft_compute(&IstftOptions::from(&stft_options()), &target_stft)
            .map_err(|err| format!("target-speaker ISTFT failed: {err}"))?;
        if waveform.len() < valid_samples {
            return Err(format!(
                "target-speaker ISTFT truncated: expected={valid_samples} actual={}",
                waveform.len()
            ));
        }
        log::info!(
            "[target-speaker] extracted chunk valid_ms={} inference_ms={}",
            valid_samples / 16,
            started.elapsed().as_millis()
        );
        let target = &waveform[..valid_samples];
        let mixture = &samples[..valid_samples];
        let target_energy = target
            .iter()
            .map(|sample| (*sample as f64).powi(2))
            .sum::<f64>();
        let mixture_energy = mixture
            .iter()
            .map(|sample| (*sample as f64).powi(2))
            .sum::<f64>();
        let dot = mixture
            .iter()
            .zip(target)
            .map(|(left, right)| *left as f64 * *right as f64)
            .sum::<f64>();
        // The checkpoint can return an arbitrary target scale/sign. Fit one
        // scalar before measuring what the owner estimate cannot explain.
        let gain = dot / target_energy.max(1e-12);
        let residual_energy = mixture
            .iter()
            .zip(target)
            .map(|(left, right)| (*left as f64 - gain * *right as f64).powi(2))
            .sum::<f64>();
        Ok(ExtractedChunk {
            pcm: samples_to_pcm(target, valid_samples),
            residual_energy,
            mixture_energy,
        })
    }
}

pub(crate) fn speaker_embedding_from_enrollment_pcm(pcm: &[u8]) -> Result<Vec<f32>, String> {
    let speaker_fbank = enrollment_fbank(pcm)?;
    let tensor = TensorRef::from_array_view((
        [1usize, ENROLLMENT_FRAMES, FBANK_BINS],
        speaker_fbank.as_slice(),
    ))
    .map_err(|err| format!("create target-speaker enrollment tensor failed: {err}"))?;
    let mut session = encoder_session()?.lock();
    let outputs = session
        .run(ort::inputs!["speaker_fbank" => tensor])
        .map_err(|err| format!("run target-speaker encoder failed: {err}"))?;
    let (shape, values) = outputs["speaker_logits"]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("decode target-speaker embedding failed: {err}"))?;
    if shape.as_ref() != [1_i64, SPEAKER_EMBEDDING_DIMENSION as i64] {
        return Err(format!(
            "unexpected target-speaker embedding shape: {shape:?}"
        ));
    }
    let embedding = values.to_vec();
    TargetSpeakerExtractor::from_embedding(embedding.clone())?;
    Ok(embedding)
}

pub(crate) fn wake_phrase_enrollment_pcm(wake_pcm: &[u8], wake_end_seconds: f32) -> Vec<u8> {
    let even_len = wake_pcm.len() & !1;
    let end = ((wake_end_seconds.max(0.0) * 32_000.0).round() as usize).min(even_len) & !1usize;
    let desired = SAMPLE_RATE * 2 * 2_200 / 1_000;
    let start = end.saturating_sub(desired) & !1usize;
    wake_pcm[start..end].to_vec()
}

/// A hidden authoritative stream fed only target-speaker-extracted audio.
/// The normal cloud stream keeps driving low-latency preview and endpointing;
/// this stream replaces only the final text when it completes successfully.
pub(crate) struct TargetSpeakerStream {
    audio_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    worker: Mutex<
        Option<tauri::async_runtime::JoinHandle<Result<Option<crate::asr::RawTranscript>, String>>>,
    >,
    interference_detected: Arc<AtomicBool>,
    secondary_asr: Arc<Mutex<Option<Arc<crate::asr::VolcengineStreamingASR>>>>,
}

async fn feed_extracted_chunk(
    secondary: &mut Option<Arc<crate::asr::VolcengineStreamingASR>>,
    staged_pcm: &mut Vec<u8>,
    credentials: &crate::asr::VolcengineCredentials,
    hotwords: &[crate::asr::DictionaryHotword],
    extracted: ExtractedChunk,
    residual_energy: &mut f64,
    mixture_energy: &mut f64,
    interference_detected: &AtomicBool,
    secondary_slot: &Mutex<Option<Arc<crate::asr::VolcengineStreamingASR>>>,
) -> Result<(), String> {
    *residual_energy += extracted.residual_energy;
    *mixture_energy += extracted.mixture_energy;
    if let Some(asr) = secondary.as_ref() {
        asr.consume_pcm_chunk(&extracted.pcm);
        return Ok(());
    }

    staged_pcm.extend_from_slice(&extracted.pcm);
    let residual_ratio = *residual_energy / mixture_energy.max(1e-12);
    if residual_ratio < STRONG_INTERFERENCE_RESIDUAL_RATIO {
        return Ok(());
    }

    interference_detected.store(true, Ordering::SeqCst);
    log::info!(
        "[target-speaker] strong interference detected during capture residual_ratio={residual_ratio:.6} threshold={STRONG_INTERFERENCE_RESIDUAL_RATIO:.6}; opening streaming extracted ASR"
    );
    let asr = Arc::new(crate::asr::VolcengineStreamingASR::new(
        credentials.clone(),
        hotwords.to_vec(),
    ));
    asr.open_session_for_deferred_audio()
        .await
        .map_err(|err| format!("open target-speaker ASR failed: {err}"))?;
    asr.mark_audio_delivery_ready();
    asr.consume_pcm_chunk(staged_pcm);
    staged_pcm.clear();
    *secondary_slot.lock() = Some(Arc::clone(&asr));
    *secondary = Some(asr);
    Ok(())
}

impl TargetSpeakerStream {
    pub(crate) fn start(
        credentials: crate::asr::VolcengineCredentials,
        hotwords: Vec<crate::asr::DictionaryHotword>,
        speaker_embedding: Vec<f32>,
    ) -> Arc<Self> {
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let interference_detected = Arc::new(AtomicBool::new(false));
        let worker_interference_detected = Arc::clone(&interference_detected);
        let secondary_asr = Arc::new(Mutex::new(None));
        let worker_secondary_asr = Arc::clone(&secondary_asr);
        let worker = tauri::async_runtime::spawn(async move {
            let extractor = tauri::async_runtime::spawn_blocking(move || {
                TargetSpeakerExtractor::from_embedding(speaker_embedding)
            })
            .await
            .map_err(|err| format!("target-speaker embedding task failed: {err}"))??;
            let extractor = Arc::new(extractor);
            let mut buffered = Vec::with_capacity(CHUNK_BYTES * 2);
            let mut staged_pcm = Vec::new();
            let mut residual_energy = 0.0f64;
            let mut mixture_energy = 0.0f64;
            let mut secondary = None;
            while let Some(pcm) = audio_rx.recv().await {
                buffered.extend_from_slice(&pcm);
                while buffered.len() >= CHUNK_BYTES {
                    let trailing = buffered.split_off(CHUNK_BYTES);
                    let chunk = std::mem::replace(&mut buffered, trailing);
                    let extractor = Arc::clone(&extractor);
                    let extracted = tauri::async_runtime::spawn_blocking(move || {
                        extractor.extract_chunk_with_metrics(&chunk)
                    })
                    .await
                    .map_err(|err| format!("target-speaker chunk task failed: {err}"))??;
                    feed_extracted_chunk(
                        &mut secondary,
                        &mut staged_pcm,
                        &credentials,
                        &hotwords,
                        extracted,
                        &mut residual_energy,
                        &mut mixture_energy,
                        &worker_interference_detected,
                        &worker_secondary_asr,
                    )
                    .await?;
                }
            }
            if !buffered.is_empty() {
                let extractor = Arc::clone(&extractor);
                let extracted = tauri::async_runtime::spawn_blocking(move || {
                    extractor.extract_chunk_with_metrics(&buffered)
                })
                .await
                .map_err(|err| format!("target-speaker tail task failed: {err}"))??;
                feed_extracted_chunk(
                    &mut secondary,
                    &mut staged_pcm,
                    &credentials,
                    &hotwords,
                    extracted,
                    &mut residual_energy,
                    &mut mixture_energy,
                    &worker_interference_detected,
                    &worker_secondary_asr,
                )
                .await?;
            }
            let residual_ratio = residual_energy / mixture_energy.max(1e-12);
            if residual_ratio < STRONG_INTERFERENCE_RESIDUAL_RATIO {
                if let Some(asr) = secondary {
                    asr.cancel();
                }
                worker_secondary_asr.lock().take();
                log::info!(
                    "[target-speaker] clean owner stream kept on primary residual_ratio={residual_ratio:.6} threshold={STRONG_INTERFERENCE_RESIDUAL_RATIO:.6}"
                );
                return Ok(None);
            }
            let Some(secondary) = secondary else {
                return Err("strong target-speaker interference had no extracted ASR".to_string());
            };
            let result = match secondary.send_last_frame().await {
                Ok(()) => secondary
                    .await_final_result()
                    .await
                    .map(Some)
                    .map_err(|err| format!("await target-speaker ASR failed: {err}")),
                Err(err) => Err(format!("finalize target-speaker ASR failed: {err}")),
            };
            worker_secondary_asr.lock().take();
            result
        });
        Arc::new(Self {
            audio_tx: Mutex::new(Some(audio_tx)),
            worker: Mutex::new(Some(worker)),
            interference_detected,
            secondary_asr,
        })
    }

    pub(crate) fn interference_detected(&self) -> bool {
        self.interference_detected.load(Ordering::SeqCst)
    }

    pub(crate) fn consume_pcm_chunk(&self, pcm: &[u8]) {
        if pcm.is_empty() {
            return;
        }
        if let Some(tx) = self.audio_tx.lock().as_ref() {
            let _ = tx.send(pcm.to_vec());
        }
    }

    pub(crate) async fn finish(&self) -> Result<Option<crate::asr::RawTranscript>, String> {
        self.audio_tx.lock().take();
        let worker = self.worker.lock().take();
        let Some(worker) = worker else {
            return Err("target-speaker stream already finalized".to_string());
        };
        let result = match worker.await {
            Ok(result) => result,
            Err(err) => Err(format!("target-speaker worker failed: {err}")),
        };
        if let Some(asr) = self.secondary_asr.lock().take() {
            if result.is_err() {
                asr.cancel();
            }
        }
        result
    }

    pub(crate) fn cancel(&self) {
        self.audio_tx.lock().take();
        if let Some(worker) = self.worker.lock().take() {
            worker.abort();
        }
        if let Some(asr) = self.secondary_asr.lock().take() {
            asr.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_enrollment_is_bounded_to_phrase_tail() {
        let pcm = vec![1u8; SAMPLE_RATE * 2 * 5];
        let enrollment = wake_phrase_enrollment_pcm(&pcm, 4.0);
        assert_eq!(enrollment.len(), SAMPLE_RATE * 2 * 2_200 / 1_000);
    }

    #[test]
    fn stft_shape_matches_exported_model_contract() {
        let waveform = vec![0.0f32; CHUNK_SAMPLES];
        let stft = stft_compute(&stft_options(), &waveform).unwrap();
        assert_eq!(stft.n_fft / 2 + 1, FFT_BINS);
        assert_eq!(stft.num_frames, STFT_FRAMES);
    }

    #[test]
    #[ignore = "real ONNX inference against the public overlap fixture"]
    fn real_overlap_matches_python_reference() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.artifacts/research/target-filter-real");
        let owner = std::fs::read(root.join("owner.pcm")).unwrap();
        let mix = std::fs::read(root.join("mix.pcm")).unwrap();
        let reference =
            std::fs::read(root.join("mix-wesep-onnx-core-3s-int8-aligned.pcm")).unwrap();
        // Production reaches this path only after local KWS has loaded the
        // shared runtime. Mirror that order in the standalone diagnostic.
        let _ = crate::wake_phrase::detect(&owner, "开始录音");
        ensure_ort_environment().unwrap();
        let extractor = TargetSpeakerExtractor::new(&owner).unwrap();
        let mut actual = Vec::new();
        let mut residual_energy = 0.0;
        let mut mixture_energy = 0.0;
        for chunk in mix[..CHUNK_BYTES * 2].chunks(CHUNK_BYTES) {
            let extracted = extractor.extract_chunk_with_metrics(chunk).unwrap();
            actual.extend(extracted.pcm);
            residual_energy += extracted.residual_energy;
            mixture_energy += extracted.mixture_energy;
        }
        assert_eq!(actual.len(), reference.len());

        let actual = pcm16le_to_f32(&actual, 1.0);
        let reference = pcm16le_to_f32(&reference, 1.0);
        let dot: f64 = actual
            .iter()
            .zip(&reference)
            .map(|(left, right)| *left as f64 * *right as f64)
            .sum();
        let actual_energy: f64 = actual.iter().map(|value| (*value as f64).powi(2)).sum();
        let reference_energy: f64 = reference.iter().map(|value| (*value as f64).powi(2)).sum();
        let correlation = dot / (actual_energy * reference_energy).sqrt();
        let residual_ratio = residual_energy / mixture_energy;
        eprintln!(
            "target_speaker_python_reference_correlation={correlation:.6} residual_ratio={residual_ratio:.6}"
        );
        assert!(
            correlation >= 0.98,
            "Rust preprocessing diverged from Python reference: correlation={correlation:.6}"
        );
        assert!(residual_ratio >= STRONG_INTERFERENCE_RESIDUAL_RATIO);
    }

    #[test]
    #[ignore = "real ONNX inference against the public clean-owner fixture"]
    fn real_clean_owner_does_not_open_second_cloud_stream() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.artifacts/research/target-filter-real");
        let owner = std::fs::read(root.join("owner.pcm")).unwrap();
        let _ = crate::wake_phrase::detect(&owner, "开始录音");
        ensure_ort_environment().unwrap();
        let extractor = TargetSpeakerExtractor::new(&owner).unwrap();
        let extracted = extractor
            .extract_chunk_with_metrics(&owner[..CHUNK_BYTES])
            .unwrap();
        let residual_ratio = extracted.residual_energy / extracted.mixture_energy;
        eprintln!("target_speaker_clean_owner_residual_ratio={residual_ratio:.6}");
        assert!(
            residual_ratio < STRONG_INTERFERENCE_RESIDUAL_RATIO,
            "clean owner must preserve the one-cloud-session baseline"
        );
    }
}
