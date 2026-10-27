//! Whisper language detection, using the selected Foundry model's own weights.
//! An unspecified Foundry file-ASR language is not reliable automatic detection.
//! Decode only the SOT language logits, then let AudioClient transcribe normally.
use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

use anyhow::{bail, ensure, Context, Result};
use ort::{
    memory::Allocator,
    session::{RunOptions, Session, SessionInputValue},
    value::{Tensor, TensorRef},
};
use rustfft::{num_complex::Complex, FftPlanner};
use serde_json::Value;

const SAMPLES: usize = 480_000;
const FFT: usize = 400;
const HOP: usize = 160;
const FRAMES: usize = SAMPLES / HOP;

#[derive(Debug)]
pub(super) struct DetectedLanguage {
    pub language: String,
    pub probability: f32,
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(
        &std::fs::read(path)
            .with_context(|| format!("read language detection config {}", path.display()))?,
    )
    .context("parse language detection config")
}

fn model_file(root: &Path, config: &Value, pointer: &str) -> Result<PathBuf> {
    let name = config
        .pointer(pointer)
        .and_then(Value::as_str)
        .context("Whisper language detection model filename missing")?;
    ensure!(
        Path::new(name).components().count() == 1 && !name.contains(['/', '\\']),
        "Whisper language detection filename is not local"
    );
    Ok(root.join(name))
}

fn language_tokens(tokenizer: &Value) -> Result<(i32, Vec<(usize, String)>)> {
    let tokens = tokenizer["added_tokens"]
        .as_array()
        .context("Whisper tokenizer tokens missing")?;
    let mut sot = None;
    let mut languages = Vec::new();
    for token in tokens {
        let Some(text) = token["content"].as_str() else {
            continue;
        };
        let Some(id) = token["id"].as_u64() else {
            continue;
        };
        if text == "<|startoftranscript|>" {
            sot = Some(i32::try_from(id)?);
        }
        if let Some(code) = text.strip_prefix("<|").and_then(|s| s.strip_suffix("|>")) {
            if (2..=3).contains(&code.len()) && code.bytes().all(|c| c.is_ascii_lowercase()) {
                languages.push((id as usize, code.to_owned()));
            }
        }
    }
    ensure!(
        !languages.is_empty(),
        "Whisper model has no language tokens"
    );
    Ok((sot.context("Whisper SOT token missing")?, languages))
}

fn hz_to_mel(hz: f64) -> f64 {
    if hz < 1000.0 {
        hz / (200.0 / 3.0)
    } else {
        15.0 + (hz / 1000.0).ln() / (6.4_f64.ln() / 27.0)
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    if mel < 15.0 {
        mel * (200.0 / 3.0)
    } else {
        1000.0 * ((mel - 15.0) * (6.4_f64.ln() / 27.0)).exp()
    }
}

/// Whisper's periodic Hann, centered STFT and Slaney-normalized log mel.
/// Keep the first 30 seconds and pad before reflecting the first STFT window.
fn log_mel(pcm: &[u8], bins: usize) -> Result<Vec<f32>> {
    ensure!(
        !pcm.is_empty() && pcm.len() % 2 == 0,
        "empty or incomplete language detection PCM"
    );
    ensure!(
        matches!(bins, 80 | 128),
        "unsupported Whisper mel width {bins}"
    );
    let mut audio = vec![0.0_f64; SAMPLES];
    for (target, bytes) in audio.iter_mut().zip(pcm.chunks_exact(2)) {
        *target = i16::from_le_bytes([bytes[0], bytes[1]]) as f64 / 32768.0;
    }
    let edges: Vec<f64> = (0..bins + 2)
        .map(|i| mel_to_hz(i as f64 * hz_to_mel(8000.0) / (bins + 1) as f64))
        .collect();
    let filters: Vec<Vec<(usize, f64)>> = (0..bins)
        .map(|bin| {
            (0..=FFT / 2)
                .filter_map(|k| {
                    let hz = k as f64 * 16000.0 / FFT as f64;
                    let weight = ((hz - edges[bin]) / (edges[bin + 1] - edges[bin]))
                        .min((edges[bin + 2] - hz) / (edges[bin + 2] - edges[bin + 1]))
                        .max(0.0)
                        * 2.0
                        / (edges[bin + 2] - edges[bin]);
                    (weight > 0.0).then_some((k, weight))
                })
                .collect()
        })
        .collect();
    let window: Vec<f64> = (0..FFT)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FFT as f64).cos())
        .collect();
    let fft = FftPlanner::<f64>::new().plan_fft_forward(FFT);
    let mut frame = vec![Complex::new(0.0, 0.0); FFT];
    let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    let mut power = [0.0_f64; FFT / 2 + 1];
    let mut mel = vec![-10.0_f32; bins * FRAMES];
    let mut maximum = -10.0_f32;
    for t in 0..FRAMES {
        for i in 0..FFT {
            let index = t as isize * HOP as isize + i as isize - (FFT / 2) as isize;
            let index = index.unsigned_abs();
            let index = if index >= SAMPLES {
                2 * SAMPLES - 2 - index
            } else {
                index
            };
            frame[i] = Complex::new(audio[index] * window[i], 0.0);
        }
        // Trailing zero padding has exactly the same floor without an FFT.
        if frame.iter().all(|x| x.re == 0.0) {
            continue;
        }
        fft.process_with_scratch(&mut frame, &mut scratch);
        for (p, f) in power.iter_mut().zip(&frame) {
            *p = f.norm_sqr();
        }
        for (bin, filter) in filters.iter().enumerate() {
            let value = filter
                .iter()
                .map(|&(k, w)| power[k] * w)
                .sum::<f64>()
                .max(1e-10)
                .log10() as f32;
            mel[bin * FRAMES + t] = value;
            maximum = maximum.max(value);
        }
    }
    for value in &mut mel {
        *value = (value.max(maximum - 8.0) + 4.0) / 4.0;
    }
    Ok(mel)
}

