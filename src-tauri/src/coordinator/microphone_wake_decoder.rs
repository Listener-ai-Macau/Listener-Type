//! KWS stream clocks stay aligned with the bounded native microphone buffer.
use super::microphone_wake_buffer::{WakeBuffer, PCM_BYTES_PER_SECOND};
use crate::wake_phrase::{Match, StreamingDetector};
use parking_lot::Mutex;

pub(super) fn confirm_candidate(pcm: &[u8], phrase: &str) -> Result<bool, String> {
    let boosted = crate::wake_phrase::gain_normalized_pcm16(pcm);
    crate::asr::local::wake_helper::confirm_pcm_if_ready(
        &boosted,
        phrase,
        std::time::Duration::from_secs(4),
    )
    .map(|result| result.matched)
}

pub(super) struct Chunk {
    pub start: u64,
    pub end: u64,
    pub pcm: Vec<u8>,
}

pub(super) struct Decoder {
    detector: StreamingDetector,
    phrase: String,
    origin: u64,
    accepted_end: u64,
    unproven_retries: u8,
}

impl Decoder {
    pub fn new(detector: StreamingDetector, phrase: String) -> Self {
        Self {
            detector,
            phrase,
            origin: 0,
            accepted_end: 0,
            unproven_retries: 0,
        }
    }

