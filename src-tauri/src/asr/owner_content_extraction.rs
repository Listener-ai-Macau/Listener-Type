//! Optional full-session content recovery. This produces ordinary audio only:
//! neither an extracted waveform nor its text establishes speaker identity.
//!
//! This adapter intentionally does not split the bidirectional PS4 model into
//! independent live chunks. Missing/unverified assets, excessive duration and
//! resource pressure return an error so the caller retains its original PCM.

use kaldi_native_fbank::{istft_compute, stft_compute, IstftOptions, StftOptions, StftResult};
use ort::{
    session::{RunOptions, Session},
    value::TensorRef,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

const FFT: usize = 512;
const HOP: usize = 128;
const BINS: usize = FFT / 2 + 1;
const MEL_BINS: usize = 80;
const MIN_SAMPLES: usize = FFT + 1;
// Endpoint transport may append seconds of exact digital silence. PS4 is
// bidirectional: those otherwise information-free frames change its earlier
// output. Canonicalize only a long, strictly zero terminal pad internally;
// preserve the full source and restore every zero at its original coordinate.
const TERMINAL_PADDING_MIN_SAMPLES: usize = 16_000;
// Full-session recovery is bounded. Longer audio stays on the original route;
// it must never be silently cropped to fit this non-streaming model.
const MAX_SAMPLES: usize = super::content_consensus::MAX_CONTENT_SAMPLES;
pub(crate) const MAX_RECOVERY_PCM_BYTES: usize = MAX_SAMPLES * 2;
const AUTHOR_REVISION: &str = "8feb7a1b3da1264659f155c42bfb40a5be88d730";
const AUTHOR_CHECKPOINT: &str = "3d5621c89e4af7d6a42180604aa191962636826b244c86cc3ac5bc2508064196";
const ENCODER_SHA256: &str = "2b25cbc959b854ec2bed5f51bca2759a94c020f52288027dc37769fa36c54046";
const SEPARATOR_SHA256: &str = "5348fbf800da4d5fe8995c49f3ade8aa810bfe9f3b49b3046ee7c247c439569e";
const MEL_SHA256: &str = "8d3d92001ebf2e73cc7e4b5d7febc967a4661659a1c22b76140e6fdf0016d035";

#[derive(Deserialize)]
struct ModelManifest {
    author_revision: String,
    author_checkpoint_sha256: String,
    parity_passed: bool,
    encoder_sha256: String,
    separator_sha256: String,
    mel_sha256: String,
}

fn resource_root() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(root) = std::env::var_os("LISTENER_OWNER_CONTENT_FIXTURES") {
        return Ok(PathBuf::from(root));
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = executable
        .parent()
        .ok_or("content recovery executable has no parent")?
        .join("resources/models");
    if root.join("ps4-content-manifest.json").is_file() {
        return Ok(root);
    }
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/models"))
}

fn checked_asset(root: &Path, name: &str, expected: &str) -> Result<PathBuf, String> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid content recovery asset fingerprint".into());
    }
    let path = root.join(name);
    let mut file = std::fs::File::open(&path)
        .map_err(|e| format!("content recovery asset unavailable: {e}"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if format!("{:x}", hash.finalize()) != expected.to_ascii_lowercase() {
        return Err("content recovery asset fingerprint mismatch".into());
    }
    Ok(path)
}

fn normalized_pcm(pcm: &[u8]) -> Result<Vec<f32>, String> {
    if pcm.len() % 2 != 0 {
        return Err("content recovery PCM is not sample aligned".into());
    }
    let mut samples: Vec<f32> = pcm
        .chunks_exact(2)
        .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32)
        .collect();
    if !(MIN_SAMPLES..=MAX_SAMPLES).contains(&samples.len()) {
        return Err("content recovery requires a complete bounded session".into());
    }
    let peak = samples.iter().copied().map(f32::abs).fold(0.0, f32::max);
    if peak <= 0.0 {
        return Err("content recovery PCM is silent".into());
    }
    for value in &mut samples {
        *value /= peak;
    }
    Ok(samples)
}

fn spectral_options() -> StftOptions {
    let mut options = StftOptions::default();
    options.n_fft = FFT;
    options.win_length = FFT;
    options.hop_length = HOP;
    options.frame_opts.window_type = "hann".into();
    options.center = true;
    options.pad_mode = "reflect".into();
    options.normalized = false;
    options
}