fn session(path: &Path) -> Result<Session> {
    Ok(Session::builder()?
        .with_intra_threads(2)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .commit_from_file(path)?)
}

fn configured_name(config: &Value, pointer: &str, layer: usize) -> Result<String> {
    Ok(config
        .pointer(pointer)
        .and_then(Value::as_str)
        .context("Whisper language detection tensor name missing")?
        .replace("%d", &layer.to_string()))
}

pub(super) fn detect(
    root: &Path,
    runtime_dll: &Path,
    audio_path: &Path,
    timeout: Duration,
) -> Result<DetectedLanguage> {
    let started = Instant::now();
    let config = read_json(&root.join("genai_config.json"))?;
    ensure!(
        config["model"]["type"] == "whisper",
        "automatic language requires a Whisper model"
    );
    let preprocessor = read_json(&root.join("preprocessor_config.json"))?;
    ensure!(
        preprocessor["sampling_rate"] == 16000
            && preprocessor["n_fft"] == FFT
            && preprocessor["hop_length"] == HOP,
        "unsupported Whisper audio frontend"
    );
    let bins = preprocessor["feature_size"]
        .as_u64()
        .context("Whisper mel width missing")? as usize;
    let (sot, tokens) = language_tokens(&read_json(&root.join("tokenizer.json"))?)?;
    let wav = std::fs::read(audio_path).context("read automatic-language audio")?;
    let pcm =
        denzic_audio_v1_core::read_wav_pcm16le(&wav).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let features = log_mel(&pcm, bins)?;
    // Use the already prepared runtime. No speaker model or extra download is
    // required. ORT's process-wide initializer reuses an existing newer runtime.
    let _ = ort::init_from(runtime_dll)?
        .with_name("listener-local-language")
        .commit();
    let options = Arc::new(RunOptions::new()?);
    let (finished, wait) = mpsc::channel::<()>();
    let terminate = Arc::clone(&options);
    let remaining = timeout.saturating_sub(started.elapsed());
    ensure!(
        !remaining.is_zero(),
        "automatic language detection timed out"
    );
    std::thread::spawn(move || {
        if matches!(
            wait.recv_timeout(remaining),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            let _ = terminate.terminate();
        }
    });
    // Dropping this sender wakes the watchdog on every success/error path.
    let _finished = finished;
    let mut encoder = session(&model_file(root, &config, "/model/encoder/filename")?)?;
    let mut decoder = session(&model_file(root, &config, "/model/decoder/filename")?)?;
    ensure!(
        started.elapsed() < timeout,
        "automatic language model loading timed out"
    );
    let audio = TensorRef::from_array_view(([1usize, bins, FRAMES], features.as_slice()))?;
    let input_name = config
        .pointer("/model/encoder/inputs/audio_features")
        .and_then(Value::as_str)
        .context("Whisper audio input name missing")?;
    let encoded = encoder.run_with_options(ort::inputs![input_name => audio], &options)?;
    let input_name = config
        .pointer("/model/decoder/inputs/input_ids")
        .and_then(Value::as_str)
        .context("Whisper token input name missing")?;
    let mut inputs: Vec<(String, SessionInputValue<'_>)> = vec![(
        input_name.into(),
        Tensor::from_array(([1usize, 1], vec![sot]))?.into(),
    )];
    let layers = config
        .pointer("/model/decoder/num_hidden_layers")
        .and_then(Value::as_u64)
        .context("Whisper layer count missing")? as usize;
    ensure!((1..=32).contains(&layers), "invalid Whisper layer count");
    for layer in 0..layers {
        for kind in ["key", "value"] {
            let output_name = configured_name(
                &config,
                &format!("/model/encoder/outputs/cross_present_{kind}_names"),
                layer,
            )?;
            let cross = encoded
                .get(&output_name)
                .context("Whisper cross attention output missing")?;
            let (shape, _) = cross.try_extract_tensor::<f32>()?;
            ensure!(
                shape.len() == 4 && shape[0] == 1 && shape.iter().all(|&n| n > 0),
                "invalid Whisper cross attention shape"
            );
            let cross_name = configured_name(
                &config,
                &format!("/model/decoder/inputs/cross_past_{kind}_names"),
                layer,
            )?;
            inputs.push((cross_name, cross.into()));
            let self_name = configured_name(
                &config,
                &format!("/model/decoder/inputs/past_{kind}_names"),
                layer,
            )?;
            inputs.push((
                self_name,
                // First decoder step has no self-attention history. ORT's
                // allocator supports a zero-length axis; from_array rejects it.
                Tensor::<f32>::new(
                    &Allocator::default(),
                    [1usize, shape[1] as usize, 0, shape[3] as usize],
                )?
                .into(),
            ));
        }
    }
    let output = decoder.run_with_options(inputs, &options)?;
    let name = config
        .pointer("/model/decoder/outputs/logits")
        .and_then(Value::as_str)
        .context("Whisper language logits missing")?;
    let (shape, logits) = output
        .get(name)
        .context("Whisper language logits absent")?
        .try_extract_tensor::<f32>()?;
    ensure!(
        shape.len() == 3 && shape[0] == 1 && shape[1] == 1,
        "invalid Whisper language logits shape"
    );
    let mut best = None;
    for (id, language) in &tokens {
        let value = *logits
            .get(*id)
            .context("Whisper language token exceeds vocabulary")?;
        ensure!(value.is_finite(), "non-finite Whisper language score");
        if best
            .as_ref()
            .is_none_or(|(_, score): &(String, f32)| value > *score)
        {
            best = Some((language.clone(), value));
        }
    }
    let Some((language, score)) = best else {
        bail!("no Whisper language scores")
    };
    let sum: f32 = tokens
        .iter()
        .map(|(id, _)| (logits[*id] - score).exp())
        .sum();
    Ok(DetectedLanguage {
        language,
        probability: 1.0 / sum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_frontend_is_finite_and_token_selection_excludes_controls() {
        let features = log_mel(&vec![0; 640], 80).unwrap();
        assert_eq!(features.len(), 80 * 3000);
        assert!(features.iter().all(|&x| x == -1.5));
        let (sot, langs) = language_tokens(&serde_json::json!({"added_tokens": [
            {"id": 10, "content": "<|startoftranscript|>"},
            {"id": 11, "content": "<|en|>"}, {"id": 12, "content": "<|zh|>"},
            {"id": 13, "content": "<|translate|>"}]}))
        .unwrap();
        assert_eq!(sot, 10);
        assert_eq!(langs, [(11, "en".into()), (12, "zh".into())]);
    }

    #[test]
    fn whisper_frontend_matches_fixed_slaney_hann_golden() {
        // Independent NumPy Whisper frontend: one second of 440 Hz PCM,
        // amplitude 6000, then pad to 30 seconds. Includes both boundaries.
        let pcm: Vec<u8> = (0..16000)
            .flat_map(|i| {
                let sample = (6000.0
                    * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16000.0).sin())
                .round() as i16;
                sample.to_le_bytes()
            })
            .collect();
        let features = log_mel(&pcm, 80).unwrap();
        for (bin, frame, expected) in [
            (0, 0, 0.76514924),
            (0, 40, -0.77992904),
            (10, 40, 1.1306058),
            (11, 40, 1.2200708),
            (12, 40, 1.0753889),
            (40, 40, -0.77992904),
            (11, 100, 1.0747893),
            (79, 2999, -0.77992904),
        ] {
            assert!(
                (features[bin * FRAMES + frame] - expected).abs() < 0.0002,
                "Whisper frontend differs at mel {bin}, frame {frame}"
            );
        }
    }

    #[test]
    #[ignore = "requires existing private Foundry model cache and two public PCM fixtures; no download"]
    fn native_same_weights_detect_both_fixture_languages() {
        let root =
            PathBuf::from(std::env::var_os("LISTENER_FOUNDRY_LANGUAGE_MODEL").expect("model root"));
        let runtime = PathBuf::from(
            std::env::var_os("LISTENER_FOUNDRY_LANGUAGE_RUNTIME").expect("runtime DLL"),
        );
        for (var, expected) in [
            ("LISTENER_FOUNDRY_LANGUAGE_EN", "en"),
            ("LISTENER_FOUNDRY_LANGUAGE_ZH", "zh"),
        ] {
            let wav = PathBuf::from(std::env::var_os(var).expect("fixture WAV"));
            let start = Instant::now();
            let result = detect(&root, &runtime, &wav, Duration::from_secs(20)).unwrap();
            println!(
                "native language: {expected} => {result:?}; elapsed={:?}",
                start.elapsed()
            );
            assert_eq!(result.language, expected);
            assert!(result.probability > 0.95);
        }
    }
}
