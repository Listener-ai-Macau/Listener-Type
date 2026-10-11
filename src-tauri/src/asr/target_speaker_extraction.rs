//! Local target-speaker extraction for enrolled-owner dictation.
//!
//! The cloud transcript remains the low-latency preview source. This module
//! removes physically overlapping non-owner speech from fixed three-second
//! chunks before a second, authoritative ASR stream sees them. Identity
//! gating alone cannot do that: once two voices share the same samples, a
//! diarizer can label the mixture but cannot remove the unwanted words.

use crate::asr::AudioConsumer;
use crate::polish::ProviderProxyConfig;
use kaldi_native_fbank::{
    istft_compute,
    online::{FeatureComputer, OnlineFeature},
    stft_compute, FbankComputer, FbankOptions, IstftOptions, StftOptions, StftResult,
};
use once_cell::sync::OnceCell;
use ort::{
    session::{RunOptions, Session},
    value::TensorRef,
};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
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
// The enrolled session profile is derived beside the separator worker. Model
// scheduling can let the separator consume its first source chunks before the
// profile callback arrives. Retain only this bounded prefix for later source
// identity observation; separator/ASR delivery never waits for the profile and
// the rest of the recording is never accumulated here.
const SOURCE_PROFILE_PENDING_MAX_CHUNKS: usize = 2;
// The separated waveform has scale/sign ambiguity. Restore it to the mixture
// projection before ASR, but keep a hard ceiling so a nearly-silent target
// estimate cannot amplify separator residue into speech-like noise.
const MAX_ALIGNMENT_GAIN: f64 = 4.0;
// Public cross-utterance Chinese probes put separated owner chunks at
// similarity 0.600-0.818, while every non-owner-only tail stays at
// 0.184-0.323. Installed-device owner evidence has dipped to 0.361, so the
// hard mismatch ceiling deliberately stops at 0.35. The wider 0.35-0.42 band
// remains pending identity. Retain original PCM, but do not certify that band
// as an owner-only source merely because it is above the hard mismatch floor.
const SEPARATOR_MODEL_FILE_NAME: &str = "wesep-bsrnn-voicefilter-3s-int8.onnx";
const ENCODER_MODEL_FILE_NAME: &str = "wesep-speaker-encoder-int8.onnx";
pub(crate) const SEPARATOR_MODEL_SHA256: &str =
    "1BFA3C60EA58288DE6947C62D6A49FBA9AEEE20BFBCE0B0415504F506F3F20B4";
pub(crate) const ENCODER_MODEL_SHA256: &str =
    "9CEC30564E3A87746BDD44108779EDDF5323CD9ED4BD0966B64883A6EEBBF46E";

static ENCODER_SESSION: OnceCell<Mutex<Session>> = OnceCell::new();
static SEPARATOR_SESSION: OnceCell<Mutex<Session>> = OnceCell::new();
static INFERENCE_RUN_OPTIONS: OnceCell<RunOptions> = OnceCell::new();

fn inference_run_options() -> Result<&'static RunOptions, String> {
    INFERENCE_RUN_OPTIONS.get_or_try_init(|| {
        let mut options = RunOptions::new()
            .map_err(|err| format!("create target-speaker run options failed: {err}"))?;
        // Keep the models and thread pools warm, but return unused temporary
        // CPU arena regions after each chunk. Otherwise one three-second
        // inference keeps hundreds of MiB committed for the app's lifetime.
        options
            .add_config_entry("memory.enable_memory_arena_shrinkage", "cpu:0")
            .map_err(|err| format!("configure target-speaker memory reclamation failed: {err}"))?;
        Ok(options)
    })
}

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

fn bounded_alignment_gain(gain: f64) -> f64 {
    if gain.is_finite() {
        gain.clamp(-MAX_ALIGNMENT_GAIN, MAX_ALIGNMENT_GAIN)
    } else {
        1.0
    }
}

fn average_speaker_embeddings(embeddings: &[Vec<f32>]) -> Result<Vec<f32>, String> {
    if embeddings.is_empty()
        || embeddings
            .iter()
            .any(|embedding| embedding.len() != SPEAKER_EMBEDDING_DIMENSION)
    {
        return Err("target-speaker enrollment produced an invalid embedding bank".to_string());
    }
    let mut average = vec![0.0f32; SPEAKER_EMBEDDING_DIMENSION];
    for embedding in embeddings {
        if embedding.iter().any(|value| !value.is_finite()) {
            return Err("target-speaker enrollment produced a non-finite embedding".to_string());
        }
        for (mean, value) in average.iter_mut().zip(embedding) {
            *mean += *value;
        }
    }
    let divisor = embeddings.len() as f32;
    for value in &mut average {
        *value /= divisor;
    }
    Ok(average)
}

pub(crate) struct TargetSpeakerExtractor {
    speaker_embedding: Vec<f32>,
}

pub(crate) struct ExtractedChunk {
    pub(crate) pcm: Vec<u8>,
    pub(crate) residual_energy: f64,
    pub(crate) mixture_energy: f64,
    pub(crate) target_to_mix_energy: f64,
    pub(crate) target_similarity: Option<f32>,
}

use super::target_speaker_output::{ExtractedOutputDecision, ExtractedOutputQueue};

fn accumulate_physical_interference_evidence(
    extracted: &ExtractedOutputDecision,
    residual_energy: &mut f64,
    mixture_energy: &mut f64,
    evidence_chunks: &mut usize,
) -> bool {
    if !extracted.supports_physical_interference_measurement() {
        return false;
    }
    *residual_energy += extracted.residual_energy;
    *mixture_energy += extracted.mixture_energy;
    *evidence_chunks += 1;
    true
}

pub(crate) struct ExtractedWakeCandidate {
    pub(crate) pcm: Vec<u8>,
    pub(crate) residual_ratio: f64,
    pub(crate) inference_ms: u64,
    pub(crate) source_pcm_ms: usize,
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
                .run_with_options(
                    ort::inputs![
                        "mix_stft_ri" => mix_tensor,
                        "speaker_logits" => speaker_tensor
                    ],
                    inference_run_options()?,
                )
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
        let output_gain = bounded_alignment_gain(gain);
        let residual_energy = mixture
            .iter()
            .zip(target)
            .map(|(left, right)| (*left as f64 - gain * *right as f64).powi(2))
            .sum::<f64>();
        let aligned_target = target
            .iter()
            .map(|sample| (*sample as f64 * output_gain) as f32)
            .collect::<Vec<_>>();
        let pcm = samples_to_pcm(&aligned_target, valid_samples);
        let target_similarity = speaker_embedding_from_single_window(&pcm)
            .ok()
            .and_then(|embedding| cosine_similarity(&self.speaker_embedding, &embedding));
        let target_to_mix_energy = target_energy / mixture_energy.max(1e-12);
        log::info!(
            "[target-speaker] chunk calibration valid_ms={} raw_gain={gain:.6} output_gain={output_gain:.6} target_to_mix_energy={target_to_mix_energy:.6} target_similarity={:?}",
            valid_samples / 16,
            target_similarity
        );
        Ok(ExtractedChunk {
            pcm,
            residual_energy,
            mixture_energy,
            target_to_mix_energy,
            target_similarity,
        })
    }
}

/// Extract one bounded owner-wake snapshot before the ordinary phrase gate.
/// This is intentionally separate from `TargetSpeakerStream`: wake recovery
/// must not open a cloud stream, and the caller still requires both an exact
/// local phrase result and an enrolled-owner verification on this output.
pub(crate) fn extract_enrolled_owner_wake_candidate(
    pcm: &[u8],
    speaker_embedding: Vec<f32>,
) -> Result<ExtractedWakeCandidate, String> {
    if pcm.is_empty() {
        return Err("target-speaker wake candidate is empty".to_string());
    }
    let bounded_len = pcm.len().min(CHUNK_BYTES) & !1usize;
    let bounded = &pcm[..bounded_len];
    let extractor = TargetSpeakerExtractor::from_embedding(speaker_embedding)?;
    let started = Instant::now();
    let extracted = extractor.extract_chunk_with_metrics(bounded)?;
    let inference_ms = started.elapsed().as_millis() as u64;
    let residual_ratio = extracted.residual_energy / extracted.mixture_energy.max(1e-12);
    Ok(ExtractedWakeCandidate {
        pcm: extracted.pcm,
        residual_ratio,
        inference_ms,
        source_pcm_ms: bounded_len / 32,
    })
}

