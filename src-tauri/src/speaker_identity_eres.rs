//! Model-specific identity evidence. This module never grants transcript authority.
//!
//! ERes vectors cannot share a bank with CAM++ or the WeSep conditioning vector,
//! even when their dimensions happen to agree. The caller must separately prove
//! speech quality, enrollment provenance and the exact source-audio occurrence.

use kaldi_native_fbank::{
    online::{FeatureComputer, OnlineFeature},
    FbankComputer, FbankOptions,
};
use ort::{session::{RunOptions, Session}, value::TensorRef};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) const DIMENSION: usize = 192;
pub(crate) const FRONTEND_ID: &str = "kaldi-16k-povey-25ms-10ms-80mel-dither0-cmn-v1";
// Fixed export of the strict-loaded official v1.0.2 checkpoint. Python/ONNX
// parity is recorded separately; the native frontend still needs validation.
pub(crate) const MODEL_SHA256: &str =
    "344CD74516083546BF8756AD5FBCFB8AFE7AB63C277771D62E5536CEC72C6BFC";
const SAMPLE_RATE: usize = 16_000;
const MEL_BINS: usize = 80;
pub(crate) const MODEL_NAME: &str = "eres2netv2-enrollment-embedding-v1.0.2.onnx";

/// Loaded outside capture/preview callbacks. The immutable model and reference
/// never learn from a dictation or from another model's decisions.
pub(crate) fn local_runtime() -> Result<std::sync::Arc<parking_lot::Mutex<EresRuntime>>, String> {
    static RUNTIME: once_cell::sync::Lazy<parking_lot::Mutex<Option<std::sync::Arc<parking_lot::Mutex<EresRuntime>>>>> =
        once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(None));
    let mut slot = RUNTIME.lock();
    if let Some(runtime) = slot.as_ref() { return Ok(std::sync::Arc::clone(runtime)); }
    let root = crate::persistence::speaker_verification_root().map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let installed = executable.parent().ok_or("ERes executable has no parent")?
        .join("resources/models").join(MODEL_NAME);
    let model = if installed.is_file() { installed }
        else { std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/models").join(MODEL_NAME) };
    let runtime = std::sync::Arc::new(parking_lot::Mutex::new(EresRuntime::load(&model, &root.join("onnxruntime.dll"))?));
    *slot = Some(std::sync::Arc::clone(&runtime));
    Ok(runtime)
}

pub(crate) fn enrollment_embedding(pcm: &[u8]) -> Result<EresEmbedding, String> {
    local_runtime()?.lock().embedding(pcm)
}

#[derive(Clone, Debug)]
pub(crate) struct EresEmbedding(Vec<f32>);

impl EresEmbedding {
    /// Protected persistence must supply both identities. Dimension alone is
    /// deliberately insufficient to reconstruct this model-specific value.
    pub(crate) fn from_bound_values(
        model_sha256: &str,
        frontend_id: &str,
        values: Vec<f32>,
    ) -> Result<Self, String> {
        if model_sha256 != MODEL_SHA256 || frontend_id != FRONTEND_ID {
            return Err("ERes voiceprint model or frontend is incompatible".into());
        }
        Self::from_model_output(values)
    }

    fn from_model_output(mut values: Vec<f32>) -> Result<Self, String> {
        if values.len() != DIMENSION || values.iter().any(|value| !value.is_finite()) {
            return Err("ERes voiceprint dimension or values are invalid".into());
        }
        let norm = values.iter().map(|value| (*value as f64).powi(2)).sum::<f64>().sqrt();
        if norm <= f64::EPSILON || !norm.is_finite() {
            return Err("ERes voiceprint norm is invalid".into());
        }
        for value in &mut values { *value = (*value as f64 / norm) as f32; }
        Ok(Self(values))
    }

    pub(crate) fn values(&self) -> &[f32] { &self.0 }

    pub(crate) fn cosine(&self, other: &Self) -> f32 {
        self.0.iter().zip(&other.0).map(|(left, right)| left * right).sum()
    }
}

