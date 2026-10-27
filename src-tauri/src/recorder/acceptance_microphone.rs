//! Explicit synthetic input for a noninteractive, disposable customer instance.
//! Excluded from ordinary builds. Invalid setup fails closed before CPAL starts;
//! this must never open an Owner microphone or turn a fixture into live evidence.

use super::{RecorderError, TARGET_SAMPLE_RATE};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HANDLE,
    System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, UOI_NAME, USEROBJECTFLAGS,
    },
};

const FRAME_SAMPLES: usize = 640;
const FRAME_TIME: Duration = Duration::from_millis(40);
const MAX_WAV_BYTES: u64 = 2_000_000;

fn error(message: impl ToString) -> RecorderError {
    RecorderError::EngineFailed(format!(
        "isolated synthetic microphone: {}",
        message.to_string()
    ))
}

fn private_station() -> Result<String, RecorderError> {
    unsafe {
        let station = GetProcessWindowStation().map_err(error)?;
        let handle = HANDLE(station.0);
        let mut flags = USEROBJECTFLAGS::default();
        GetUserObjectInformationW(
            handle,
            UOI_FLAGS,
            Some((&mut flags as *mut USEROBJECTFLAGS).cast()),
            std::mem::size_of_val(&flags) as u32,
            None,
        )
        .map_err(error)?;
        let mut name = [0_u16; 256];
        GetUserObjectInformationW(
            handle,
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            std::mem::size_of_val(&name) as u32,
            None,
        )
        .map_err(error)?;
        let name = String::from_utf16_lossy(
            &name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())],
        );
        validate_station(&name, flags.dwFlags & 1 != 0)?;
        Ok(name)
    }
}

fn validate_station(name: &str, visible: bool) -> Result<(), RecorderError> {
    if visible || !name.starts_with("ListenerNativeReplay_") {
        return Err(error("requires a private, non-visible window station"));
    }
    Ok(())
}

/// Resolve only; the customer may prepare the next waveform after app startup.
pub(super) fn configured_input() -> Result<Option<PathBuf>, RecorderError> {
    let Some(wav) = std::env::var_os("LISTENER_TYPE_ACCEPTANCE_MIC_WAV") else {
        return Ok(None);
    };
    let station = private_station()?;
    let data = std::env::var_os("LISTENER_TYPE_DATA_DIR")
        .ok_or_else(|| error("missing isolated data directory"))?;
    let data = PathBuf::from(data).canonicalize().map_err(error)?;
    let nonce = std::env::var("LISTENER_TYPE_ACCEPTANCE_MIC_NONCE").map_err(error)?;
    let marker: serde_json::Value = serde_json::from_slice(
        &std::fs::read(data.join("acceptance-microphone.json")).map_err(error)?,
    )
    .map_err(error)?;
    if nonce.len() != 32
        || !nonce.bytes().all(|c| c.is_ascii_hexdigit())
        || marker["nonce"].as_str() != Some(&nonce)
        || marker["windowStation"].as_str() != Some(&station)
    {
        return Err(error("private session marker does not match"));
    }
    let wav = PathBuf::from(wav);
    let parent = wav
        .parent()
        .ok_or_else(|| error("missing input directory"))?
        .canonicalize()
        .map_err(error)?;
    // A symlink to elsewhere cannot redirect the fixture out of this profile.
    if parent != data.join("acceptance-input")
        || wav.file_name().is_none_or(|n| n != "microphone.wav")
    {
        return Err(error("input must belong to the isolated data directory"));
    }
    Ok(Some(parent.join("microphone.wav")))
}

fn read_samples(path: &Path) -> Result<Vec<f32>, RecorderError> {
    // canonicalize the file too: the final component may itself be a symlink.
    if path.canonicalize().map_err(error)? != path {
        return Err(error("input cannot redirect to another file"));
    }
    let meta = std::fs::metadata(path).map_err(error)?;
    if !meta.is_file() || meta.len() > MAX_WAV_BYTES {
        return Err(error("expected at most 60 seconds of 16 kHz mono PCM WAV"));
    }
    let wav = std::fs::read(path).map_err(error)?;
    let pcm = denzic_audio_v1_core::read_wav_pcm16le(&wav).map_err(error)?;
    if pcm.is_empty() || pcm.len() > TARGET_SAMPLE_RATE as usize * 2 * 60 {
        return Err(error("expected 0–60 seconds of audio"));
    }
    Ok(pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect())
}

pub(super) struct SyntheticCapture {
    stop_tx: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl SyntheticCapture {
    pub(super) fn start(
        path: PathBuf,
        sink: impl FnMut(&[f32], u32) + Send + 'static,
        runtime_errors: Sender<RecorderError>,
    ) -> Result<Self, RecorderError> {
        let samples = read_samples(&path)?;
        let count = samples.len();
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("isolated-synthetic-mic".into())
            .spawn(move || {
                // Keep the ordinary recorder error channel connected until capture ends.
                let _runtime_errors = runtime_errors;
                stream(&samples, sink, stop_rx);
            })
            .map_err(error)?;
        log::info!("[recorder] SYNTHETIC private input: {count} samples; not a live microphone");
        Ok(Self {
            stop_tx: Some(stop_tx),
            worker: Some(worker),
        })
    }

    pub(super) fn stop(mut self) -> Result<(), String> {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "synthetic input worker panicked".to_string())?;
        }
        Ok(())
    }
}

impl Drop for SyntheticCapture {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn stream(samples: &[f32], mut sink: impl FnMut(&[f32], u32), stop: Receiver<()>) {
    let silence = [0_f32; FRAME_SAMPLES];
    let mut offset = 0;
    let mut next = Instant::now();
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        match stop.recv_timeout(wait) {
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if offset < samples.len() {
            let end = (offset + FRAME_SAMPLES).min(samples.len());
            sink(&samples[offset..end], TARGET_SAMPLE_RATE);
            offset = end;
        } else {
            // Natural quiet after the input; normal UI still owns finishing.
            sink(&silence, TARGET_SAMPLE_RATE);
        }
        // Never rush accumulated samples into ASR after a scheduling delay.
        next = Instant::now() + FRAME_TIME;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_owner_desktop_even_with_private_looking_name() {
        assert!(validate_station("WinSta0", false).is_err());
        assert!(validate_station("ListenerNativeReplay_example", true).is_err());
        assert!(validate_station("ListenerNativeReplay_example", false).is_ok());
        // Tests run on the Owner station; this must not silently fall back to CPAL.
        assert!(private_station().is_err());
    }

    #[test]
    fn emits_input_then_silence_and_stops_without_a_microphone() {
        let (tx, rx) = mpsc::channel();
        let input = [0.25_f32; FRAME_SAMPLES];
        let mut frames = Vec::new();
        stream(
            &input,
            |frame, sr| {
                assert_eq!(sr, TARGET_SAMPLE_RATE);
                frames.push(frame.to_vec());
                if frames.len() == 2 {
                    tx.send(()).unwrap();
                }
            },
            rx,
        );
        assert_eq!(frames, vec![input.to_vec(), vec![0.0; FRAME_SAMPLES]]);
    }

    #[test]
    fn missing_fixture_does_not_start_capture() {
        let missing =
            std::env::temp_dir().join(format!("listener-missing-{}.wav", uuid::Uuid::new_v4()));
        assert!(read_samples(&missing).is_err());
    }
}
