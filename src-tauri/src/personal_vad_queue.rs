//! Source-coordinate mailbox for personal activity. Model work never holds it.
//! Capacity bounds PCM bytes, independently of transport packetization. A real
//! overrun retires unfinished analysis, not the recording or the whole worker.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};

const MAX_QUEUED_PCM_BYTES: usize = 60 * 16_000 * 2;

#[derive(Default)]
struct State {
    pcm: Vec<u8>,
    input_end_samples: Option<u64>,
    generation: u64,
    closed: bool,
}

pub(super) struct PresenceInputQueue {
    state: Mutex<State>,
    ready: Condvar,
}

pub(super) struct PresenceInputBatch {
    pub pcm: Vec<u8>,
    pub end_samples: u64,
    pub generation: u64,
}

pub(super) struct PresenceInputAdmission {
    pub generation: u64,
    pub rebased: bool,
}

/// Replaces the old channel's disconnected-consumer signal. Closing on unwind
/// prevents a dead worker from leaving an apparently healthy mailbox forever.
#[must_use]
pub(super) struct PresenceInputWorkerLifetime(Arc<PresenceInputQueue>);

impl Drop for PresenceInputWorkerLifetime {
    fn drop(&mut self) { self.0.close(); }
}

impl Default for PresenceInputQueue {
    fn default() -> Self {
        Self { state: Mutex::new(State { generation: 1, ..State::default() }), ready: Condvar::new() }
    }
}

