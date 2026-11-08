//! Bounded source-clock PCM retained only in memory for a microphone handoff.
use std::collections::VecDeque;

pub(super) const PCM_BYTES_PER_SECOND: u64 = 32_000;
pub(super) const RETAINED_BYTES: usize = 12 * 32_000;

#[derive(Default)]
pub(super) struct WakeBuffer {
    pcm: VecDeque<u8>,
    pub start: u64,
    pub end: u64,
    pub detected: bool,
}

impl WakeBuffer {
    pub fn append(&mut self, pcm: &[u8]) -> (u64, u64) {
        let start = self.end;
        self.end += pcm.len() as u64;
        self.pcm.extend(pcm);
        let discard = self.pcm.len().saturating_sub(RETAINED_BYTES);
        self.pcm.drain(..discard);
        self.start += discard as u64;
        (start, self.end)
    }

    pub fn range(&self, start: u64, end: u64) -> Result<Vec<u8>, String> {
        if start % 2 != 0 || end % 2 != 0 || start < self.start || end > self.end || start > end {
            return Err("microphone wake source range expired".into());
        }
        Ok(self
            .pcm
            .iter()
            .skip((start - self.start) as usize)
            .take((end - start) as usize)
            .copied()
            .collect())
    }

    pub fn body_after(&self, keyword_end: u64) -> Result<Vec<u8>, String> {
        self.range(keyword_end, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_handoff_retains_only_post_keyword_pcm_in_source_order() {
        let mut buffer = WakeBuffer::default();
        buffer.append(&[1, 0, 2, 0, 3, 0]);
        let keyword_end = buffer.end;
        buffer.append(&[4, 0, 5, 0]);
        buffer.append(&[6, 0]);
        assert_eq!(buffer.body_after(keyword_end).unwrap(), [4, 0, 5, 0, 6, 0]);
        assert!(buffer.body_after(keyword_end + 1).is_err());
        assert!(buffer.body_after(buffer.end + 2).is_err());
    }

    #[test]
    fn wake_handoff_rejects_expired_audio_instead_of_silently_losing_body() {
        let mut buffer = WakeBuffer::default();
        buffer.append(&vec![1; RETAINED_BYTES]);
        buffer.append(&[2, 0, 3, 0]);
        assert!(buffer.body_after(0).is_err());
        assert_eq!(buffer.end - buffer.start, RETAINED_BYTES as u64);
        assert_eq!(
            buffer.range(buffer.end - 4, buffer.end).unwrap(),
            [2, 0, 3, 0]
        );
    }
}