pub(crate) fn extract_enrolled_owner_pcm(
    enrollment_pcm: &[u8],
    mixture_pcm: &[u8],
) -> Result<Vec<u8>, String> {
    extract_enrolled_owner_pcm_with_report(enrollment_pcm, mixture_pcm).map(|(pcm, _)| pcm)
}

pub(crate) fn extract_enrolled_owner_pcm_with_report(
    enrollment_pcm: &[u8],
    mixture_pcm: &[u8],
) -> Result<(Vec<u8>, String), String> {
    if mixture_pcm.is_empty() {
        return Err("target-speaker mixture is empty".to_string());
    }
    let extractor = TargetSpeakerExtractor::new(enrollment_pcm)?;
    let mut output = Vec::with_capacity(mixture_pcm.len());
    let mut report = String::from(
        "chunk,valid_ms,target_similarity,target_to_mix_energy,residual_ratio,suppressed\n",
    );
    for (index, chunk) in mixture_pcm.chunks(CHUNK_BYTES).enumerate() {
        let extracted = extractor.extract_chunk_with_metrics(chunk)?;
        let similarity = extracted
            .target_similarity
            .map(|value| format!("{value:.6}"))
            .unwrap_or_else(|| "unknown".to_string());
        let residual_ratio = extracted.residual_energy / extracted.mixture_energy.max(1e-12);
        // Raw extraction diagnostic: conditioning cosine cannot authorize suppression.
        let suppressed = false;
        report.push_str(&format!(
            "{index},{},{similarity},{:.6},{residual_ratio:.6},{suppressed}\n",
            chunk.len() / 32,
            extracted.target_to_mix_energy,
        ));
        output.extend(extracted.pcm);
    }
    Ok((output, report))
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let dot = left
        .iter()
        .zip(right)
        .map(|(a, b)| *a as f64 * *b as f64)
        .sum::<f64>();
    let left_norm = left
        .iter()
        .map(|value| (*value as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    let right_norm = right
        .iter()
        .map(|value| (*value as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    let denominator = left_norm * right_norm;
    if denominator <= f64::EPSILON {
        return None;
    }
    Some((dot / denominator) as f32)
}

fn speaker_embedding_from_single_window(pcm: &[u8]) -> Result<Vec<f32>, String> {
    let speaker_fbank = enrollment_fbank(pcm)?;
    let tensor = TensorRef::from_array_view((
        [1usize, ENROLLMENT_FRAMES, FBANK_BINS],
        speaker_fbank.as_slice(),
    ))
    .map_err(|err| format!("create target-speaker enrollment tensor failed: {err}"))?;
    let mut session = encoder_session()?.lock();
    let outputs = session
        .run_with_options(
            ort::inputs!["speaker_fbank" => tensor],
            inference_run_options()?,
        )
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

pub(crate) fn speaker_embedding_from_enrollment_pcm(pcm: &[u8]) -> Result<Vec<f32>, String> {
    if pcm.len() <= CHUNK_BYTES {
        return speaker_embedding_from_single_window(pcm);
    }
    let embeddings = pcm
        .chunks(CHUNK_BYTES)
        .filter(|window| window.len() >= SAMPLE_RATE)
        .map(speaker_embedding_from_single_window)
        .collect::<Result<Vec<_>, _>>()?;
    average_speaker_embeddings(&embeddings)
}

pub(crate) fn wake_phrase_enrollment_pcm(wake_pcm: &[u8], wake_end_seconds: f32) -> Vec<u8> {
    let even_len = wake_pcm.len() & !1;
    let end = ((wake_end_seconds.max(0.0) * 32_000.0).round() as usize).min(even_len) & !1usize;
    let desired = SAMPLE_RATE * 2 * 2_200 / 1_000;
    let start = end.saturating_sub(desired) & !1usize;
    wake_pcm[start..end].to_vec()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetSpeakerFinishReason {
    CleanPrimary,
    OwnerCandidate,
    NoOwnerEvidence,
    Unresolved,
}

#[derive(Debug)]
pub(crate) struct TargetSpeakerFinishOutcome {
    pub(crate) transcript: Option<crate::asr::RawTranscript>,
    pub(crate) reason: TargetSpeakerFinishReason,
    pub(crate) enqueued_bytes: u64,
    pub(crate) processed_bytes: u64,
}

impl TargetSpeakerFinishOutcome {
    fn new(
        transcript: Option<crate::asr::RawTranscript>,
        reason: TargetSpeakerFinishReason,
        enqueued_bytes: u64,
        processed_bytes: u64,
        output_unresolved: bool,
    ) -> Self {
        let reason = if output_unresolved || enqueued_bytes != processed_bytes {
            TargetSpeakerFinishReason::Unresolved
        } else {
            reason
        };
        Self {
            transcript,
            reason,
            enqueued_bytes,
            processed_bytes,
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.enqueued_bytes == self.processed_bytes
    }

    pub(crate) fn certifies_clean_primary(&self) -> bool {
        self.reason == TargetSpeakerFinishReason::CleanPrimary && self.is_complete()
    }

    pub(crate) fn into_owner_candidate(self) -> Option<crate::asr::RawTranscript> {
        // A pending tail prevents a complete/clean certificate, but cannot
        // revoke already decoded, positively authorized owner audio. Timing
        // and owner ranges still constrain recovery of this prefix.
        match self.reason {
            TargetSpeakerFinishReason::OwnerCandidate | TargetSpeakerFinishReason::Unresolved => self.transcript,
            _ => None,
        }
    }
}

/// A hidden authoritative stream fed only target-speaker-extracted audio.
/// The normal stream keeps driving clean-room preview. After explicit
/// non-owner evidence, this stream owns live text as well as the final.
pub(crate) struct TargetSpeakerStream {
    audio_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    worker: Mutex<
        Option<
            tauri::async_runtime::JoinHandle<Result<TargetSpeakerFinishOutcome, String>>,
        >,
    >,
    interference_detected: Arc<AtomicBool>,
    non_target_speech_detected: Arc<AtomicBool>,
    secondary_asr: Arc<Mutex<Option<Arc<crate::asr::VolcengineStreamingASR>>>>,
    owner_audio_verified: Arc<AtomicBool>,
    owner_audio_ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    source_profile: Arc<Mutex<Option<crate::speaker_verification::SessionSpeakerProfile>>>,
    source_owner_audio_ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    source_owner_audio_ranges_updated_at: Arc<Mutex<Option<Instant>>>,
    physical_interference_ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    final_timing: Arc<Mutex<Option<crate::transcript_boundary::TimedTranscript>>>,
    /// Last complete three-second source chunk for which separator and original-
    /// source identity processing have both finished. Endpointing reads this
    /// frontier only as a bounded in-flight barrier; it never grants ownership.
    processed_through_ms: Arc<AtomicU64>,
    /// Exact input and worker frontiers. Millisecond frontiers round PCM byte
    /// offsets and therefore cannot prove that STOP drained the final samples.
    enqueued_bytes: Arc<AtomicU64>,
    processed_bytes: Arc<AtomicU64>,
    /// True while extracted output lacks a resolved owner decision. Pending is
    /// neither clean nor foreign and therefore cannot certify the primary.
    output_unresolved: Arc<AtomicBool>,
    worker_terminal: Arc<AtomicBool>,
    #[cfg(test)]
    chunk_diagnostics: Arc<Mutex<Vec<TargetSpeakerChunkDiagnostic>>>,
    #[cfg(test)]
    secondary_opened_ever: Arc<AtomicBool>,
    #[cfg(test)]
    secondary_provider_responses: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[cfg(test)]
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct TargetSpeakerChunkDiagnostic {
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
    pub(crate) ownership: String,
    pub(crate) target_similarity: Option<f32>,
    pub(crate) target_to_mix_energy: f64,
    pub(crate) residual_energy: f64,
    pub(crate) mixture_energy: f64,
    pub(crate) output_silence_confirmed: bool,
    pub(crate) extracted_pcm_path: Option<String>,
    pub(crate) owner_transport_pcm_path: Option<String>,
}

#[cfg(test)]
const TEST_PCM_EXPORT_DIRECTORY_ENV: &str = "LISTENER_TEST_SEPARATOR_PCM_EXPORT_DIR";

#[cfg(test)]
fn test_pcm_export_directory() -> Option<std::path::PathBuf> {
    std::env::var_os(TEST_PCM_EXPORT_DIRECTORY_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
fn export_test_chunk_pcm(
    source_range_ms: (u64, u64),
    extracted: &ExtractedChunk,
    transport: Option<Vec<u8>>,
) -> Result<Option<(String, Option<String>)>, String> {
    let Some(directory) = test_pcm_export_directory() else {
        return Ok(None);
    };
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "create target-speaker test PCM export directory {} failed: {error}",
            directory.display()
        )
    })?;
    let stem = format!(
        "source-{:06}-{:06}",
        source_range_ms.0, source_range_ms.1
    );
    let extracted_path = directory.join(format!("{stem}-extracted.pcm"));
    let owner_transport_path = directory.join(format!("{stem}-owner-transport.pcm"));
    std::fs::write(&extracted_path, &extracted.pcm).map_err(|error| {
        format!(
            "write target-speaker extracted test PCM {} failed: {error}",
            extracted_path.display()
        )
    })?;
    let transport_path = if let Some(pcm) = transport {
        std::fs::write(&owner_transport_path, pcm)
            .map_err(|error| format!("write owner transport failed: {error}"))?;
        Some(owner_transport_path.to_string_lossy().into_owned())
    } else { None };
    Ok(Some((extracted_path.to_string_lossy().into_owned(), transport_path)))
}

struct TargetSpeakerWorkerTerminalGuard(Arc<AtomicBool>);

impl Drop for TargetSpeakerWorkerTerminalGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn source_owner_analysis_is_pending(
    enrolled_profile_ready: bool,
    worker_terminal: bool,
    processed_through_ms: u64,
    captured_audio_ms: u64,
    speech_end_ms: u64,
) -> bool {
    if !enrolled_profile_ready || worker_terminal {
        return false;
    }
    let complete_chunk_ms = (CHUNK_BYTES / 32) as u64;
    let available_frontier_ms = captured_audio_ms / complete_chunk_ms * complete_chunk_ms;
    speech_end_ms > processed_through_ms && available_frontier_ms > processed_through_ms
}

fn push_merged_audio_range(ranges: &Mutex<Vec<(u64, u64)>>, range: (u64, u64)) {
    if range.0 >= range.1 {
        return;
    }
    let mut ranges = ranges.lock();
    if let Some(last) = ranges.last_mut() {
        if range.0 <= last.1 {
            last.1 = last.1.max(range.1);
            return;
        }
    }
    ranges.push(range);
}

fn retain_source_owner_ranges(
    ranges: &Mutex<Vec<(u64, u64)>>,
    updated_at: &Mutex<Option<Instant>>,
    certified: Vec<(u64, u64)>,
) {
    if certified.is_empty() {
        return;
    }
    for range in &certified {
        push_merged_audio_range(ranges, *range);
    }
    *updated_at.lock() = Some(Instant::now());
    log::info!(
        "[target-speaker] original source owner evidence retained ranges={certified:?}"
    );
}

fn retain_extracted_chunk_ranges(
    extracted: &ExtractedOutputDecision,
    source_range_ms: (u64, u64),
    owner_audio_ranges: &Mutex<Vec<(u64, u64)>>,
    physical_interference_ranges: &Mutex<Vec<(u64, u64)>>,
) {
    if extracted.ownership() != crate::speech_decision_kernel::SeparatedAudioOwnership::Owner {
        return;
    }
    push_merged_audio_range(owner_audio_ranges, source_range_ms);
    let residual_ratio = extracted.residual_energy / extracted.mixture_energy.max(1e-12);
    if extracted_chunk_has_physical_interference(extracted) {
        push_merged_audio_range(physical_interference_ranges, source_range_ms);
        log::info!(
            "[target-speaker] physical interference interval retained start_ms={} end_ms={} residual_ratio={residual_ratio:.6}",
            source_range_ms.0,
            source_range_ms.1,
        );
    }
}

fn extracted_chunk_has_physical_interference(extracted: &ExtractedOutputDecision) -> bool {
    extracted.supports_physical_interference_measurement()
        && extracted.residual_energy / extracted.mixture_energy.max(1e-12)
            >= STRONG_INTERFERENCE_RESIDUAL_RATIO
}

// The separator's output identity cannot classify its input: suppression or
// distortion may remove the owner's voice. Reuse the independently verified
// session profile on the bounded ORIGINAL chunk. This evidence authorizes only
// source-timed recovery outside confirmed overlap/foreign intervals.
#[cfg(test)]
fn source_observation_certifies_owner(
    signal_quality_sufficient: bool,
    classification: crate::speaker_verification::SessionSpeakerClassification,
) -> bool {
    signal_quality_sufficient
        && matches!(
            classification,
            crate::speaker_verification::SessionSpeakerClassification::Target { .. }
        )
}

#[derive(Debug)]
struct PendingSourceOwnerChunk {
    pcm: Vec<u8>,
    source_range_ms: (u64, u64),
}

#[derive(Default)]
struct PendingSourceOwnerEvidence {
    prefix: VecDeque<PendingSourceOwnerChunk>,
}

impl PendingSourceOwnerEvidence {
    fn submit(
        &mut self,
        profile_ready: bool,
        current: Option<PendingSourceOwnerChunk>,
    ) -> Vec<PendingSourceOwnerChunk> {
        if !profile_ready {
            if let Some(current) = current {
                if self.prefix.len() < SOURCE_PROFILE_PENDING_MAX_CHUNKS {
                    self.prefix.push_back(current);
                }
            }
            return Vec::new();
        }

        let mut ready = self.prefix.drain(..).collect::<Vec<_>>();
        if let Some(current) = current {
            ready.push(current);
        }
        ready
    }
}

fn certified_source_owner_ranges(
    profile: Option<&crate::speaker_verification::SessionSpeakerProfile>,
    context: &mut Option<crate::speaker_verification::VerifiedOriginalSourceContext>,
    chunks: Vec<PendingSourceOwnerChunk>,
    quality: &Mutex<crate::speech_signal_quality::SpeechSignalTimeline>,
) -> Vec<(u64, u64)> {
    if context.is_none() {
        *context = profile.cloned()
            .and_then(crate::speaker_verification::VerifiedOriginalSourceContext::new);
    }
    chunks
        .into_iter()
        .filter_map(|chunk| {
            let upper_bound = quality.lock().speech_upper_bound_ms(chunk.source_range_ms.0, chunk.source_range_ms.1);
            context.as_mut().is_some_and(|context| context.certifies_original_pcm_with_quality(&chunk.pcm, upper_bound))
                .then_some(chunk.source_range_ms)
        })
        .collect()
}

async fn feed_extracted_chunk(
    secondary: &mut Option<Arc<crate::asr::VolcengineStreamingASR>>,
    staged_pcm: &mut Vec<u8>,
    credentials: &crate::asr::VolcengineCredentials,
    hotwords: &[crate::asr::DictionaryHotword],
    proxy_config: &ProviderProxyConfig,
    extracted: ExtractedOutputDecision,
    source_range_ms: (u64, u64),
    residual_energy: &mut f64,
    mixture_energy: &mut f64,
    interference_evidence_chunks: &mut usize,
    interference_detected: &AtomicBool,
    non_target_speech_detected: &AtomicBool,
    secondary_slot: &Mutex<Option<Arc<crate::asr::VolcengineStreamingASR>>>,
    owner_audio_verified: &AtomicBool,
    owner_audio_ranges: &Mutex<Vec<(u64, u64)>>,
    physical_interference_ranges: &Mutex<Vec<(u64, u64)>>,
    on_partial: &Arc<dyn Fn(String) + Send + Sync>,
    on_streaming_event: &Arc<dyn Fn(super::volcengine::VolcengineStreamingEvent) + Send + Sync>,
    on_final_intermediate: &Arc<
        dyn Fn(super::volcengine::FinalIntermediateTranscript) + Send + Sync,
    >,
) -> Result<(), String> {
    let ownership = extracted.ownership();
    let owner_pcm = extracted.owner_transport_pcm().ok_or_else(|| "unresolved output must remain queued".to_string())?;
    if ownership == crate::speech_decision_kernel::SeparatedAudioOwnership::Owner {
        owner_audio_verified.store(true, Ordering::SeqCst);
    } else {
        log::info!("[target-speaker] extracted audio ownership={ownership:?} valid_ms={} target_similarity={:?}; retained original, owner transport muted", extracted.pcm.len() / 32, extracted.target_similarity);
    }
    let measurement_valid = accumulate_physical_interference_evidence(
        &extracted,
        residual_energy,
        mixture_energy,
        interference_evidence_chunks,
    );
    retain_extracted_chunk_ranges(
        &extracted,
        source_range_ms,
        owner_audio_ranges,
        physical_interference_ranges,
    );
    if extracted.confidently_non_target() {
        non_target_speech_detected.store(true, Ordering::SeqCst);
    }
    if let Some(asr) = secondary.as_ref() {
        asr.consume_pcm_chunk(&owner_pcm);
        return Ok(());
    }

    staged_pcm.extend_from_slice(&owner_pcm);
    if !measurement_valid {
        return Ok(());
    }
    // Ranges and decoder activation consume the same local decision. Averaging
    // this interval into minutes of clean audio must not erase its existence.
    let residual_ratio = extracted.residual_energy / extracted.mixture_energy.max(1e-12);
    if !extracted_chunk_has_physical_interference(&extracted) {
        return Ok(());
    }

    interference_detected.store(true, Ordering::SeqCst);
    if !owner_audio_verified.load(Ordering::SeqCst) {
        return Ok(());
    }
    log::info!(
        "[target-speaker] strong interference detected during capture residual_ratio={residual_ratio:.6} threshold={STRONG_INTERFERENCE_RESIDUAL_RATIO:.6}; opening streaming extracted ASR"
    );
    let asr = Arc::new(crate::asr::VolcengineStreamingASR::new_with_proxy_config(
        credentials.clone(),
        hotwords.to_vec(),
        proxy_config.clone(),
    ));
    asr.set_partial_transcript_callback(Some(Arc::clone(on_partial)));
    asr.set_streaming_event_callback(Some(Arc::clone(on_streaming_event)));
    asr.set_final_intermediate_transcript_callback(Some(Arc::clone(on_final_intermediate)));
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
        proxy_config: ProviderProxyConfig,
        quality_sink: Arc<Mutex<crate::speech_signal_quality::SpeechSignalTimeline>>,
        on_partial: Arc<dyn Fn(String) + Send + Sync>,
        on_streaming_event: Arc<dyn Fn(super::volcengine::VolcengineStreamingEvent) + Send + Sync>,
        on_final_intermediate: Arc<
            dyn Fn(super::volcengine::FinalIntermediateTranscript) + Send + Sync,
        >,
    ) -> Arc<Self> {
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let interference_detected = Arc::new(AtomicBool::new(false));
        let worker_interference_detected = Arc::clone(&interference_detected);
        let non_target_speech_detected = Arc::new(AtomicBool::new(false));
        let worker_non_target_speech_detected = Arc::clone(&non_target_speech_detected);
        let owner_audio_verified = Arc::new(AtomicBool::new(false));
        let worker_owner_audio_verified = Arc::clone(&owner_audio_verified);
        let owner_audio_ranges = Arc::new(Mutex::new(Vec::new()));
        let worker_owner_audio_ranges = Arc::clone(&owner_audio_ranges);
        let source_profile = Arc::new(Mutex::new(None::<crate::speaker_verification::SessionSpeakerProfile>));
        let worker_source_profile = Arc::clone(&source_profile);
        let source_owner_audio_ranges = Arc::new(Mutex::new(Vec::new()));
        let worker_source_owner_audio_ranges = Arc::clone(&source_owner_audio_ranges);
        let source_owner_audio_ranges_updated_at = Arc::new(Mutex::new(None));
        let worker_source_owner_audio_ranges_updated_at =
            Arc::clone(&source_owner_audio_ranges_updated_at);
        let physical_interference_ranges = Arc::new(Mutex::new(Vec::new()));
        let worker_physical_interference_ranges = Arc::clone(&physical_interference_ranges);
        let final_timing = Arc::new(Mutex::new(None));
        let worker_final_timing = Arc::clone(&final_timing);
        #[cfg(test)]
        let chunk_diagnostics = Arc::new(Mutex::new(Vec::new()));
        #[cfg(test)]
        let worker_chunk_diagnostics = Arc::clone(&chunk_diagnostics);
        let processed_through_ms = Arc::new(AtomicU64::new(0));
        let worker_processed_through_ms = Arc::clone(&processed_through_ms);
        let enqueued_bytes = Arc::new(AtomicU64::new(0));
        let worker_enqueued_bytes = Arc::clone(&enqueued_bytes);
        let processed_bytes_frontier = Arc::new(AtomicU64::new(0));
        let worker_processed_bytes_frontier = Arc::clone(&processed_bytes_frontier);
        let output_unresolved = Arc::new(AtomicBool::new(false));
        let worker_output_unresolved = Arc::clone(&output_unresolved);
        #[cfg(test)]
        let secondary_opened_ever = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let worker_secondary_opened_ever = Arc::clone(&secondary_opened_ever);
        #[cfg(test)]
        let secondary_provider_responses = Arc::new(Mutex::new(Vec::new()));
        #[cfg(test)]
        let worker_secondary_provider_responses = Arc::clone(&secondary_provider_responses);
        let worker_terminal = Arc::new(AtomicBool::new(false));
        let worker_terminal_guard = Arc::clone(&worker_terminal);
        let partial_evidence = Arc::clone(&non_target_speech_detected);
        let event_evidence = Arc::clone(&non_target_speech_detected);
        let final_evidence = Arc::clone(&non_target_speech_detected);
        let on_partial: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text| {
            if partial_evidence.load(Ordering::SeqCst) {
                on_partial(text);
            }
        });
        let on_streaming_event: Arc<
            dyn Fn(super::volcengine::VolcengineStreamingEvent) + Send + Sync,
        > = Arc::new(move |event| {
            if event_evidence.load(Ordering::SeqCst) {
                on_streaming_event(event);
            }
        });
        let on_final_intermediate: Arc<
            dyn Fn(super::volcengine::FinalIntermediateTranscript) + Send + Sync,
        > = Arc::new(move |update| {
            if final_evidence.load(Ordering::SeqCst) {
                on_final_intermediate(update);
            }
        });
        let secondary_asr = Arc::new(Mutex::new(None));
        let worker_secondary_asr = Arc::clone(&secondary_asr);
        let worker = tauri::async_runtime::spawn(async move {
            let _terminal_guard = TargetSpeakerWorkerTerminalGuard(worker_terminal_guard);
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
            let mut interference_evidence_chunks = 0usize;
            let mut processed_bytes = 0usize;
            let mut pending_source_owner_evidence = PendingSourceOwnerEvidence::default();
            let mut source_context = None;
            let mut output_queue = tauri::async_runtime::spawn_blocking(ExtractedOutputQueue::new)
                .await.map_err(|err| format!("extracted quality initialization failed: {err}"))?;
            let mut secondary = None;
            while let Some(pcm) = audio_rx.recv().await {
                buffered.extend_from_slice(&pcm);
                while buffered.len() >= CHUNK_BYTES {
                    let trailing = buffered.split_off(CHUNK_BYTES);
                    let chunk = std::mem::replace(&mut buffered, trailing);
                    let source_range_ms = (
                        (processed_bytes / 32) as u64,
                        ((processed_bytes + CHUNK_BYTES) / 32) as u64,
                    );
                    processed_bytes += CHUNK_BYTES;
                    let extractor = Arc::clone(&extractor);
                    let profile = worker_source_profile.lock().clone();
                    let source_candidates = pending_source_owner_evidence.submit(
                        profile.as_ref().is_some_and(|profile| !profile.is_adaptive()),
                        Some(PendingSourceOwnerChunk {
                            pcm: chunk.clone(),
                            source_range_ms,
                        }),
                    );
                    let mut context = source_context.take();
                    let quality = Arc::clone(&quality_sink);
                    let (extracted, certified_ranges, context) = tauri::async_runtime::spawn_blocking(move || {
                        let certified_ranges = certified_source_owner_ranges(
                            profile.as_ref(),
                            &mut context,
                            source_candidates,
                            &quality,
                        );
                        extractor
                            .extract_chunk_with_metrics(&chunk)
                            .map(|extracted| (extracted, certified_ranges, context))
                    })
                    .await
                    .map_err(|err| format!("target-speaker chunk task failed: {err}"))??;
                    source_context = context;
                    #[cfg(test)]
                    export_test_chunk_pcm(source_range_ms, &extracted, None)?;
                    let profile = worker_source_profile.lock().clone();
                    let (queue, ready_outputs) = tauri::async_runtime::spawn_blocking(move || {
                        let result = output_queue.submit(extracted, source_range_ms, profile);
                        (output_queue, result)
                    }).await.map_err(|err| format!("extracted identity task failed: {err}"))?;
                    output_queue = queue;
                    worker_output_unresolved.store(output_queue.unresolved(), Ordering::SeqCst);
                    let ready_outputs = ready_outputs?;
                    for (extracted, output_range_ms) in ready_outputs {
                        #[cfg(test)]
                        let chunk_diagnostic = {
                            let exported = export_test_chunk_pcm(output_range_ms, &extracted, extracted.owner_transport_pcm())?;
                            TargetSpeakerChunkDiagnostic {
                                start_ms: output_range_ms.0, end_ms: output_range_ms.1,
                                ownership: format!("{:?}", extracted.ownership()),
                                target_similarity: extracted.target_similarity,
                                target_to_mix_energy: extracted.target_to_mix_energy,
                                residual_energy: extracted.residual_energy, mixture_energy: extracted.mixture_energy,
                                output_silence_confirmed: extracted.silence_confirmed,
                                extracted_pcm_path: exported.as_ref().map(|paths| paths.0.clone()),
                                owner_transport_pcm_path: exported.and_then(|paths| paths.1),
                            }
                        };
                        feed_extracted_chunk(
                            &mut secondary, &mut staged_pcm, &credentials, &hotwords, &proxy_config,
                            extracted, output_range_ms, &mut residual_energy, &mut mixture_energy,
                            &mut interference_evidence_chunks, &worker_interference_detected,
                            &worker_non_target_speech_detected, &worker_secondary_asr,
                            &worker_owner_audio_verified, &worker_owner_audio_ranges,
                            &worker_physical_interference_ranges, &on_partial, &on_streaming_event,
                            &on_final_intermediate,
                        ).await?;
                        #[cfg(test)] {
                            worker_chunk_diagnostics.lock().push(chunk_diagnostic);
                            if worker_secondary_asr.lock().is_some() {
                                worker_secondary_opened_ever.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                retain_source_owner_ranges(
                        &worker_source_owner_audio_ranges,
                        &worker_source_owner_audio_ranges_updated_at,
                        certified_ranges,
                    );
                    worker_processed_bytes_frontier
                        .store(processed_bytes as u64, Ordering::Release);
                    worker_processed_through_ms.store(source_range_ms.1, Ordering::Release);

                }
            }
            if !buffered.is_empty() {
                let tail_bytes = buffered.len();
                let source_range_ms = (
                    (processed_bytes / 32) as u64,
                    ((processed_bytes + tail_bytes) / 32) as u64,
                );
                let extractor = Arc::clone(&extractor);
                let profile = worker_source_profile.lock().clone();
                let source_candidates = pending_source_owner_evidence.submit(
                    profile.as_ref().is_some_and(|profile| !profile.is_adaptive()),
                    Some(PendingSourceOwnerChunk {
                        pcm: buffered.clone(),
                        source_range_ms,
                    }),
                );
                let mut context = source_context.take();
                let quality = Arc::clone(&quality_sink);
                let (extracted, certified_ranges, context) = tauri::async_runtime::spawn_blocking(move || {
                    let certified_ranges = certified_source_owner_ranges(
                        profile.as_ref(),
                        &mut context,
                        source_candidates,
                        &quality,
                    );
                    extractor
                        .extract_chunk_with_metrics(&buffered)
                        .map(|extracted| (extracted, certified_ranges, context))
                })
                .await
                .map_err(|err| format!("target-speaker tail task failed: {err}"))??;
                source_context = context;
                #[cfg(test)]
                export_test_chunk_pcm(source_range_ms, &extracted, None)?;
                let profile = worker_source_profile.lock().clone();
                let (queue, ready_outputs) = tauri::async_runtime::spawn_blocking(move || {
                    let result = output_queue.submit(extracted, source_range_ms, profile);
                    (output_queue, result)
                }).await.map_err(|err| format!("extracted identity task failed: {err}"))?;
                output_queue = queue;
                worker_output_unresolved.store(output_queue.unresolved(), Ordering::SeqCst);
                let ready_outputs = ready_outputs?;
                for (extracted, output_range_ms) in ready_outputs {
                    #[cfg(test)]
                    let chunk_diagnostic = {
                        let exported = export_test_chunk_pcm(output_range_ms, &extracted, extracted.owner_transport_pcm())?;
                        TargetSpeakerChunkDiagnostic {
                            start_ms: output_range_ms.0, end_ms: output_range_ms.1,
                            ownership: format!("{:?}", extracted.ownership()),
                            target_similarity: extracted.target_similarity,
                            target_to_mix_energy: extracted.target_to_mix_energy,
                            residual_energy: extracted.residual_energy, mixture_energy: extracted.mixture_energy,
                            output_silence_confirmed: extracted.silence_confirmed,
                            extracted_pcm_path: exported.as_ref().map(|paths| paths.0.clone()),
                            owner_transport_pcm_path: exported.and_then(|paths| paths.1),
                        }
                    };
                    feed_extracted_chunk(
                        &mut secondary, &mut staged_pcm, &credentials, &hotwords, &proxy_config,
                        extracted, output_range_ms, &mut residual_energy, &mut mixture_energy,
                        &mut interference_evidence_chunks, &worker_interference_detected,
                        &worker_non_target_speech_detected, &worker_secondary_asr,
                        &worker_owner_audio_verified, &worker_owner_audio_ranges,
                        &worker_physical_interference_ranges, &on_partial, &on_streaming_event,
                        &on_final_intermediate,
                    ).await?;
                    #[cfg(test)] {
                        worker_chunk_diagnostics.lock().push(chunk_diagnostic);
                        if worker_secondary_asr.lock().is_some() {
                            worker_secondary_opened_ever.store(true, Ordering::SeqCst);
                        }
                    }
                }
                retain_source_owner_ranges(
                    &worker_source_owner_audio_ranges,
                    &worker_source_owner_audio_ranges_updated_at,
                    certified_ranges,
                );
                processed_bytes += tail_bytes;
                worker_processed_bytes_frontier.store(processed_bytes as u64, Ordering::Release);
                worker_processed_through_ms.store(source_range_ms.1, Ordering::Release);

            }
            let profile = worker_source_profile.lock().clone();
            let (queue, ready_outputs) = tauri::async_runtime::spawn_blocking(move || {
                let ready = output_queue.finish(profile);
                (output_queue, ready)
            }).await.map_err(|err| format!("extracted identity drain failed: {err}"))?;
            output_queue = queue;
            worker_output_unresolved.store(output_queue.unresolved(), Ordering::SeqCst);
            for (extracted, output_range_ms) in ready_outputs {
                #[cfg(test)]
                let chunk_diagnostic = {
                    let exported = export_test_chunk_pcm(output_range_ms, &extracted, extracted.owner_transport_pcm())?;
                    TargetSpeakerChunkDiagnostic {
                        start_ms: output_range_ms.0, end_ms: output_range_ms.1,
                        ownership: format!("{:?}", extracted.ownership()),
                        target_similarity: extracted.target_similarity,
                        target_to_mix_energy: extracted.target_to_mix_energy,
                        residual_energy: extracted.residual_energy, mixture_energy: extracted.mixture_energy,
                        output_silence_confirmed: extracted.silence_confirmed,
                        extracted_pcm_path: exported.as_ref().map(|paths| paths.0.clone()),
                        owner_transport_pcm_path: exported.and_then(|paths| paths.1),
                    }
                };
                feed_extracted_chunk(
                    &mut secondary, &mut staged_pcm, &credentials, &hotwords, &proxy_config,
                    extracted, output_range_ms, &mut residual_energy, &mut mixture_energy,
                    &mut interference_evidence_chunks, &worker_interference_detected,
                    &worker_non_target_speech_detected, &worker_secondary_asr,
                    &worker_owner_audio_verified, &worker_owner_audio_ranges,
                    &worker_physical_interference_ranges, &on_partial, &on_streaming_event,
                    &on_final_intermediate,
                ).await?;
                #[cfg(test)] {
                    worker_chunk_diagnostics.lock().push(chunk_diagnostic);
                    if worker_secondary_asr.lock().is_some() {
                        worker_secondary_opened_ever.store(true, Ordering::SeqCst);
                    }
                }
            }
            let profile = worker_source_profile.lock().clone();
            let source_candidates = pending_source_owner_evidence.submit(
                profile.as_ref().is_some_and(|profile| !profile.is_adaptive()),
                None,
            );
            if !source_candidates.is_empty() {
                let certified_ranges = tauri::async_runtime::spawn_blocking(move || {
                    certified_source_owner_ranges(profile.as_ref(), &mut source_context, source_candidates, &quality_sink)
                })
                .await
                .map_err(|err| format!("target-speaker deferred source identity task failed: {err}"))?;
                retain_source_owner_ranges(
                    &worker_source_owner_audio_ranges,
                    &worker_source_owner_audio_ranges_updated_at,
                    certified_ranges,
                );
            }
            let finish_outcome =
                |transcript, reason| TargetSpeakerFinishOutcome::new(
                    transcript,
                    reason,
                    worker_enqueued_bytes.load(Ordering::Acquire),
                    worker_processed_bytes_frontier.load(Ordering::Acquire),
                    worker_output_unresolved.load(Ordering::Acquire),
                );
            if interference_evidence_chunks == 0 {
                #[cfg(test)]
                if let Some(asr) = secondary.as_ref() {
                    *worker_secondary_provider_responses.lock() =
                        asr.diagnostic_provider_responses();
                }
                if let Some(asr) = secondary {
                    asr.cancel();
                }
                worker_secondary_asr.lock().take();
                log::warn!(
                    "[target-speaker] separator produced no owner-valid interference evidence; preserving primary final"
                );
                return Ok(finish_outcome(
                    None,
                    TargetSpeakerFinishReason::NoOwnerEvidence,
                ));
            }
            let residual_ratio = residual_energy / mixture_energy.max(1e-12);
            if !worker_interference_detected.load(Ordering::SeqCst)
                && !worker_non_target_speech_detected.load(Ordering::SeqCst)
            {
                #[cfg(test)]
                if let Some(asr) = secondary.as_ref() {
                    *worker_secondary_provider_responses.lock() =
                        asr.diagnostic_provider_responses();
                }
                if let Some(asr) = secondary {
                    asr.cancel();
                }
                worker_secondary_asr.lock().take();
                log::info!(
                    "[target-speaker] clean owner stream kept on primary residual_ratio={residual_ratio:.6} threshold={STRONG_INTERFERENCE_RESIDUAL_RATIO:.6}"
                );
                return Ok(finish_outcome(
                    None,
                    TargetSpeakerFinishReason::CleanPrimary,
                ));
            }
            let Some(secondary) = secondary else {
                log::warn!("[target-speaker] interference remains unresolved: no positively verified owner audio; no secondary candidate");
                return Ok(finish_outcome(
                    None,
                    TargetSpeakerFinishReason::Unresolved,
                ));
            };
            let result = match secondary.send_last_frame().await {
                Ok(()) => secondary.await_final_result().await.map(|result| {
                    *worker_final_timing.lock() = secondary.transcript_timing_snapshot_for(&result.text);
                    finish_outcome(
                        Some(result),
                        TargetSpeakerFinishReason::OwnerCandidate,
                    )
                }).map_err(|err| format!("await target-speaker ASR failed: {err}")),
                Err(err) => Err(format!("finalize target-speaker ASR failed: {err}")),
            };
            #[cfg(test)]
            {
                *worker_secondary_provider_responses.lock() =
                    secondary.diagnostic_provider_responses();
            }
            worker_secondary_asr.lock().take();
            result
        });
        Arc::new(Self {
            audio_tx: Mutex::new(Some(audio_tx)),
            worker: Mutex::new(Some(worker)),
            interference_detected,
            non_target_speech_detected,
            secondary_asr,
            owner_audio_verified,
            owner_audio_ranges,
            source_profile,
            source_owner_audio_ranges,
            source_owner_audio_ranges_updated_at,
            physical_interference_ranges,
            final_timing,
            processed_through_ms,
            enqueued_bytes,
            processed_bytes: processed_bytes_frontier,
            output_unresolved,
            worker_terminal,
            #[cfg(test)]
            chunk_diagnostics,
            #[cfg(test)]
            secondary_opened_ever,
            #[cfg(test)]
            secondary_provider_responses,
        })
    }

    pub(crate) fn interference_detected(&self) -> bool {
        self.interference_detected.load(Ordering::SeqCst)
    }

    pub(crate) fn non_target_speech_detected(&self) -> bool {
        self.non_target_speech_detected.load(Ordering::SeqCst)
    }

    pub(crate) fn owner_audio_ranges(&self) -> Vec<(u64, u64)> {
        self.owner_audio_ranges.lock().clone()
    }

    pub(crate) fn set_source_profile(&self, profile: &crate::speaker_verification::SessionSpeakerProfile) {
        let mut slot = self.source_profile.lock();
        if slot.is_none() && !profile.is_adaptive() {
            *slot = Some(profile.clone());
        }
    }

    pub(crate) fn source_owner_audio_ranges(&self) -> Vec<(u64, u64)> {
        self.source_owner_audio_ranges.lock().clone()
    }

    pub(crate) fn source_owner_audio_ranges_updated_at(&self) -> Option<Instant> {
        *self.source_owner_audio_ranges_updated_at.lock()
    }

    /// Whether a complete source chunk overlapping the latest speech edge has
    /// entered this worker but has not published its identity result yet.
    /// Partial tail PCM is intentionally excluded: it is finalized only after
    /// STOP and therefore cannot hold STOP waiting on itself.
    pub(crate) fn source_owner_analysis_pending(
        &self,
        captured_audio_ms: u64,
        speech_end_ms: u64,
    ) -> bool {
        source_owner_analysis_is_pending(
            self.source_profile
                .lock()
                .as_ref()
                .is_some_and(|profile| !profile.is_adaptive()),
            self.worker_terminal.load(Ordering::Acquire),
            self.processed_through_ms.load(Ordering::Acquire),
            captured_audio_ms,
            speech_end_ms,
        )
    }

    pub(crate) fn physical_interference_ranges(&self) -> Vec<(u64, u64)> {
        self.physical_interference_ranges.lock().clone()
    }

    pub(crate) fn final_timing_snapshot(
        &self,
    ) -> Option<crate::transcript_boundary::TimedTranscript> {
        self.final_timing.lock().clone()
    }

    pub(crate) fn live_owner_asr(&self) -> Option<Arc<crate::asr::VolcengineStreamingASR>> {
        self.owner_audio_verified
            .load(Ordering::SeqCst)
            .then(|| self.secondary_asr.lock().clone())
            .flatten()
    }

    #[cfg(test)]
    pub(crate) async fn test_wait_for_terminal(&self, timeout: std::time::Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.worker_terminal.load(Ordering::SeqCst) && Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        self.worker_terminal.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn test_diagnostics(&self) -> serde_json::Value {
        let live_secondary = self.secondary_asr.lock().clone();
        let secondary_provider_responses = live_secondary
            .as_ref()
            .map(|asr| asr.diagnostic_provider_responses())
            .unwrap_or_else(|| self.secondary_provider_responses.lock().clone());
        let secondary_final_timing = self.final_timing_snapshot().map(|timing| {
            serde_json::json!({
                "text": timing.text,
                "words": timing.words.into_iter().map(|word| serde_json::json!({
                    "text": word.text,
                    "start_ms": word.start_ms,
                    "end_ms": word.end_ms,
                })).collect::<Vec<_>>(),
            })
        });
        serde_json::json!({
            "worker_terminal": self.worker_terminal.load(Ordering::SeqCst),
            "processed_through_ms": self.processed_through_ms.load(Ordering::Acquire),
            "enqueued_bytes": self.enqueued_bytes.load(Ordering::Acquire),
            "processed_bytes": self.processed_bytes.load(Ordering::Acquire),
            "output_unresolved": self.output_unresolved.load(Ordering::Acquire),
            "owner_audio_verified": self.owner_audio_verified.load(Ordering::SeqCst),
            "secondary_opened_ever": self.secondary_opened_ever.load(Ordering::SeqCst),
            "owner_audio_ranges": self.owner_audio_ranges(),
            "source_owner_audio_ranges": self.source_owner_audio_ranges(),
            "physical_interference_ranges": self.physical_interference_ranges(),
            "chunks": self.chunk_diagnostics.lock().clone(),
            "pcm_export_directory": test_pcm_export_directory()
                .map(|path| path.to_string_lossy().into_owned()),
            "secondary_provider_responses": secondary_provider_responses,
            "secondary_final_timing": secondary_final_timing,
        })
    }

    #[cfg(test)]
    pub(crate) fn test_live_owner(
        asr: Option<Arc<crate::asr::VolcengineStreamingASR>>,
        physical_overlap: bool,
        non_target: bool,
    ) -> Arc<Self> {
        let secondary_opened = asr.is_some();
        let secondary_provider_responses = asr
            .as_ref()
            .map(|asr| asr.diagnostic_provider_responses())
            .unwrap_or_default();
        Arc::new(Self {
            audio_tx: Mutex::new(None),
            worker: Mutex::new(None),
            interference_detected: Arc::new(AtomicBool::new(physical_overlap)),
            non_target_speech_detected: Arc::new(AtomicBool::new(non_target)),
            secondary_asr: Arc::new(Mutex::new(asr)),
            owner_audio_verified: Arc::new(AtomicBool::new(true)),
            owner_audio_ranges: Arc::new(Mutex::new(Vec::new())),
            source_profile: Arc::new(Mutex::new(None)),
            source_owner_audio_ranges: Arc::new(Mutex::new(Vec::new())),
            source_owner_audio_ranges_updated_at: Arc::new(Mutex::new(None)),
            physical_interference_ranges: Arc::new(Mutex::new(Vec::new())),
            final_timing: Arc::new(Mutex::new(None)),
            chunk_diagnostics: Arc::new(Mutex::new(Vec::new())),
            processed_through_ms: Arc::new(AtomicU64::new(0)),
            enqueued_bytes: Arc::new(AtomicU64::new(0)),
            processed_bytes: Arc::new(AtomicU64::new(0)),
            output_unresolved: Arc::new(AtomicBool::new(false)),
            secondary_opened_ever: Arc::new(AtomicBool::new(secondary_opened)),
            secondary_provider_responses: Arc::new(Mutex::new(secondary_provider_responses)),
            worker_terminal: Arc::new(AtomicBool::new(true)),
        })
    }

    #[cfg(test)]
    pub(crate) fn test_set_source_owner_evidence(
        &self,
        ranges: Vec<(u64, u64)>,
        updated_at: Instant,
    ) {
        *self.source_owner_audio_ranges.lock() = ranges;
        *self.source_owner_audio_ranges_updated_at.lock() = Some(updated_at);
    }

    #[cfg(test)]
    pub(crate) fn test_set_physical_interference_ranges(&self, ranges: Vec<(u64, u64)>) {
        *self.physical_interference_ranges.lock() = ranges;
    }

    pub(crate) fn consume_pcm_chunk(&self, pcm: &[u8]) {
        if pcm.is_empty() {
            return;
        }
        let tx = self.audio_tx.lock();
        if let Some(tx) = tx.as_ref() {
            if tx.send(pcm.to_vec()).is_ok() {
                self.enqueued_bytes
                    .fetch_add(pcm.len() as u64, Ordering::Release);
            }
        }
    }

    pub(crate) async fn finish(&self) -> Result<TargetSpeakerFinishOutcome, String> {
        self.audio_tx.lock().take();
        let worker = self.worker.lock().take();
        let Some(worker) = worker else {
            return Err("target-speaker stream already finalized".to_string());
        };
        let result = match worker.await {
            Ok(result) => result,
            Err(err) => Err(format!("target-speaker worker failed: {err}")),
        };
        let secondary = self.secondary_asr.lock().take();
        if let Some(asr) = secondary {
            #[cfg(test)]
            {
                *self.secondary_provider_responses.lock() =
                    asr.diagnostic_provider_responses();
            }
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
        let secondary = self.secondary_asr.lock().take();
        if let Some(asr) = secondary {
            #[cfg(test)]
            {
                *self.secondary_provider_responses.lock() =
                    asr.diagnostic_provider_responses();
            }
            asr.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_pending_output_never_certifies_a_clean_primary() {
        let no_owner = TargetSpeakerFinishOutcome::new(
            None,
            TargetSpeakerFinishReason::NoOwnerEvidence,
            32_000,
            32_000,
            false,
        );
        assert!(!no_owner.certifies_clean_primary());

        let pending = TargetSpeakerFinishOutcome::new(
            None,
            TargetSpeakerFinishReason::CleanPrimary,
            32_000,
            32_000,
            true,
        );
        assert_eq!(pending.reason, TargetSpeakerFinishReason::Unresolved);
        assert!(!pending.certifies_clean_primary());
    }

    #[test]
    fn clean_primary_requires_an_exact_stop_frontier() {
        let incomplete = TargetSpeakerFinishOutcome::new(
            None,
            TargetSpeakerFinishReason::CleanPrimary,
            32_000,
            31_998,
            false,
        );
        assert_eq!(incomplete.reason, TargetSpeakerFinishReason::Unresolved);
        assert!(!incomplete.is_complete());
        assert!(!incomplete.certifies_clean_primary());
    }

    #[test]
    fn pending_tail_retains_an_already_authorized_owner_prefix() {
        let pending = TargetSpeakerFinishOutcome::new(
            Some(crate::asr::RawTranscript { text: "已确认的本人正文".into(), duration_ms: 3000 }),
            TargetSpeakerFinishReason::OwnerCandidate, 128_000, 128_000, true,
        );
        assert_eq!(pending.reason, TargetSpeakerFinishReason::Unresolved);
        assert!(!pending.certifies_clean_primary());
        assert_eq!(pending.into_owner_candidate().unwrap().text, "已确认的本人正文");
    }

    fn pending_source_chunk(index: u8) -> PendingSourceOwnerChunk {
        let start_ms = u64::from(index) * 3_000;
        PendingSourceOwnerChunk {
            pcm: vec![index; 8],
            source_range_ms: (start_ms, start_ms + 3_000),
        }
    }

    #[test]
    fn delayed_source_profile_replays_only_the_bounded_prefix_without_waiting() {
        let mut pending = PendingSourceOwnerEvidence::default();
        assert!(pending.submit(false, Some(pending_source_chunk(0))).is_empty());
        assert!(pending.submit(false, Some(pending_source_chunk(1))).is_empty());
        assert!(pending.submit(false, Some(pending_source_chunk(2))).is_empty());
        assert_eq!(pending.prefix.len(), SOURCE_PROFILE_PENDING_MAX_CHUNKS);

        let ready = pending.submit(true, Some(pending_source_chunk(3)));
        assert_eq!(
            ready
                .iter()
                .map(|chunk| chunk.source_range_ms)
                .collect::<Vec<_>>(),
            vec![(0, 3_000), (3_000, 6_000), (9_000, 12_000)],
            "the first bounded chunks are replayed, the overflow remains unknown, and current audio continues"
        );
        assert!(pending.prefix.is_empty());
    }

    #[test]
    fn unknown_source_identity_never_certifies_owner_audio() {
        use crate::speaker_verification::SessionSpeakerClassification::{
            NonTarget, Uncertain,
        };
        assert!(!source_observation_certifies_owner(
            true,
            Uncertain { score: 0.41 },
        ));
        assert!(!source_observation_certifies_owner(
            true,
            NonTarget { score: 0.04 },
        ));
    }

    #[test]
    fn insufficient_quality_cannot_turn_target_score_into_source_owner() {
        use crate::speaker_verification::SessionSpeakerClassification::Target;
        assert!(!source_observation_certifies_owner(
            false,
            Target { score: 0.93 },
        ));
        assert!(source_observation_certifies_owner(
            true,
            Target { score: 0.44 },
        ));
    }

    #[test]
    #[ignore = "requires the installed ONNX Runtime and bundled speaker models"]
    fn cpu_arena_reclamation_preserves_model_outputs() {
        ensure_ort_environment().unwrap();
        let options = inference_run_options().unwrap();
        {
            let mut session = encoder_session().unwrap().lock();
            for amplitude in [0.0, 0.25, 1.0] {
                let features: Vec<f32> = (0..ENROLLMENT_FRAMES * FBANK_BINS)
                    .map(|index| (index as f32 * 0.017).sin() * amplitude)
                    .collect();
                let input = || {
                    TensorRef::from_array_view((
                        [1usize, ENROLLMENT_FRAMES, FBANK_BINS],
                        features.as_slice(),
                    ))
                    .unwrap()
                };
                let baseline = session
                    .run(ort::inputs!["speaker_fbank" => input()])
                    .unwrap()["speaker_logits"]
                    .try_extract_tensor::<f32>()
                    .unwrap()
                    .1
                    .to_vec();
                let reclaimed = session
                    .run_with_options(ort::inputs!["speaker_fbank" => input()], options)
                    .unwrap()["speaker_logits"]
                    .try_extract_tensor::<f32>()
                    .unwrap()
                    .1
                    .to_vec();
                assert_eq!(
                    baseline, reclaimed,
                    "encoder changed at amplitude={amplitude}"
                );
            }
        }
        let mut session = separator_session().unwrap().lock();
        let embedding =
            vec![1.0 / (SPEAKER_EMBEDDING_DIMENSION as f32).sqrt(); SPEAKER_EMBEDDING_DIMENSION];
        for amplitude in [0.0, 0.25, 1.0] {
            let mixture: Vec<f32> = (0..2 * FFT_BINS * STFT_FRAMES)
                .map(|index| (index as f32 * 0.013).sin() * amplitude)
                .collect();
            let inputs = || {
                ort::inputs![
                    "mix_stft_ri" => TensorRef::from_array_view((
                        [1usize, 2usize, FFT_BINS, STFT_FRAMES], mixture.as_slice(),
                    )).unwrap(),
                    "speaker_logits" => TensorRef::from_array_view((
                        [1usize, SPEAKER_EMBEDDING_DIMENSION], embedding.as_slice(),
                    )).unwrap()
                ]
            };
            let baseline = session.run(inputs()).unwrap()["target_stft_ri"]
                .try_extract_tensor::<f32>()
                .unwrap()
                .1
                .to_vec();
            let reclaimed = session.run_with_options(inputs(), options).unwrap()["target_stft_ri"]
                .try_extract_tensor::<f32>()
                .unwrap()
                .1
                .to_vec();
            assert_eq!(
                baseline, reclaimed,
                "separator changed at amplitude={amplitude}"
            );
        }
    }

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
    fn alignment_gain_is_finite_and_bounded() {
        assert_eq!(bounded_alignment_gain(2.5), 2.5);
        assert_eq!(bounded_alignment_gain(9.0), MAX_ALIGNMENT_GAIN);
        assert_eq!(bounded_alignment_gain(-9.0), -MAX_ALIGNMENT_GAIN);
        assert_eq!(bounded_alignment_gain(f64::NAN), 1.0);
    }

    #[test]
    fn enrollment_bank_averages_every_guided_take() {
        let first = vec![1.0; SPEAKER_EMBEDDING_DIMENSION];
        let second = vec![3.0; SPEAKER_EMBEDDING_DIMENSION];
        let third = vec![5.0; SPEAKER_EMBEDDING_DIMENSION];
        let average = average_speaker_embeddings(&[first, second, third]).unwrap();
        assert!(average
            .iter()
            .all(|value| (*value - 3.0).abs() < f32::EPSILON));
    }

    fn decided_chunk(ownership: crate::speech_decision_kernel::SeparatedAudioOwnership,
        similarity: Option<f32>, energy_ratio: f64) -> ExtractedOutputDecision {
        ExtractedOutputDecision { extracted: ExtractedChunk {
            pcm: vec![1, 2, 3, 4], residual_energy: 1.0, mixture_energy: 2.0,
            target_to_mix_energy: energy_ratio, target_similarity: similarity,
        }, ownership, silence_confirmed: false }
    }

    #[test]
    fn output_authority_is_independent_of_conditioning_similarity_and_scale() {
        use crate::speech_decision_kernel::SeparatedAudioOwnership::Owner;
        // Quiet saved extraction was intelligible at these conditioning scores;
        // long 57-60s was Owner despite raw scale below the old 0.025 cutoff.
        for (similarity, ratio) in [(0.35147, 0.037), (0.26037, 0.03),
            (0.18226, 0.04), (0.19177, 0.03), (0.3000, 0.00498)] {
            let owner = decided_chunk(Owner, Some(similarity), ratio);
            assert_eq!(owner.owner_transport_pcm(), Some(owner.pcm.clone()));
            let mut residual = 0.0; let mut mixture = 0.0; let mut count = 0;
            assert!(accumulate_physical_interference_evidence(&owner, &mut residual, &mut mixture, &mut count));
            assert_eq!((residual, mixture, count), (1.0, 2.0, 1));
        }
    }

    #[test]
    fn pending_output_keeps_pcm_and_has_no_transport_or_physical_authority() {
        use crate::speech_decision_kernel::SeparatedAudioOwnership::Pending;
        let pending = decided_chunk(Pending, Some(0.95), 1.0);
        assert_eq!(pending.owner_transport_pcm(), None);
        assert_eq!(pending.pcm, vec![1, 2, 3, 4]);
        assert!(!pending.confidently_non_target());
        let mut residual = 0.0; let mut mixture = 0.0; let mut count = 0;
        assert!(!accumulate_physical_interference_evidence(&pending, &mut residual, &mut mixture, &mut count));
        assert_eq!((residual, mixture, count), (0.0, 0.0, 0));
    }

    #[test]
    fn same_output_decision_controls_transport_ranges_and_residual() {
        use crate::speech_decision_kernel::SeparatedAudioOwnership::{Owner, Other, Pending};
        let owner_ranges = Mutex::new(Vec::new());
        let physical_ranges = Mutex::new(Vec::new());
        for (index, ownership) in [Owner, Other, Pending, Owner].into_iter().enumerate() {
            let mut chunk = decided_chunk(ownership, Some(0.25), 0.004);
            chunk.extracted.residual_energy = if index == 0 { 0.01 } else { 0.15 };
            chunk.extracted.mixture_energy = 1.0;
            let start = index as u64 * 3000;
            retain_extracted_chunk_ranges(&chunk, (start, start + 3000), &owner_ranges, &physical_ranges);
            assert_eq!(chunk.supports_physical_interference_measurement(), ownership == Owner);
            assert_eq!(chunk.confidently_non_target(), ownership == Other);
        }
        assert_eq!(*owner_ranges.lock(), vec![(0,3000),(9000,12000)]);
        assert_eq!(*physical_ranges.lock(), vec![(9000,12000)]);
    }

    #[test]
    fn local_interference_survives_a_lower_whole_session_average() {
        use crate::speech_decision_kernel::SeparatedAudioOwnership::Owner;
        let mut owner = decided_chunk(Owner, Some(0.30), 0.00498);
        owner.extracted.residual_energy = 0.178936638;
        owner.extracted.mixture_energy = 1.0;
        let mut residual = 0.0;
        let mut mixture = 10.0;
        let mut count = 10;
        accumulate_physical_interference_evidence(&owner, &mut residual, &mut mixture, &mut count);
        assert!(residual / mixture < STRONG_INTERFERENCE_RESIDUAL_RATIO);
        assert!(extracted_chunk_has_physical_interference(&owner));
        let owner_ranges = Mutex::new(Vec::new());
        let physical_ranges = Mutex::new(Vec::new());
        retain_extracted_chunk_ranges(&owner, (57000,60000), &owner_ranges, &physical_ranges);
        assert_eq!(*physical_ranges.lock(), vec![(57000,60000)]);
    }

    #[test]
    fn known_silence_preserves_time_without_becoming_other_or_owner() {
        use crate::speech_decision_kernel::SeparatedAudioOwnership::{Other, Pending};
        let other = decided_chunk(Other, Some(0.9), 1.0);
        assert_eq!(other.owner_transport_pcm(), Some(vec![0;4]));
        assert!(other.confidently_non_target());
        let mut silence = decided_chunk(Pending, None, 0.0);
        silence.silence_confirmed = true;
        assert_eq!(silence.owner_transport_pcm(), Some(vec![0;4]));
        assert!(!silence.confidently_non_target());
        assert!(!silence.supports_physical_interference_measurement());
    }

    #[test]
    fn cosine_similarity_handles_unit_and_invalid_vectors() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), None);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), None);
    }

    #[test]
    fn source_owner_analysis_frontier_is_bounded_to_available_complete_chunks() {
        assert!(source_owner_analysis_is_pending(true, false, 9_000, 13_000, 12_896),
            "the 9-12s source chunk is a real in-flight identity decision");
        assert!(!source_owner_analysis_is_pending(true, false, 12_000, 13_000, 12_896),
            "a partial 12-13s tail cannot wait for STOP to finalize itself");
        assert!(!source_owner_analysis_is_pending(true, false, 9_000, 11_900, 11_800),
            "capture without another complete chunk has no actionable frontier");
        assert!(!source_owner_analysis_is_pending(false, false, 9_000, 13_000, 12_896),
            "unknown/adaptive identity cannot manufacture an owner-analysis hold");
        assert!(!source_owner_analysis_is_pending(true, true, 9_000, 13_000, 12_896),
            "a terminal worker cannot keep endpointing alive");
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
