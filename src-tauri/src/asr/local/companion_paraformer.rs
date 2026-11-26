pub(crate) const PROVIDER_ID: &str = "companion-paraformer-large";

pub(crate) fn is_provider(id: &str) -> bool {
    id.trim().eq_ignore_ascii_case(PROVIDER_ID)
}

pub(crate) fn canonical_provider(id: &str) -> &str {
    if cfg!(target_os = "windows")
        && (super::foundry::is_foundry_local_whisper(id) || super::is_local_qwen3(id))
    {
        PROVIDER_ID
    } else {
        id
    }
}

#[cfg(target_os = "windows")]
const SAMPLE_RATE: usize = 16_000;
#[cfg(target_os = "windows")]
pub(crate) const MAX_CHUNK_PCM_BYTES: usize = 20 * SAMPLE_RATE * 2;
#[cfg(target_os = "windows")]
pub(crate) const MAX_LIVE_PHRASE_PCM_BYTES: usize = 60 * SAMPLE_RATE * 2;

#[cfg(target_os = "windows")]
fn bundled_model_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("resources/models/companion-paraformer-large");
    ["model.int8.onnx", "tokens.txt"].iter().all(|name| dir.join(name).is_file()).then_some(dir)
}

pub(crate) fn bundled_available() -> bool {
    #[cfg(target_os = "windows")]
    return bundled_model_dir().is_some();
    #[cfg(not(target_os = "windows"))]
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn is_digital_silence(pcm: &[u8]) -> bool {
    !pcm.is_empty() && pcm.len() % 2 == 0 && pcm.iter().all(|&byte| byte == 0)
}

// Same quiet-boundary search used by Companion. Every source sample belongs
// to one segment. The final shorter segment is passed through unchanged.
#[cfg(target_os = "windows")]
fn segment_end(pcm: &[u8]) -> usize {
    if pcm.len() <= MAX_CHUNK_PCM_BYTES {
        return pcm.len();
    }
    let preferred = 16 * SAMPLE_RATE;
    let radius = SAMPLE_RATE * 12 / 100;
    let earliest = 14 * SAMPLE_RATE;
    let latest = 20 * SAMPLE_RATE - SAMPLE_RATE / 5;
    let step = SAMPLE_RATE / 5;
    (earliest..=latest)
        .step_by(step)
        .min_by_key(|&sample| {
            let energy: u64 = pcm[(sample - radius)*2..(sample + radius)*2]
                .chunks_exact(2)
                .map(|x| u64::from(i16::from_le_bytes([x[0], x[1]]).unsigned_abs()))
                .sum();
            let distance_penalty = (sample.abs_diff(preferred) as u64) / 20;
            energy * 5 + distance_penalty * 12
        })
        .unwrap_or(preferred) * 2
}

#[cfg(target_os = "windows")]
fn decode_segments(
    mut remaining: &[u8],
    mut decode: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<(String, usize, usize), String> {
    let mut fragments = Vec::new();
    let mut empty_segments = 0;
    while !remaining.is_empty() {
        let (segment, rest) = remaining.split_at(segment_end(remaining));
        remaining = rest;
        if is_digital_silence(segment) {
            continue;
        }
        let text = decode(segment)?;
        let text = text.trim();
        if text.is_empty() {
            empty_segments += 1;
        } else {
            fragments.push(text.to_owned());
        }
    }
    Ok((fragments.join("\n"), fragments.len(), empty_segments))
}

#[cfg(target_os = "windows")]
mod windows {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;

    use super::{bundled_model_dir, decode_segments, is_digital_silence};
    use crate::asr::local::paraformer::ParaformerRuntime;

    struct LocalModel {
        runtime: ParaformerRuntime,
        last_used: Instant,
    }

    impl LocalModel {
        fn is_idle(&self, limit: Duration) -> bool {
            self.last_used.elapsed() >= limit
        }
    }

    // Sherpa's recognizer and streams are used serially under MODEL with the
    // bundled CPU runtime. No stream or pointer leaves the blocking worker.
    unsafe impl Send for LocalModel {}

    static MODEL: Mutex<Option<LocalModel>> = Mutex::new(None);

    fn with_model<T>(run: impl FnOnce(&ParaformerRuntime) -> Result<T, String>) -> Result<T, String> {
        let mut slot = MODEL.lock();
        if slot.is_none() {
            let path = bundled_model_dir()
                .or_else(|| {
                    let path = PathBuf::from(std::env::var_os("APPDATA")?)
                        .join("Companion Type/models/local-asr/funasr-paraformer-large-zh-2024");
                    path.join("ready").is_file().then_some(path)
                })
                .ok_or("Companion Paraformer Large model files are missing. Install Listener Type with its local model, or prepare the same model in Companion Type.")?;
            let model = ParaformerRuntime::load_companion(&path)?;
            model.warm_up()?;
            *slot = Some(LocalModel {
                runtime: model,
                last_used: Instant::now(),
            });
            log::info!("[companion-paraformer] verified CPU model ready");
        }
        let model = slot.as_mut().expect("local model initialized");
        let result = run(&model.runtime);
        model.last_used = Instant::now();
        result
    }

    pub(crate) async fn prepare() -> Result<(), String> {
        tokio::task::spawn_blocking(|| with_model(|_| Ok(())))
            .await.map_err(|error| error.to_string())?
    }

    /// A preview only borrows an already warm and currently available recognizer.
    pub(crate) async fn preview(pcm: Vec<u8>) -> Result<Option<String>, String> {
        if pcm.is_empty() || is_digital_silence(&pcm) {
            return Ok(None);
        }
        tokio::task::spawn_blocking(move || {
            let Some(mut slot) = MODEL.try_lock() else { return Ok(None); };
            let Some(model) = slot.as_mut() else { return Ok(None); };
            let result = model.runtime.decode_companion_dictation(&pcm);
            model.last_used = Instant::now();
            result.map(|text| (!text.trim().is_empty()).then_some(text))
        })
        .await
        .map_err(|error| error.to_string())?
    }

    pub(crate) async fn transcribe(
        pcm: std::sync::Arc<[u8]>,
        timeout: Duration,
    ) -> Result<String, String> {
        if pcm.is_empty() || pcm.len() % 2 != 0 {
            return Err("invalid PCM for Companion Paraformer dictation".into());
        }
        let deadline = Instant::now() + timeout;
        let decode = tokio::task::spawn_blocking(move || {
            with_model(|model| {
                let started = Instant::now();
                let (text, segments, empty_segments) = decode_segments(&pcm, |segment| {
                    if Instant::now() >= deadline {
                        return Err("Companion Paraformer transcription timed out".into());
                    }
                    model.decode_companion_dictation(segment)
                })?;
                log::info!("[companion-paraformer] final decode audio_ms={} inference_ms={} segments={segments} empty_segments={empty_segments} chars={}",
                    pcm.len() / 32, started.elapsed().as_millis(), text.chars().count());
                Ok(text)
            })
        });
        tokio::time::timeout(timeout, decode)
            .await
            .map_err(|_| "Companion Paraformer transcription timed out".to_string())?
            .map_err(|error| error.to_string())?
    }

    pub(crate) async fn release() {
        let _ = tokio::task::spawn_blocking(|| drop(MODEL.lock().take())).await;
    }

    pub(crate) async fn release_if_idle(
        limit: Duration,
        can_release: impl FnOnce() -> bool + Send + 'static,
    ) -> bool {
        // Avoid dispatching work when the cache is empty, busy, or still warm.
        if !MODEL
            .try_lock()
            .is_some_and(|slot| slot.as_ref().is_some_and(|model| model.is_idle(limit)))
        {
            return false;
        }
        tokio::task::spawn_blocking(move || {
            let mut slot = MODEL.lock();
            if !slot.as_ref().is_some_and(|model| model.is_idle(limit)) || !can_release() {
                return false;
            }
            drop(slot.take());
            true
        })
        .await
        .unwrap_or(false)
    }
}

#[cfg(target_os = "windows")]
pub(crate) use windows::{prepare, preview, release, release_if_idle, transcribe};

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn companion_digital_silence_guard_does_not_discard_low_amplitude_voice() {
        let mut pcm = vec![0; SAMPLE_RATE * 2];
        assert!(is_digital_silence(&pcm));
        pcm[842] = 1;
        assert!(!is_digital_silence(&pcm));
        assert!(!is_digital_silence(&[]));
        assert!(!is_digital_silence(&[0]));
    }

    #[test]
    fn companion_keeps_recognized_speech_when_the_next_segment_is_blank() {
        let pcm = vec![1; 22 * SAMPLE_RATE * 2];
        let mut responses = ["这两句已经识别出来了", ""].into_iter();
        let (text, segments, empty_segments) = decode_segments(&pcm, |_| {
            Ok(responses.next().expect("two segments").to_owned())
        })
        .expect("blank recognition is a successful segment");
        assert_eq!(text, "这两句已经识别出来了");
        assert_eq!((segments, empty_segments), (1, 1));
        assert!(responses.next().is_none());
    }

    #[test]
    fn companion_long_capture_keeps_the_tail_and_bounds_each_chunk() {
        let mut pcm = Vec::new();
        for _ in 0..(45 * SAMPLE_RATE + 5) {
            pcm.extend_from_slice(&800_i16.to_le_bytes());
        }
        pcm[16 * SAMPLE_RATE * 2..16 * SAMPLE_RATE * 2 + SAMPLE_RATE].fill(0);
        let mut cursor = 0;
        let mut chunks = 0;
        while cursor < pcm.len() {
            let size = segment_end(&pcm[cursor..]);
            assert!(size > 0 && size <= MAX_CHUNK_PCM_BYTES && size % 2 == 0);
            cursor += size;
            chunks += 1;
        }
        assert_eq!(cursor, pcm.len());
        assert_eq!(chunks, 3);
    }
}
