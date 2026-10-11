//! Audio evidence for an unconfirmed ordinary-ASR suffix after body delivery.
//! This may resolve that exact suffix's endpoint obligation; it never deletes
//! text, grants owner identity, confirms recognition, or owns physical STOP.

use crate::speaker_verification::{self, NativeWordSpeakerProfile};
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingSourceKey {
    pub capture: u64,
    pub capture_generation: u64,
    pub lifecycle_generation: u64,
    pub profile: [u8; 32],
    pub phrase: String,
    pub key: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

struct Request {
    epoch: u64,
    key: PendingSourceKey,
    pcm: Vec<u8>,
    profile: NativeWordSpeakerProfile,
}

#[derive(Default)]
struct State {
    epoch: u64,
    running: bool,
    pending: Option<Request>,
    in_flight: Option<(u64, PendingSourceKey)>,
    completed: Option<(PendingSourceKey, bool)>,
}

#[derive(Default)]
pub(crate) struct PendingSourceIdentity {
    state: Arc<Mutex<State>>,
}

impl PendingSourceIdentity {
    pub(crate) fn reset(&self) {
        let mut state = self.state.lock();
        state.epoch = state.epoch.wrapping_add(1);
        state.pending = None;
        state.completed = None;
        // An old in-flight model job is bounded. Its epoch cannot publish into
        // the new capture; the same worker can then consume the newest request.
    }

    /// The watchdog only submits one bounded source job and reads a matching
    /// result. The cloud receive actor and live insertion never await this job.
    /// Newer revisions replace queued work, while absent evidence stays pending.
    pub(crate) fn confirmed_other(&self, key: PendingSourceKey, pcm: Vec<u8>,
        profile: NativeWordSpeakerProfile) -> bool {
        let mut state = self.state.lock();
        if let Some((cached, other)) = state.completed.as_ref().filter(|(cached, _)| cached == &key) {
            return *other && speaker_verification::native_word_binding_is_current(&cached.phrase, cached.profile);
        }
        let epoch = state.epoch;
        if state.in_flight.as_ref().is_some_and(|(old_epoch, old)| *old_epoch == epoch && old == &key)
            || state.pending.as_ref().is_some_and(|old| old.epoch == epoch && old.key == key) {
            return false;
        }
        state.pending = Some(Request { epoch, key, pcm, profile });
        if state.running { return false; }
        state.running = true;
        let shared = Arc::clone(&self.state);
        drop(state);
        tauri::async_runtime::spawn_blocking(move || loop {
            let request = {
                let mut state = shared.lock();
                let Some(request) = state.pending.take() else {
                    state.running = false;
                    state.in_flight = None;
                    break;
                };
                state.in_flight = Some((request.epoch, request.key.clone()));
                request
            };
            let began = std::time::Instant::now();
            let other = request.profile.observe(&request.pcm).is_ok_and(|observed|
                observed.endpoint_presence_identity() == crate::personal_vad::PresenceState::ConfirmedOther);
            let current = speaker_verification::native_word_binding_is_current(&request.key.phrase, request.key.profile);
            let mut state = shared.lock();
            if state.epoch == request.epoch && current {
                log::debug!("[pending-source-identity] capture={} generation={} range_ms={}..{} strong_other={} compute_ms={}",
                    request.key.capture, request.key.capture_generation, request.key.start_ms,
                    request.key.end_ms, other, began.elapsed().as_millis());
                state.completed = Some((request.key, other));
            }
            state.in_flight = None;
        });
        false
    }

    #[cfg(test)]
    pub(crate) fn install_saved_result_for_test(&self, key: PendingSourceKey, other: bool) {
        self.state.lock().completed = Some((key, other));
    }

    #[cfg(test)]
    pub(crate) fn is_busy_for_test(&self) -> bool { self.state.lock().running }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_cache_reset_retires_results_and_queued_work() {
        let identity = PendingSourceIdentity::default();
        let key = PendingSourceKey { capture:1, capture_generation:2, lifecycle_generation:3,
            profile:[0;32], phrase:"开始录音".into(), key:"尾段".into(), start_ms:4000, end_ms:6000 };
        identity.install_saved_result_for_test(key, true);
        let epoch = identity.state.lock().epoch;
        identity.reset();
        let state = identity.state.lock();
        assert_ne!(state.epoch, epoch);
        assert!(state.completed.is_none() && state.pending.is_none());
    }
}
