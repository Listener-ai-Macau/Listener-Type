//! Unified recording admission gate.
//!
//! All recording-related entry points (hotkey, device key, BLE start/PCM,
//! recording capsules, background listener refresh while dictation owns BLE)
//! must consult this gate instead of scattering `embedded_ble_ota_active`
//! checks. Policy lives here; callers only map a denial to logs/side-effects.
//!
//! Architecture:
//!   trigger → RecordIntent → RecordingGate::decide → Allow | Deny
//!
//! Current policy (v1): firmware OTA exclusive ownership blocks every
//! recording path and recording-related capsule states. Idle/Error/etc. still
//! allowed so cancel/suppress can force the UI dark.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::types::CapsuleState;

use super::Inner;

/// What the caller is asking to do through the recording pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordIntent {
    /// Desktop global hotkey press (start / toggle / hold begin).
    HotkeyPress,
    /// Device custom-key dictation start/stop control.
    DeviceKeyDictation,
    /// BLE `StreamingSessionEvent::Started` (device/voice wake start).
    BleSessionStart,
    /// BLE PCM ingest for an in-flight or candidate session.
    BlePcmIngest,
    /// Open a buffered speaker candidate or full embedded session.
    BleBeginCandidateOrSession,
    /// Emit a capsule state to the UI.
    ShowCapsule { state: CapsuleState },
    /// Background EmbeddedBle notify listener refresh / keep-alive.
    BackgroundListener,
}

/// Why admission was denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingDenyReason {
    /// Firmware OTA owns BLE exclusively; no dictation/wake/capsule recording UI.
    FirmwareOtaActive,
}

impl RecordingDenyReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FirmwareOtaActive => "firmware_ota_active",
        }
    }
}

/// Snapshot of state the gate needs (pure; easy to unit-test).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RecordingGateSnapshot {
    pub firmware_ota_active: bool,
}

impl RecordingGateSnapshot {
    pub(crate) fn from_inner(inner: &Inner) -> Self {
        Self {
            firmware_ota_active: inner.embedded_ble_ota_active.load(Ordering::SeqCst),
        }
    }

