use std::{
    env, fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::Instant,
};

use chrono::Utc;
use serde::Serialize;

const MACHINE_EVIDENCE_PATH_ENV: &str = "LISTENER_TYPE_MACHINE_EVIDENCE_PATH";

static MACHINE_EVIDENCE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
static MACHINE_EVIDENCE_STATE: OnceLock<Mutex<Option<MachineEvidenceState>>> = OnceLock::new();

#[derive(Debug)]
struct MachineEvidenceState {
    started_at: String,
    started_at_monotonic: Instant,
    startup_path: Option<&'static str>,
    background_notify_ready_elapsed_ms: Option<u64>,
    background_notify_ready_count: u32,
    pair_async_attempt_count: u32,
    unpair_async_attempt_count: u32,
}

#[derive(Debug, Serialize)]
struct MachineEvidenceSnapshot<'a> {
    schema: &'static str,
    started_at: &'a str,
    startup_path: Option<&'a str>,
    background_notify_ready_elapsed_ms: Option<u64>,
    background_notify_ready_count: u32,
    pair_async_attempt_count: u32,
    unpair_async_attempt_count: u32,
}

fn machine_evidence_path() -> Option<&'static PathBuf> {
    MACHINE_EVIDENCE_PATH
        .get_or_init(|| env::var_os(MACHINE_EVIDENCE_PATH_ENV).map(PathBuf::from))
        .as_ref()
}

fn machine_evidence_state() -> &'static Mutex<Option<MachineEvidenceState>> {
    MACHINE_EVIDENCE_STATE.get_or_init(|| Mutex::new(None))
}

impl MachineEvidenceState {
    fn new() -> Self {
        Self {
            started_at: Utc::now().to_rfc3339(),
            started_at_monotonic: Instant::now(),
            startup_path: None,
            background_notify_ready_elapsed_ms: None,
            background_notify_ready_count: 0,
            pair_async_attempt_count: 0,
            unpair_async_attempt_count: 0,
        }
    }

    fn snapshot(&self) -> MachineEvidenceSnapshot<'_> {
        MachineEvidenceSnapshot {
            schema: "listener.type.startup-evidence.v1",
            started_at: &self.started_at,
            startup_path: self.startup_path,
            background_notify_ready_elapsed_ms: self.background_notify_ready_elapsed_ms,
            background_notify_ready_count: self.background_notify_ready_count,
            pair_async_attempt_count: self.pair_async_attempt_count,
            unpair_async_attempt_count: self.unpair_async_attempt_count,
        }
    }
}

fn persist(state: &MachineEvidenceState) {
    let Some(path) = machine_evidence_path() else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec_pretty(&state.snapshot()) else {
        return;
    };
    let _ = fs::write(path, bytes);
}

fn update(mutator: impl FnOnce(&mut MachineEvidenceState)) {
    if machine_evidence_path().is_none() {
        return;
    }
    let Ok(mut state) = machine_evidence_state().lock() else {
        return;
    };
    let Some(state) = state.as_mut() else {
        return;
    };
    mutator(state);
    persist(state);
}

/// Starts an opt-in, privacy-safe startup evidence snapshot for an installed-Type test.
/// Normal product launches do not set the environment variable and never write this file.
pub(crate) fn begin_process_evidence() {
    if machine_evidence_path().is_none() {
        return;
    }
    let Ok(mut state) = machine_evidence_state().lock() else {
        return;
    };
    *state = Some(MachineEvidenceState::new());
    if let Some(state) = state.as_ref() {
        persist(state);
    }
}

pub(crate) fn record_startup_path(path: &'static str) {
    update(|state| state.startup_path = Some(path));
}

pub(crate) fn record_pair_async_attempt() {
    update(|state| {
        state.pair_async_attempt_count = state.pair_async_attempt_count.saturating_add(1)
    });
}

pub(crate) fn record_unpair_async_attempt() {
    update(|state| {
        state.unpair_async_attempt_count = state.unpair_async_attempt_count.saturating_add(1)
    });
}

pub(crate) fn record_background_notify_ready() {
    update(|state| {
        state.background_notify_ready_count = state.background_notify_ready_count.saturating_add(1);
        if state.background_notify_ready_elapsed_ms.is_none() {
            state.background_notify_ready_elapsed_ms = Some(
                state
                    .started_at_monotonic
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::MachineEvidenceState;

    #[test]
    fn startup_snapshot_contains_only_machine_timing_and_counts() {
        let mut state = MachineEvidenceState::new();
        state.startup_path = Some("native_windows_hid");
        state.background_notify_ready_elapsed_ms = Some(812);
        state.background_notify_ready_count = 1;
        let value = serde_json::to_value(state.snapshot()).expect("snapshot should serialize");

        assert_eq!(value["schema"], "listener.type.startup-evidence.v1");
        assert_eq!(value["startup_path"], "native_windows_hid");
        assert_eq!(value["background_notify_ready_elapsed_ms"], 812);
        assert_eq!(value["pair_async_attempt_count"], 0);
        assert_eq!(value["unpair_async_attempt_count"], 0);
        assert!(value.get("address").is_none());
        assert!(value.get("message").is_none());
        assert!(value.get("transcript").is_none());
    }

    #[test]
    fn installed_takeover_evidence_stays_wired_to_real_ble_transition_points() {
        let lib = include_str!("lib.rs");
        let embedded_ble = include_str!("embedded_ble.rs");
        let coordinator = include_str!("coordinator.rs");

        assert!(lib.contains("startup_evidence::begin_process_evidence()"));
        assert!(embedded_ble.contains("record_startup_path(\"native_windows_hid\")"));
        assert!(embedded_ble.contains("record_startup_path(\"persisted_cached\")"));
        assert_eq!(
            embedded_ble.matches("record_pair_async_attempt()").count(),
            2,
            "both standard and custom Windows PairAsync calls must remain observable",
        );
        assert_eq!(
            embedded_ble
                .matches("record_unpair_async_attempt()")
                .count(),
            1,
            "the shared Windows UnpairAsync helper must remain observable",
        );
        assert!(coordinator.contains("record_background_notify_ready()"));
    }
}