fn spectrum_input(samples: &[f32]) -> Result<(usize, Vec<f32>), String> {
    let spectrum = stft_compute(&spectral_options(), samples)?;
    let frames = spectrum.num_frames;
    let plane = BINS * frames;
    let mut values = vec![0.0; 2 * plane];
    for frame in 0..frames {
        for bin in 0..BINS {
            values[bin * frames + frame] = spectrum.real[frame * BINS + bin];
            values[plane + bin * frames + frame] = spectrum.imag[frame * BINS + bin];
        }
    }
    Ok((frames, values))
}

fn model_signal_samples(samples: &[f32]) -> usize {
    let end = samples
        .iter()
        .rposition(|sample| *sample != 0.0)
        .map_or(0, |position| position + 1);
    if samples.len().saturating_sub(end) >= TERMINAL_PADDING_MIN_SAMPLES {
        end.max(MIN_SAMPLES).min(samples.len())
    } else {
        samples.len()
    }
}

fn restore_terminal_padding(
    original: &[f32],
    signal_samples: usize,
    mut recovered: Vec<f32>,
) -> Result<Vec<f32>, String> {
    if recovered.len() != signal_samples
        || signal_samples > original.len()
        || original[signal_samples..]
            .iter()
            .any(|sample| *sample != 0.0)
        || recovered.iter().any(|sample| !sample.is_finite())
    {
        return Err("content recovery cannot restore nonzero source audio".into());
    }
    recovered.resize(original.len(), 0.0);
    Ok(recovered)
}

fn reconstruct(values: &[f32], frames: usize, samples: usize) -> Result<Vec<f32>, String> {
    let plane = BINS
        .checked_mul(frames)
        .ok_or("content spectrum size overflow")?;
    if values.len() != 2 * plane || values.iter().any(|v| !v.is_finite()) || frames == 0 {
        return Err("invalid content recovery spectrum".into());
    }
    let mut spectrum = StftResult {
        real: vec![0.0; plane],
        imag: vec![0.0; plane],
        num_frames: frames,
        n_fft: FFT,
    };
    for frame in 0..frames {
        for bin in 0..BINS {
            spectrum.real[frame * BINS + bin] = values[bin * frames + frame];
            spectrum.imag[frame * BINS + bin] = values[plane + bin * frames + frame];
        }
    }
    // Library center=true removes both entire pads and floors length to a hop.
    // Torch's explicit length retains the final partial hop. Reconstruct the
    // padded signal then take the exact source range; never zero-fill the tail.
    let mut options = IstftOptions::from(&spectral_options());
    options.center = false;
    let padded = istft_compute(&options, &spectrum)?;
    padded
        .get(FFT / 2..FFT / 2 + samples)
        .map(|s| s.to_vec())
        .ok_or_else(|| "content recovery lost its source sample clock".into())
}

fn safe_pcm(samples: &[f32], expected: usize) -> Result<Vec<u8>, String> {
    if samples.len() != expected || samples.iter().any(|v| !v.is_finite()) {
        return Err("content recovery returned invalid samples".into());
    }
    let peak = samples.iter().copied().map(f32::abs).fold(0.0, f32::max);
    if peak <= f32::EPSILON {
        return Err("content recovery returned silence".into());
    }
    let gain = (0.95 / peak).min(1.0);
    // One gain for the complete file: preserve every time coordinate and avoid
    // the clipped samples observed in the author-output PCM conversion.
    Ok(samples
        .iter()
        .flat_map(|v| ((*v * gain * 32768.0).round() as i16).to_le_bytes())
        .collect())
}

