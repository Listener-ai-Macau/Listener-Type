pub(crate) const PROVIDER_ID: &str = "companion-paraformer-large";

pub(crate) fn is_provider(id: &str) -> bool {
    id.trim().eq_ignore_ascii_case(PROVIDER_ID)
}

#[cfg(target_os = "windows")]
const SAMPLE_RATE: usize = 16_000;
#[cfg(target_os = "windows")]
pub(crate) const MAX_CHUNK_PCM_BYTES: usize = 20 * SAMPLE_RATE * 2;
#[cfg(target_os = "windows")]
pub(crate) const MAX_DICTATION_PCM_BYTES: usize = 60 * SAMPLE_RATE * 2;

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
mod windows {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;

    use super::{is_digital_silence, segment_end, MAX_DICTATION_PCM_BYTES};
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
            let root = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .ok_or("Companion Type model cache path is unavailable")?;
            let path = root.join("Companion Type/models/local-asr/funasr-paraformer-large-zh-2024");
            if !path.join("ready").is_file() {
                return Err("Companion Paraformer Large is missing from the local Companion Type model cache".into());
            }
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

    pub(crate) async fn transcribe(pcm: std::sync::Arc<[u8]>) -> Result<String, String> {
        if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > MAX_DICTATION_PCM_BYTES {
            return Err("invalid PCM for Companion Paraformer dictation".into());
        }
        tokio::task::spawn_blocking(move || {
            with_model(|model| {
                let started = Instant::now();
                let mut fragments = Vec::new();
                let mut remaining = pcm.as_ref();
                while !remaining.is_empty() {
                    let (segment, rest) = remaining.split_at(segment_end(remaining));
                    remaining = rest;
                    if is_digital_silence(segment) {
                        continue;
                    }
                    let text = model.decode_companion_dictation(segment)?;
                    if text.trim().is_empty() {
                        return Err("unrecognized local segment".into());
                    }
                    fragments.push(text.trim().to_string());
                }
                let text = fragments.join("\n");
                log::info!("[companion-paraformer] final decode audio_ms={} inference_ms={} segments={} chars={}",
                    pcm.len() / 32, started.elapsed().as_millis(), fragments.len(), text.chars().count());
                Ok(text)
            })
        }).await.map_err(|error| error.to_string())?
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
pub(crate) use windows::{prepare, release, release_if_idle, transcribe};

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