/// Matches the fixed model's official Kaldi input, including dynamic frame
/// count. The separator encoder uses Hamming and 300 padded frames and cannot
/// provide features for this model.
pub(crate) fn fbank(pcm: &[u8]) -> Result<(usize, Vec<f32>), String> {
    if pcm.len() % 2 != 0 || pcm.len() < 35 * 32 || pcm.len() > 30 * SAMPLE_RATE * 2 {
        return Err("ERes audio must contain 35 ms to 30 s of complete PCM16 samples".into());
    }
    let samples = pcm.chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32)
        .collect::<Vec<_>>();
    if samples.iter().all(|sample| *sample == 0.0) {
        return Err("ERes audio contains only digital silence".into());
    }
    let mut options = FbankOptions::default();
    options.frame_opts.samp_freq = SAMPLE_RATE as f32;
    options.frame_opts.frame_length_ms = 25.0;
    options.frame_opts.frame_shift_ms = 10.0;
    options.frame_opts.dither = 0.0;
    options.frame_opts.preemph_coeff = 0.97;
    options.frame_opts.remove_dc_offset = true;
    options.frame_opts.window_type = "povey".into();
    options.frame_opts.round_to_power_of_two = true;
    options.frame_opts.snip_edges = true;
    options.mel_opts.num_bins = MEL_BINS;
    options.mel_opts.low_freq = 20.0;
    options.mel_opts.high_freq = 0.0;
    options.use_energy = false;
    options.raw_energy = true;
    // This Rust port floors log-energy at 1e-20. The model's Torchaudio
    // Kaldi frontend floors mel power at float32 epsilon instead. Apply that
    // model contract before CMN; silence cannot acquire a 30-log-unit error.
    options.use_log_fbank = false;
    options.use_power = true;
    let computer = FbankComputer::new(options)
        .map_err(|err| format!("create ERes fbank failed: {err}"))?;
    let mut online = OnlineFeature::new(FeatureComputer::Fbank(computer));
    online.accept_waveform(SAMPLE_RATE as f32, &samples);
    online.input_finished();
    for frame in &mut online.features {
        for value in frame { *value = value.max(f32::EPSILON).ln(); }
    }
    let frame_count = online.features.len();
    if frame_count < 2 || online.features.iter().any(|frame| frame.len() != MEL_BINS) {
        return Err("ERes fbank frame shape is invalid".into());
    }
    let mut means = [0.0_f64; MEL_BINS];
    for frame in &online.features {
        for (mean, value) in means.iter_mut().zip(frame) { *mean += *value as f64; }
    }
    for mean in &mut means { *mean /= frame_count as f64; }
    let values = online.features.iter().flat_map(|frame| {
        frame.iter().zip(means).map(|(value, mean)| (*value as f64 - mean) as f32)
    }).collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err("ERes fbank contains a non-finite feature".into());
    }
    Ok((frame_count, values))
}

pub(crate) struct EresRuntime {
    session: Session,
    options: RunOptions,
}

impl EresRuntime {
    /// Loading is explicit; the caller owns model availability and lifecycle.
    /// Missing assets are an error, never a request to download during speech.
    pub(crate) fn load(model_path: &Path, runtime_dll: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(model_path)
            .map_err(|err| format!("read ERes model failed: {err}"))?;
        let actual_sha = format!("{:X}", Sha256::digest(&bytes));
        if !actual_sha.eq_ignore_ascii_case(MODEL_SHA256) {
            return Err("ERes model hash is incompatible".into());
        }
        let builder = ort::init_from(runtime_dll)
            .map_err(|err| format!("load ERes ONNX runtime failed: {err}"))?;
        let _ = builder.with_name("listener-owner-identity").commit();
        let session = Session::builder()
            .map_err(|err| format!("create ERes session failed: {err}"))?
            .with_intra_threads(2)
            .map_err(|err| format!("configure ERes threads failed: {err}"))?
            .with_inter_threads(1)
            .map_err(|err| format!("configure ERes scheduler failed: {err}"))?
            .commit_from_memory(&bytes)
            .map_err(|err| format!("load ERes model failed: {err}"))?;
        let mut options = RunOptions::new()
            .map_err(|err| format!("create ERes run options failed: {err}"))?;
        options.add_config_entry("memory.enable_memory_arena_shrinkage", "cpu:0")
            .map_err(|err| format!("configure ERes memory reclamation failed: {err}"))?;
        Ok(Self { session, options })
    }