fn reference_fbank(samples: &[f32], mel: &[f32]) -> Result<(usize, Vec<f32>), String> {
    use kaldi_native_fbank::rfft::Rfft;
    if mel.len() != BINS * MEL_BINS || mel.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err("invalid PS4 mel matrix".into());
    }
    let preemphasis: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, v)| *v - 0.97 * samples[if i == 0 { 1 } else { i - 1 }])
        .collect();
    let mut padded = Vec::with_capacity(samples.len() + FFT);
    padded.extend((1..=FFT / 2).rev().map(|i| preemphasis[i]));
    padded.extend_from_slice(&preemphasis);
    padded.extend((0..FFT / 2).map(|i| preemphasis[preemphasis.len() - 2 - i]));
    let frames = samples.len() / HOP + 1;
    let window: Vec<f32> = (0..FFT)
        .map(|i| 0.54 - 0.46 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos())
        .collect();
    let mut fft = Rfft::new(FFT, false);
    let mut work = vec![0.0; FFT];
    let mut features = vec![0.0; frames * MEL_BINS];
    for frame in 0..frames {
        for i in 0..FFT {
            work[i] = padded[frame * HOP + i] * window[i];
        }
        fft.compute(&mut work);
        for bin in 0..MEL_BINS {
            let mut power =
                work[0].powi(2) * mel[bin] + work[1].powi(2) * mel[(BINS - 1) * MEL_BINS + bin];
            for k in 1..BINS - 1 {
                power += (work[2 * k].powi(2) + work[2 * k + 1].powi(2)) * mel[k * MEL_BINS + bin];
            }
            features[frame * MEL_BINS + bin] = (power + 1e-8).ln();
        }
    }
    for bin in 0..MEL_BINS {
        let mean = (0..frames)
            .map(|f| features[f * MEL_BINS + bin])
            .sum::<f32>()
            / frames as f32;
        for frame in 0..frames {
            features[frame * MEL_BINS + bin] -= mean;
        }
    }
    Ok((frames, features))
}

fn resource_headroom(extra_bytes: u64) -> Result<(), String> {
    use windows::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetCurrentProcess,
    };
    let mut memory = MEMORYSTATUSEX::default();
    memory.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    unsafe { GlobalMemoryStatusEx(&mut memory) }.map_err(|e| e.to_string())?;
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    }
    .map_err(|e| e.to_string())?;
    if memory.ullAvailPhys < 2 * 1024u64.pow(3) + extra_bytes
        || counters.WorkingSetSize as u64 > 4 * 1024u64.pow(3)
    {
        return Err("content recovery memory protection".into());
    }
    Ok(())
}