impl PresenceInputQueue {
    pub fn worker_lifetime(self: &Arc<Self>) -> PresenceInputWorkerLifetime {
        PresenceInputWorkerLifetime(Arc::clone(self))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Invalidate evidence under the same generation lock as result publication.
    /// `new_origin_samples` begins fresh retained audio. The caller must keep the
    /// preceding unobserved interval Unknown, never infer silence/Other from it.
    pub fn submit(&self, pcm: &[u8], end_samples: u64,
        invalidate: impl FnOnce(u64)) -> Result<PresenceInputAdmission, String> {
        if pcm.len() % 2 != 0 { return Err("personal activity received incomplete PCM16".into()); }
        let mut state = self.lock();
        if state.closed { return Err("personal activity input is closed".into()); }
        if pcm.is_empty() {
            return Ok(PresenceInputAdmission { generation: state.generation, rebased: false });
        }
        let start = end_samples.checked_sub((pcm.len()/2) as u64)
            .ok_or("personal activity end precedes its PCM")?;
        if state.input_end_samples.is_some_and(|end| start < end) {
            return Err("personal activity source moved backwards or overlapped".into());
        }
        let discontinuous = state.input_end_samples.is_some_and(|end| start != end);
        let overrun = state.pcm.len().checked_add(pcm.len())
            .is_none_or(|bytes| bytes > MAX_QUEUED_PCM_BYTES);
        let rebased = discontinuous || overrun;
        // A single oversized input retains its newest bounded, sample-aligned
        // tail. This affects analysis only; ASR/archive PCM is owned elsewhere.
        let retained = if pcm.len() > MAX_QUEUED_PCM_BYTES {
            &pcm[pcm.len()-MAX_QUEUED_PCM_BYTES..]
        } else { pcm };
        if rebased {
            state.generation = state.generation.wrapping_add(1);
            state.pcm.clear();
            invalidate(end_samples - (retained.len()/2) as u64);
        }
        state.pcm.extend_from_slice(retained);
        state.input_end_samples = Some(end_samples);
        let admission = PresenceInputAdmission { generation: state.generation, rebased };
        drop(state);
        self.ready.notify_one();
        Ok(admission)
    }

    #[cfg(test)]
    pub fn take(&self, wait: bool) -> Option<PresenceInputBatch> {
        self.take_limit(wait, usize::MAX)
    }

    pub fn take_limit(&self, wait: bool, max_bytes: usize) -> Option<PresenceInputBatch> {
        let mut state = self.lock();
        while wait && state.pcm.is_empty() && !state.closed {
            state = self.ready.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if state.closed || state.pcm.is_empty() { return None; }
        let bytes = state.pcm.len().min(max_bytes) / 2 * 2;
        if bytes == 0 { return None; }
        let end_samples = state.input_end_samples?.checked_sub(((state.pcm.len()-bytes)/2) as u64)?;
        let pcm = state.pcm.drain(..bytes).collect();
        Some(PresenceInputBatch { pcm, end_samples, generation: state.generation })
    }

    /// All source sinks commit together relative to submit's invalidation.
    pub fn with_current<T>(&self, generation: u64, publish: impl FnOnce() -> T) -> Option<T> {
        let state = self.lock();
        (!state.closed && state.generation == generation).then(publish)
    }

    pub fn with_open<T>(&self, read: impl FnOnce() -> T) -> Option<T> {
        let state = self.lock();
        (!state.closed).then(read)
    }

    pub fn close(&self) {
        let mut state = self.lock();
        if state.closed { return; }
        state.closed = true;
        state.generation = state.generation.wrapping_add(1);
        state.pcm.clear();
        drop(state);
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires preserved real startup PCM; does not run a model"]
    fn saved_54_second_startup_retains_every_sample_across_packet_sizes() {
        let pcm = std::fs::read(std::env::var("LISTENER_PERSONAL_QUEUE_REPLAY_PCM").unwrap()).unwrap();
        assert_eq!(pcm.len(), 54_000 * 32);
        for packet_bytes in [3_200, 640, 2, pcm.len()] {
            let queue = PresenceInputQueue::default();
            let mut end = 0;
            for part in pcm.chunks(packet_bytes) {
                end += (part.len()/2) as u64;
                let admitted = queue.submit(part, end, |_| panic!("normal startup was retired")).unwrap();
                assert!(!admitted.rebased);
            }
            let batch = queue.take(false).unwrap();
            assert_eq!(batch.pcm, pcm);
            assert_eq!(batch.end_samples, end);
            assert_eq!(batch.generation, 1);
            assert!(queue.take(false).is_none());
            queue.submit(&pcm[..3_200], end+1_600, |_| panic!()).unwrap();
            assert_eq!(queue.take(false).unwrap().pcm, pcm[..3_200]);
        }
    }

    #[test]
    fn genuine_overrun_retires_inflight_results_and_recovers_fresh_pcm() {
        let queue = PresenceInputQueue::default();
        queue.submit(&[1; 3_200], 1_600, |_| panic!()).unwrap();
        let inflight = queue.take(false).unwrap();
        let first_end = 1_600 + (MAX_QUEUED_PCM_BYTES/2) as u64;
        queue.submit(&vec![2; MAX_QUEUED_PCM_BYTES], first_end, |_| panic!()).unwrap();
        let mut gap_end = None;
        let admitted = queue.submit(&[3; 3_200], first_end+1_600, |origin| gap_end = Some(origin)).unwrap();
        assert!(admitted.rebased);
        assert_eq!(gap_end, Some(first_end));
        for obsolete_label in ["Owner", "Other", "Error"] {
            assert!(queue.with_current(inflight.generation, || obsolete_label).is_none());
        }
        let fresh = queue.take(false).unwrap();
        assert_eq!(fresh.pcm, [3; 3_200]);
        assert_eq!(fresh.end_samples, first_end+1_600);
        assert_eq!(queue.with_current(fresh.generation, || "fresh"), Some("fresh"));
        queue.submit(&[4; 3_200], first_end+3_200, |_| panic!()).unwrap();
        assert_eq!(queue.take(false).unwrap().pcm, [4; 3_200]);
    }

    #[test]
    fn forward_gap_is_explicit_and_invalid_ranges_do_not_gain_a_new_identity() {
        let queue = PresenceInputQueue::default();
        queue.submit(&[1; 32], 16, |_| panic!()).unwrap();
        let mut gap_end = None;
        let admitted = queue.submit(&[2; 32], 160, |start| gap_end = Some(start)).unwrap();
        assert!(admitted.rebased);
        assert_eq!(gap_end, Some(144));
        assert_eq!(queue.take(false).unwrap().pcm, [2; 32]);
        assert!(queue.submit(&[0; 3], 163, |_| panic!()).is_err());
        assert!(queue.submit(&[0; 32], 159, |_| panic!()).is_err());
        assert!(queue.take(false).is_none());
    }

    #[test]
    fn closing_wakes_the_consumer_and_prevents_late_publication() {
        let queue = std::sync::Arc::new(PresenceInputQueue::default());
        let reader = std::sync::Arc::clone(&queue);
        let worker = std::thread::spawn(move || reader.take(true));
        queue.close();
        assert!(worker.join().unwrap().is_none());
        assert!(queue.with_current(1, || true).is_none());
        assert!(queue.with_open(|| true).is_none());
        assert!(queue.submit(&[0; 2], 1, |_| panic!()).is_err());
    }

    #[test]
    fn consumer_unwind_closes_input_and_rejects_stale_results() {
        let queue = Arc::new(PresenceInputQueue::default());
        let lifetime = queue.worker_lifetime();
        let writer = Arc::clone(&queue);
        let worker = std::thread::spawn(move || {
            let _lifetime = lifetime;
            writer.with_current(1, || panic!("controlled publication failure"));
        });
        assert!(worker.join().is_err());
        assert!(queue.with_open(|| true).is_none());
        assert!(queue.with_current(1, || "stale owner").is_none());
        assert!(queue.submit(&[0; 2], 1, |_| panic!()).is_err());
    }
}