    pub(crate) fn from_inner_arc(inner: &Arc<Inner>) -> Self {
        Self::from_inner(inner)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingGateDecision {
    Allow,
    Deny(RecordingDenyReason),
}

impl RecordingGateDecision {
    pub(crate) fn is_allow(self) -> bool {
        matches!(self, Self::Allow)
    }

    pub(crate) fn is_deny(self) -> bool {
        !self.is_allow()
    }

    pub(crate) fn deny_reason(self) -> Option<RecordingDenyReason> {
        match self {
            Self::Allow => None,
            Self::Deny(reason) => Some(reason),
        }
    }
}

/// Capsule states that surface "we are recording / processing speech".
/// OTA must never show these; Idle / Reconnecting / Cancelled / Error stay usable
/// so suppress paths can clear the UI.
pub(crate) fn is_recording_related_capsule(state: CapsuleState) -> bool {
    matches!(
        state,
        CapsuleState::Recording
            | CapsuleState::Transcribing
            | CapsuleState::Polishing
            | CapsuleState::Done
    )
}

/// Pure policy. Add future rules (debounce, busy phase, etc.) here only.
pub(crate) fn decide(
    snapshot: RecordingGateSnapshot,
    intent: RecordIntent,
) -> RecordingGateDecision {
    if snapshot.firmware_ota_active {
        match intent {
            RecordIntent::ShowCapsule { state } if !is_recording_related_capsule(state) => {
                RecordingGateDecision::Allow
            }
            _ => RecordingGateDecision::Deny(RecordingDenyReason::FirmwareOtaActive),
        }
    } else {
        RecordingGateDecision::Allow
    }
}

/// Convenience: evaluate against live coordinator state.
pub(crate) fn admit(inner: &Inner, intent: RecordIntent) -> RecordingGateDecision {
    decide(RecordingGateSnapshot::from_inner(inner), intent)
}

pub(crate) fn admit_arc(inner: &Arc<Inner>, intent: RecordIntent) -> RecordingGateDecision {
    decide(RecordingGateSnapshot::from_inner_arc(inner), intent)
}

/// True when firmware OTA is holding exclusive BLE (same flag the gate reads).
pub(crate) fn firmware_ota_active(inner: &Inner) -> bool {
    inner.embedded_ble_ota_active.load(Ordering::SeqCst)
}

pub(crate) fn firmware_ota_active_arc(inner: &Arc<Inner>) -> bool {
    firmware_ota_active(inner)
}

/// Admit or log + return false. Call sites that only need a bool gate.
pub(crate) fn try_admit(inner: &Inner, intent: RecordIntent, log_context: &str) -> bool {
    match admit(inner, intent) {
        RecordingGateDecision::Allow => true,
        RecordingGateDecision::Deny(reason) => {
            log::info!(
                "[recording-gate] deny intent={intent:?} reason={} context={log_context}",
                reason.as_str()
            );
            false
        }
    }
}

pub(crate) fn try_admit_arc(inner: &Arc<Inner>, intent: RecordIntent, log_context: &str) -> bool {
    try_admit(inner, intent, log_context)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ota_on() -> RecordingGateSnapshot {
        RecordingGateSnapshot {
            firmware_ota_active: true,
        }
    }

    fn ota_off() -> RecordingGateSnapshot {
        RecordingGateSnapshot {
            firmware_ota_active: false,
        }
    }

    #[test]
    fn ota_blocks_all_start_intents() {
        for intent in [
            RecordIntent::HotkeyPress,
            RecordIntent::DeviceKeyDictation,
            RecordIntent::BleSessionStart,
            RecordIntent::BlePcmIngest,
            RecordIntent::BleBeginCandidateOrSession,
            RecordIntent::BackgroundListener,
        ] {
            assert!(
                decide(ota_on(), intent).is_deny(),
                "expected deny for {intent:?}"
            );
        }
    }

    #[test]
    fn ota_blocks_recording_related_capsules_only() {
        for state in [
            CapsuleState::Recording,
            CapsuleState::Transcribing,
            CapsuleState::Polishing,
            CapsuleState::Done,
        ] {
            assert!(decide(ota_on(), RecordIntent::ShowCapsule { state }).is_deny());
        }
        for state in [
            CapsuleState::Idle,
            CapsuleState::Reconnecting,
            CapsuleState::Cancelled,
            CapsuleState::Error,
        ] {
            assert!(
                decide(ota_on(), RecordIntent::ShowCapsule { state }).is_allow(),
                "Idle-class capsule must remain allow during OTA: {state:?}"
            );
        }
    }

    #[test]
    fn idle_allows_everything() {
        for intent in [
            RecordIntent::HotkeyPress,
            RecordIntent::DeviceKeyDictation,
            RecordIntent::BleSessionStart,
            RecordIntent::BlePcmIngest,
            RecordIntent::BleBeginCandidateOrSession,
            RecordIntent::BackgroundListener,
            RecordIntent::ShowCapsule {
                state: CapsuleState::Recording,
            },
        ] {
            assert!(decide(ota_off(), intent).is_allow());
        }
    }

    #[test]
    fn recording_related_capsule_classifier() {
        assert!(is_recording_related_capsule(CapsuleState::Recording));
        assert!(is_recording_related_capsule(CapsuleState::Transcribing));
        assert!(is_recording_related_capsule(CapsuleState::Polishing));
        assert!(is_recording_related_capsule(CapsuleState::Done));
        assert!(!is_recording_related_capsule(CapsuleState::Idle));
        assert!(!is_recording_related_capsule(CapsuleState::Error));
        assert!(!is_recording_related_capsule(CapsuleState::Cancelled));
        assert!(!is_recording_related_capsule(CapsuleState::Reconnecting));
    }

    #[test]
    fn deny_reason_string_is_stable() {
        assert_eq!(
            RecordingDenyReason::FirmwareOtaActive.as_str(),
            "firmware_ota_active"
        );
    }

    /// Source contract: recording entry points admit via RecordingGate, not raw OTA flag loads.
    #[test]
    fn call_sites_route_through_recording_gate() {
        let support = include_str!("support.rs");
        let session = include_str!("dictation_session.rs");
        let stream = include_str!("dictation_embedded_stream.rs");
        let device = include_str!("hotkey_device_runtime.rs");
        let ble = include_str!("embedded_ble_runtime.rs");

        assert!(
            support.contains("RecordIntent::ShowCapsule")
                && support.contains("recording_gate::try_admit"),
            "capsule emit must admit via RecordingGate"
        );
        assert!(
            session.contains("RecordIntent::HotkeyPress")
                && session.contains("recording_gate::try_admit"),
            "hotkey press must admit via RecordingGate"
        );
        assert!(
            device.contains("RecordIntent::DeviceKeyDictation")
                && device.contains("recording_gate::try_admit"),
            "device-key dictation must admit via RecordingGate"
        );
        assert!(
            stream.contains("RecordIntent::BleSessionStart")
                && stream.contains("RecordIntent::BlePcmIngest")
                && stream.contains("RecordIntent::BleBeginCandidateOrSession"),
            "embedded BLE start/pcm/begin must admit via RecordingGate"
        );
        assert!(
            ble.contains("RecordIntent::BackgroundListener") && ble.contains("recording_gate::"),
            "background listener must admit via RecordingGate"
        );

        for (name, src) in [
            ("support.rs", support),
            ("dictation_session.rs", session),
            ("dictation_embedded_stream.rs", stream),
            ("hotkey_device_runtime.rs", device),
        ] {
            assert!(
                !src.contains("embedded_ble_ota_active.load"),
                "{name} must not load OTA flag for admission; use RecordingGate"
            );
        }
    }
}