pub(crate) fn extract_full_session(reference: &[u8], pcm: &[u8]) -> Result<Vec<u8>, String> {
    let samples = normalized_pcm(pcm)?;
    let reference = normalized_pcm(reference)?;
    let root = resource_root()?;
    let manifest: ModelManifest = serde_json::from_slice(
        &std::fs::read(root.join("ps4-content-manifest.json"))
            .map_err(|e| format!("validated PS4 content assets are not installed: {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    if !manifest.parity_passed
        || manifest.author_revision != AUTHOR_REVISION
        || manifest.author_checkpoint_sha256 != AUTHOR_CHECKPOINT
        || manifest.encoder_sha256 != ENCODER_SHA256
        || manifest.separator_sha256 != SEPARATOR_SHA256
        || manifest.mel_sha256 != MEL_SHA256
    {
        return Err("content recovery export is not validated".into());
    }
    let encoder_path = checked_asset(&root, "ps4-ecapa-fp32.onnx", &manifest.encoder_sha256)?;
    let separator_path = checked_asset(&root, "ps4-bsrnn-fp32.onnx", &manifest.separator_sha256)?;
    let mel_path = checked_asset(&root, "ps4-mel-257x80.f32", &manifest.mel_sha256)?;
    let bytes = encoder_path.metadata().map_err(|e| e.to_string())?.len()
        + separator_path.metadata().map_err(|e| e.to_string())?.len();
    resource_headroom(bytes * 2)?;
    let dll = crate::wake_phrase::onnx_runtime_dll_path()?;
    let _ = ort::init_from(dll)
        .map_err(|e| e.to_string())?
        .with_name("listener-content-recovery")
        .commit();
    let load = |path: &Path| {
        Session::builder()
            .map_err(|e| e.to_string())?
            // Each phase runs once. A reusable arena/pattern reserves memory
            // for a future inference which never happens in this worker.
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
            .map_err(|e| e.to_string())?
            .with_memory_pattern(false)
            .map_err(|e| e.to_string())?
            .with_intra_threads(8)
            .map_err(|e| e.to_string())?
            .with_inter_threads(1)
            .map_err(|e| e.to_string())?
            .commit_from_file(path)
            .map_err(|e| e.to_string())
    };
    let mel_bytes = std::fs::read(mel_path).map_err(|e| e.to_string())?;
    if mel_bytes.len() != BINS * MEL_BINS * 4 {
        return Err("invalid PS4 mel asset size".into());
    }
    let mel: Vec<f32> = mel_bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let (reference_frames, features) = reference_fbank(&reference, &mel)?;
    let signal_samples = model_signal_samples(&samples);
    let (frames, spectrum) = spectrum_input(&samples[..signal_samples])?;
    let run_options = Arc::new(RunOptions::new().map_err(|e| e.to_string())?);
    let (finished, cancel) = mpsc::channel();
    let watchdog_options = Arc::clone(&run_options);
    let watchdog = std::thread::spawn(move || {
        let begin = Instant::now();
        loop {
            match cancel.recv_timeout(Duration::from_millis(100)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if begin.elapsed() > Duration::from_secs(60) || resource_headroom(0).is_err() {
                let _ = watchdog_options.terminate();
                break;
            }
        }
    });
    let result = (|| {
        resource_headroom(0)?;
        // Own the 192-float result, then drop the output and encoder session
        // before allocating separator weights/activations. No audio is split.
        let embedding_values = {
            let mut encoder = load(&encoder_path)?;
            let features = TensorRef::from_array_view((
                [1usize, reference_frames, MEL_BINS],
                features.as_slice(),
            ))
            .map_err(|e| e.to_string())?;
            let output = encoder
                .run_with_options(ort::inputs!["fbank" => features], &run_options)
                .map_err(|e| e.to_string())?;
            let (shape, embedding) = output["embedding"]
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            if shape.as_ref() != [1i64, 192] || embedding.iter().any(|v| !v.is_finite()) {
                return Err("invalid PS4 embedding".into());
            }
            embedding.to_vec()
        };
        resource_headroom(separator_path.metadata().map_err(|e| e.to_string())?.len())?;
        let mut separator = load(&separator_path)?;
        let embedding =
            TensorRef::from_array_view(([1usize, 192usize], embedding_values.as_slice()))
                .map_err(|e| e.to_string())?;
        let spectrum =
            TensorRef::from_array_view(([1usize, 2usize, BINS, frames], spectrum.as_slice()))
                .map_err(|e| e.to_string())?;
        let output = separator
            .run_with_options(
                ort::inputs!["spectrum" => spectrum, "embedding" => embedding],
                &run_options,
            )
            .map_err(|e| e.to_string())?;
        let (shape, values) = output["separated_spectrum"]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        if shape.as_ref() != [1i64, 2, BINS as i64, frames as i64] {
            return Err("invalid PS4 spectrum shape".into());
        }
        resource_headroom(0)?;
        let recovered = reconstruct(values, frames, signal_samples)?;
        let complete = restore_terminal_padding(&samples, signal_samples, recovered)?;
        safe_pcm(&complete, samples.len())
    })();
    let _ = finished.send(());
    let _ = watchdog.join();
    result
}

pub(crate) fn recovery_pcm(original: Vec<u8>, verified_reference: Option<&[u8]>) -> Vec<u8> {
    recovery_pcm_with(original, verified_reference, extract_full_session)
}

fn recovery_pcm_with<F>(original: Vec<u8>, verified_reference: Option<&[u8]>, extract: F) -> Vec<u8>
where
    F: FnOnce(&[u8], &[u8]) -> Result<Vec<u8>, String>,
{
    let Some(reference) = verified_reference else {
        return original;
    };
    if original.len() % 2 != 0 || !(MIN_SAMPLES * 2..=MAX_SAMPLES * 2).contains(&original.len()) {
        return original;
    }
    match extract(reference, &original) {
        Ok(processed) if processed.len() == original.len() && processed.iter().any(|b| *b != 0) => {
            log::info!(
                "[asr] optional full-session content recovery prepared pcm_bytes={}",
                processed.len()
            );
            processed
        }
        Ok(_) => {
            log::warn!(
                "[asr] optional content recovery lost audio coverage; retaining original PCM"
            );
            original
        }
        Err(error) => {
            log::info!(
                "[asr] optional content recovery unavailable; retaining original PCM: {error}"
            );
            original
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires pinned public PS4 fixtures and a guarded native runtime"]
    fn owner_content_native_matches_author_frontend_and_waveform() {
        let root = PathBuf::from(
            std::env::var_os("LISTENER_OWNER_CONTENT_FIXTURES")
                .expect("pinned public fixture directory"),
        );
        let protocol: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("protocol.json")).unwrap()).unwrap();
        let floats = |name: &str| -> Vec<f32> {
            let bytes = std::fs::read(root.join(name)).unwrap();
            assert_eq!(bytes.len() % 4, 0);
            bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect()
        };
        let maximum_error = |left: &[f32], right: &[f32]| -> f32 {
            assert_eq!(left.len(), right.len());
            left.iter()
                .zip(right)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max)
        };
        let reference = std::fs::read(protocol["referencePcm"].as_str().unwrap()).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&reference)),
            protocol["referenceSha256"].as_str().unwrap()
        );
        let normalized = normalized_pcm(&reference).unwrap();
        assert!(maximum_error(&normalized, &floats("reference-normalized.f32")) < 0.000001);
        let mel = floats("ps4-mel-257x80.f32");
        let (_, features) = reference_fbank(&normalized, &mel).unwrap();
        let feature_error = maximum_error(&features, &floats("reference-fbank.f32"));
        println!("PS4 reference feature maximum_error={feature_error}");
        assert!(
            feature_error < 0.0005,
            "reference frontend must match the author"
        );
        for case in protocol["parityCases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let original = std::fs::read(case["pcm"].as_str().unwrap()).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(&original)),
                case["pcmSha256"].as_str().unwrap()
            );
            let (_, spectrum) = spectrum_input(&normalized_pcm(&original).unwrap()).unwrap();
            let spectral_error = maximum_error(&spectrum, &floats(&format!("{id}-spectrum.f32")));
            println!("PS4 {id} spectrum maximum_error={spectral_error}");
            assert!(
                spectral_error < 0.0001,
                "source frontend must match the author"
            );
            let expected = safe_pcm(
                &floats(&format!("{id}-onnx-waveform.f32")),
                original.len() / 2,
            )
            .unwrap();
            let started = Instant::now();
            let actual = extract_full_session(&reference, &original).unwrap();
            assert_eq!(
                actual.len(),
                original.len(),
                "every original sample must be covered"
            );
            let error = actual
                .chunks_exact(2)
                .zip(expected.chunks_exact(2))
                .map(|(a, b)| {
                    (i16::from_le_bytes(a.try_into().unwrap()) as i32
                        - i16::from_le_bytes(b.try_into().unwrap()) as i32)
                        .abs()
                })
                .max()
                .unwrap();
            println!(
                "PS4 {id} PCM maximum_error={error} elapsed_ms={}",
                started.elapsed().as_millis()
            );
            assert!(
                error <= 8,
                "native recovery must match the pinned author output"
            );
            std::fs::write(root.join(format!("{id}-rust.pcm")), actual).unwrap();
        }
    }

    #[test]
    fn owner_content_reconstruction_preserves_partial_hops_and_tail_samples() {
        for length in [23900, 79457, 48000, 513] {
            let values: Vec<f32> = (0..length)
                .map(|i| 0.4 * (i as f32 * 0.13).sin() + 0.2 * (i as f32 * 0.021).cos())
                .collect();
            let (frames, spectrum) = spectrum_input(&values).unwrap();
            let recovered = reconstruct(&spectrum, frames, length).unwrap();
            assert_eq!(recovered.len(), length);
            assert!(values
                .iter()
                .zip(&recovered)
                .all(|(a, b)| (*a - *b).abs() < 0.00001));
            assert!(
                recovered[length - 1].abs() > 0.0001,
                "partial tail must not be padded with zeros"
            );
        }
    }

    #[test]
    fn owner_content_terminal_padding_keeps_quiet_internal_and_leading_samples() {
        let mut original = vec![0.0; 79457];
        // Tiny nonzero PCM, interior and leading silence remain part of the
        // original signal. No RMS/VAD threshold decides what to remove.
        original[1000] = 1.0 / 32768.0;
        original[60000] = -1.0 / 32768.0;
        original[79456] = 0.2;
        assert_eq!(model_signal_samples(&original), original.len());
        for padding in [16000, 16001, 48000, 96000] {
            let mut padded = original.clone();
            padded.resize(original.len() + padding, 0.0);
            let signal = model_signal_samples(&padded);
            assert_eq!(signal, original.len());
            assert_eq!(&padded[..signal], original.as_slice());
            let restored = restore_terminal_padding(&padded, signal, original.clone()).unwrap();
            assert_eq!(restored, padded);
            assert_eq!(
                safe_pcm(&restored, padded.len()).unwrap().len(),
                padded.len() * 2
            );
        }
        let mut quiet_tail = original.clone();
        quiet_tail.resize(original.len() + 48000, 0.0);
        *quiet_tail.last_mut().unwrap() = 1.0 / 32768.0;
        assert_eq!(model_signal_samples(&quiet_tail), quiet_tail.len());
        let mut short_padding = original.clone();
        short_padding.resize(original.len() + 15999, 0.0);
        assert_eq!(model_signal_samples(&short_padding), short_padding.len());
    }

    #[test]
    fn owner_content_terminal_padding_refuses_to_replace_meaningful_audio() {
        let original = [0.1, 0.0, 0.2, 0.0];
        assert!(restore_terminal_padding(&original, 2, vec![0.1, 0.0]).is_err());
        assert!(restore_terminal_padding(&original, 3, vec![0.1, 0.0]).is_err());
        assert!(restore_terminal_padding(&original, 5, vec![0.0; 5]).is_err());
        assert!(restore_terminal_padding(&original, 3, vec![f32::NAN; 3]).is_err());
        assert_eq!(
            restore_terminal_padding(&original, 3, vec![0.1, 0.0, 0.2]).unwrap(),
            original
        );
    }

    #[test]
    fn owner_content_pcm_conversion_preserves_all_samples_without_clipping() {
        let samples = [1.2919, -1.2919, 0.0001, 0.4, -0.7];
        let pcm = safe_pcm(&samples, samples.len()).unwrap();
        assert_eq!(pcm.len(), samples.len() * 2);
        let actual: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]))
            .collect();
        assert_eq!(actual[0], 31130);
        assert_eq!(actual[1], -31130);
        assert_ne!(actual[2], 0);
        assert!(actual.iter().all(|v| *v != i16::MIN && *v != i16::MAX));
        assert!(safe_pcm(&[f32::NAN], 1).is_err());
        assert!(safe_pcm(&[0.0], 1).is_err());
        assert!(safe_pcm(&samples, samples.len() + 1).is_err());
    }

    #[test]
    fn owner_content_recovery_refuses_truncation_and_malformed_sources() {
        assert!(normalized_pcm(&vec![1; MAX_SAMPLES * 2 + 2]).is_err());
        assert!(normalized_pcm(&vec![1; MAX_SAMPLES * 2 - 1]).is_err());
        assert!(normalized_pcm(&vec![0; 16000]).is_err());
    }

    #[test]
    fn owner_content_failure_missing_reference_and_incomplete_outputs_keep_original_pcm() {
        let original = vec![1; 32000];
        let reference = vec![2; 32000];
        assert_eq!(
            recovery_pcm_with(original.clone(), None, |_, _| panic!(
                "no verified reference"
            )),
            original
        );
        assert_eq!(
            recovery_pcm_with(original.clone(), Some(&reference), |_, _| Err(
                "memory protection".into()
            )),
            original
        );
        assert_eq!(
            recovery_pcm_with(original.clone(), Some(&reference), |_, _| Ok(vec![
                0;
                32000
            ])),
            original
        );
        assert_eq!(
            recovery_pcm_with(original.clone(), Some(&reference), |_, _| Ok(vec![
                3;
                31998
            ])),
            original
        );
        let long = vec![1; MAX_SAMPLES * 2 + 2];
        assert_eq!(
            recovery_pcm_with(long.clone(), Some(&reference), |_, _| panic!(
                "long source must not be cropped"
            )),
            long
        );
        let processed = vec![3; 32000];
        assert_eq!(
            recovery_pcm_with(original, Some(&reference), |_, _| Ok(processed.clone())),
            processed
        );
    }
}