    pub fn accept(
        &mut self,
        chunk: Chunk,
        buffer: &Mutex<WakeBuffer>,
    ) -> Result<Option<Match>, String> {
        let pcm = if chunk.start == self.accepted_end {
            chunk.pcm
        } else {
            let recovered = buffer.lock().range(self.accepted_end, chunk.end);
            match recovered {
                Ok(pcm) => pcm,
                Err(_) => {
                    self.detector = StreamingDetector::new_strict(&self.phrase)?;
                    self.origin = chunk.start;
                    chunk.pcm
                }
            }
        };
        self.accepted_end = chunk.end;
        let mut found = self.detector.accept_pcm(&pcm)?;
        if found.is_none()
            && self.accepted_end.saturating_sub(self.origin) > 30 * PCM_BYTES_PER_SECOND
        {
            let start = self.accepted_end.saturating_sub(8 * PCM_BYTES_PER_SECOND);
            // Snapshot before model work: inference never holds up native capture.
            let overlap = buffer.lock().range(start, self.accepted_end)?;
            let mut next = StreamingDetector::new_strict(&self.phrase)?;
            found = next.accept_pcm(&overlap)?;
            self.detector = next;
            self.origin = start;
        }
        if found
            .as_ref()
            .is_some_and(|result| result.audio_end_seconds.is_none())
        {
            #[cfg(test)]
            eprintln!(
                "synthetic boundary recovery accepted_s={:.3} origin_s={:.3} attempt={}",
                self.accepted_end as f64 / 32_000.0,
                self.origin as f64 / 32_000.0,
                self.unproven_retries
            );
            // Long silence can make the native KWS timestamp segment-relative.
            // Re-decode immutable recent source windows to obtain a new proven
            // boundary. Never turn the observation clock into an audio proof.
            for seconds in [3, 6, 9] {
                let start = self
                    .accepted_end
                    .saturating_sub(seconds * PCM_BYTES_PER_SECOND)
                    .max(buffer.lock().start);
                let recent = buffer.lock().range(start, self.accepted_end)?;
                let mut fresh = StreamingDetector::new_strict(&self.phrase)?;
                let mut recovered = None;
                for pcm in recent.chunks(5_120) {
                    if let Some(result) = fresh.accept_pcm(pcm)? {
                        recovered = Some(result);
                        break;
                    }
                }
                if let Some(result) = recovered.filter(|result| {
                    result.audio_end_seconds.is_some_and(|end| {
                        let absolute = start + (end as f64 * 16_000.0).round() as u64 * 2;
                        absolute <= self.accepted_end
                            && self.accepted_end - absolute <= PCM_BYTES_PER_SECOND
                    })
                }) {
                    self.origin = start;
                    found = Some(result);
                    break;
                }
            }
            if found
                .as_ref()
                .is_some_and(|result| result.audio_end_seconds.is_none())
            {
                // An early native label may precede the last spoken syllable.
                // Keep receiving source PCM briefly instead of freezing or
                // ending the capture. Bound recovery work for ordinary speech.
                self.unproven_retries += 1;
                if self.unproven_retries >= 8 {
                    self.detector = StreamingDetector::new_strict(&self.phrase)?;
                    self.origin = self.accepted_end;
                    self.unproven_retries = 0;
                }
                return Ok(None);
            }
        }
        Ok(found.map(|mut result| {
            let offset = self.origin as f32 / PCM_BYTES_PER_SECOND as f32;
            result.start_seconds = result.start_seconds.map(|time| time + offset);
            result.end_seconds += offset;
            result.audio_end_seconds = result.audio_end_seconds.map(|time| time + offset);
            result
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires explicit synthetic fixture directory; uses local KWS, no microphone or cloud"]
    fn microphone_wake_native_decoder_preserves_clock_after_long_idle_and_queue_gap() {
        use std::time::Instant;
        let root = std::path::PathBuf::from(
            std::env::var("LISTENER_WAKE_DIAG_DIR").expect("synthetic fixture directory"),
        );
        let start = Instant::now();
        crate::asr::local::wake_helper::preload().unwrap();
        for prefix_seconds in [0, 34, 95] {
            for name in [
                "wake_huihui",
                "neg_kaishi_luxiang",
                "neg_kaishi_luru",
                "neg_kaishi_luyan",
            ] {
                let mut audio = vec![0; prefix_seconds * PCM_BYTES_PER_SECOND as usize];
                let wav = std::fs::read(root.join(format!("{name}.wav"))).unwrap();
                let offset = wav.windows(4).position(|window| window == b"data").unwrap() + 8;
                let length =
                    u32::from_le_bytes(wav[offset - 4..offset].try_into().unwrap()) as usize;
                audio.extend_from_slice(&wav[offset..offset + length]);
                audio.extend(vec![0; PCM_BYTES_PER_SECOND as usize]);
                let detector = StreamingDetector::new_strict("开始录音").unwrap();
                let mut decoder = Decoder::new(detector, "开始录音".into());
                let buffer = Mutex::new(WakeBuffer::default());
                let mut found = None;
                for (index, pcm) in audio.chunks(5_120).enumerate() {
                    let (begin, end) = buffer.lock().append(pcm);
                    if index % 7 == 2 {
                        continue;
                    } // Simulate bounded queue overflow.
                    if let Some(result) = decoder
                        .accept(
                            Chunk {
                                start: begin,
                                end,
                                pcm: pcm.to_vec(),
                            },
                            &buffer,
                        )
                        .unwrap()
                    {
                        found = Some(result);
                        break;
                    }
                }
                eprintln!(
                    "synthetic case={name} idle_s={prefix_seconds} matched={} proven_end={:?}",
                    found.is_some(),
                    found.as_ref().and_then(|result| result.audio_end_seconds)
                );
                let confirmed = if let Some(seconds) =
                    found.as_ref().and_then(|result| result.audio_end_seconds)
                {
                    let keyword_end = (seconds as f64 * 16_000.0).round() as u64 * 2;
                    let begin = keyword_end
                        .saturating_sub(4 * PCM_BYTES_PER_SECOND)
                        .max(buffer.lock().start);
                    let pcm = {
                        let buffer = buffer.lock();
                        buffer.range(begin, buffer.end).unwrap()
                    };
                    confirm_candidate(&pcm, "开始录音").unwrap()
                } else {
                    false
                };
                if name == "wake_huihui" {
                    assert!(
                        confirmed,
                        "full phrase needs independent local confirmation"
                    );
                    let found = found.expect("strict full wake phrase");
                    assert_eq!(found.matched_keyword.as_deref(), Some("开始录音"));
                    let end = found.audio_end_seconds.expect("proven audio boundary");
                    assert!(end > prefix_seconds as f32 && end < prefix_seconds as f32 + 4.0);
                    let boundary = (end as f64 * 16_000.0).round() as u64 * 2;
                    let tail = buffer.lock().body_after(boundary).unwrap();
                    let accepted = buffer.lock().end as usize;
                    assert_eq!(tail, audio[boundary as usize..accepted]);
                } else {
                    assert!(!confirmed, "near phrase {name} must not start dictation");
                }
            }
        }
        eprintln!(
            "synthetic microphone decoder elapsed_ms={}",
            start.elapsed().as_millis()
        );
    }
}