    pub(crate) fn embedding(&mut self, pcm: &[u8]) -> Result<EresEmbedding, String> {
        let (frames, features) = fbank(pcm)?;
        let tensor = TensorRef::from_array_view(([1usize, frames, MEL_BINS], features.as_slice()))
            .map_err(|err| format!("create ERes input tensor failed: {err}"))?;
        let outputs = self.session.run_with_options(ort::inputs!["fbank" => tensor], &self.options)
            .map_err(|err| format!("run ERes identity failed: {err}"))?;
        let (shape, values) = outputs["embedding"].try_extract_tensor::<f32>()
            .map_err(|err| format!("decode ERes identity failed: {err}"))?;
        if shape.as_ref() != [1_i64, DIMENSION as i64] {
            return Err(format!("unexpected ERes output shape: {shape:?}"));
        }
        EresEmbedding::from_model_output(values.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_vectors_cannot_cross_model_or_frontend_boundaries() {
        let values = vec![1.0; DIMENSION];
        assert!(EresEmbedding::from_bound_values("wesep-192", FRONTEND_ID, values.clone()).is_err());
        assert!(EresEmbedding::from_bound_values(MODEL_SHA256, "hamming-padded-300", values.clone()).is_err());
        assert!(EresEmbedding::from_bound_values(MODEL_SHA256, FRONTEND_ID, vec![0.0; DIMENSION]).is_err());
        assert!(EresEmbedding::from_bound_values(MODEL_SHA256, FRONTEND_ID, vec![f32::NAN; DIMENSION]).is_err());
        let valid = EresEmbedding::from_bound_values(MODEL_SHA256, FRONTEND_ID, values).unwrap();
        assert!((valid.cosine(&valid) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn malformed_audio_does_not_become_identity_evidence() {
        assert!(fbank(&[0; 32]).is_err());
        assert!(fbank(&[1; 1601]).is_err());
        assert!(fbank(&vec![0; 32_000]).is_err());
    }

    #[test]
    #[ignore = "requires the fixed local PCM/golden fixtures and ONNX runtime"]
    fn native_frontend_and_identity_match_fixed_golden_cases() {
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(
            std::env::var_os("LISTENER_ERES_FIXTURES").expect("fixture manifest required")
        ).unwrap()).unwrap();
        let dll = std::env::var_os("LISTENER_ERES_RUNTIME").expect("runtime DLL required");
        let model = Path::new(manifest["model_onnx"]["path"].as_str().unwrap());
        let mut runtime = EresRuntime::load(model, Path::new(&dll)).unwrap();
        let mut reference = None;
        for item in manifest["items"].as_array().unwrap() {
            let id = item["id"].as_str().unwrap();
            let pcm = std::fs::read(item["pcm"]["path"].as_str().unwrap()).unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(&pcm)), item["pcm"]["sha256"].as_str().unwrap());
            let (frames, features) = fbank(&pcm).unwrap();
            assert_eq!(frames as u64, item["golden_fbank"]["shape"][1].as_u64().unwrap());
            let golden_bytes = std::fs::read(item["golden_fbank"]["path"].as_str().unwrap()).unwrap();
            let golden: Vec<f32> = golden_bytes.chunks_exact(4)
                .map(|value| f32::from_le_bytes(value.try_into().unwrap())).collect();
            assert_eq!(features.len(), golden.len());
            let feature_error = features.iter().zip(&golden)
                .map(|(left,right)| (left-right).abs()).fold(0.0_f32, f32::max);
            let embedding = runtime.embedding(&pcm).unwrap();
            let golden_json: serde_json::Value = serde_json::from_slice(&std::fs::read(
                item["normalized_embedding_json"]["path"].as_str().unwrap()).unwrap()).unwrap();
            let golden_values: Vec<f32> = golden_json["onnx_normalized_embedding"].as_array().unwrap()
                .iter().map(|value| value.as_f64().unwrap() as f32).collect();
            let embedding_error = embedding.values().iter().zip(&golden_values)
                .map(|(left,right)| (left-right).abs()).fold(0.0_f32, f32::max);
            println!("{id}: frames={frames} feature_max_error={feature_error:.8} embedding_max_error={embedding_error:.8}");
            assert!(feature_error < 0.002, "{id}: native Kaldi frontend differs from the fixed golden contract");
            assert!(embedding_error < 0.0001, "{id}: native embedding differs from the fixed ONNX golden");
            if id == "reference" { reference = Some(embedding); }
            else {
                let score = embedding.cosine(reference.as_ref().unwrap());
                println!("{id}: three_wake_cosine={score:.8}");
                assert_eq!(score >= 0.36, id != "background_vegetable",
                    "fixed operating point must retain all owner cases and reject the labelled background");
            }
        }
    }
}
