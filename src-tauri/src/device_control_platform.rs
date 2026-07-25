//! Listener's thin BLE adapter for the shared device-control contract.
//!
//! Existing recovery evidence decides whether Type may attempt PairAsync. This
//! module does not discover devices or call Windows APIs; it makes that one
//! authorized attempt an identifiable, bounded, terminal transaction.

use denzic_device_control_v1_core::{
    DeviceControlCore, ErrorCategory, LifecycleState, OperationDecision, OperationKind,
    OperationRequest, OperationResult, OwnershipState, Transport,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const LISTENER_BACKGROUND_PAIRING_TARGET: u32 = 1;
const LISTENER_BACKGROUND_PAIRING_TIMEOUT_MS: u32 = 6_000;
static NEXT_BACKGROUND_PAIRING_OPERATION_ID: AtomicU64 = AtomicU64::new(1);
const LISTENER_DEVICE_SETTINGS_TARGET: u32 = 1;
const LISTENER_DEVICE_SETTINGS_TIMEOUT_MS: u32 = 2_000;
const LISTENER_DEVICE_CONTROL_CAPABILITY_REVISION: u32 = 1;
const LISTENER_CAPABILITY_BLE_AUDIO_VKA1: u64 = 1 << 0;
const LISTENER_CAPABILITY_BLE_AUDIO_CONTROL_V1: u64 = 1 << 1;
const LISTENER_CAPABILITY_VOICE_RECORD_TOGGLE: u64 = 1 << 2;
const LISTENER_CAPABILITY_OTA_V1: u64 = 1 << 3;
const LISTENER_CAPABILITY_DEVICE_CONTROL_V1: u64 = 1 << 4;
static NEXT_DEVICE_CONTROL_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct BackgroundPairingRecovery {
    core: DeviceControlCore,
    request: OperationRequest,
    decision: OperationDecision,
}

impl BackgroundPairingRecovery {
    pub(crate) fn begin(
        type_controlled_recovery: bool,
        manual_unpair_hold: bool,
        automatic_cleanup_allowed: bool,
    ) -> Self {
        let mut core = DeviceControlCore::new(Transport::Ble);
        core.set_lifecycle(LifecycleState::Recovering);
        if manual_unpair_hold {
            core.set_ownership(OwnershipState::ManualUnpaired);
        } else if type_controlled_recovery {
            core.set_ownership(OwnershipState::Local);
        }
        let operation_id = NEXT_BACKGROUND_PAIRING_OPERATION_ID.fetch_add(1, Ordering::Relaxed);
        let operation_id = if operation_id == 0 {
            NEXT_BACKGROUND_PAIRING_OPERATION_ID.fetch_add(1, Ordering::Relaxed)
        } else {
            operation_id
        };
        let request = OperationRequest {
            // The coordinator guard permits one recovery at a time, and this
            // process-wide sequence keeps each guarded attempt traceable.
            operation_id,
            idempotency_key: operation_id,
            target_id: LISTENER_BACKGROUND_PAIRING_TARGET,
            expected_settings_revision: 0,
            timeout_ms: LISTENER_BACKGROUND_PAIRING_TIMEOUT_MS,
            kind: OperationKind::Connect,
            automatic: true,
            recovery_authorized: automatic_cleanup_allowed,
        };
        let decision = core.begin(request, 0);
        Self {
            core,
            request,
            decision,
        }
    }

    pub(crate) fn may_execute(&self) -> bool {
        self.decision.execute
    }

    pub(crate) fn decision(&self) -> OperationDecision {
        self.decision
    }

    pub(crate) fn complete_pairing(&mut self) -> OperationDecision {
        self.core.complete(
            self.request.operation_id,
            self.request.idempotency_key,
            OperationResult::Succeeded,
            ErrorCategory::None,
        )
    }

    pub(crate) fn fail_without_reclaim(&mut self, error: ErrorCategory) -> OperationDecision {
        self.core.complete(
            self.request.operation_id,
            self.request.idempotency_key,
            OperationResult::Failed,
            error,
        )
    }
}

/// A bounded, BLE-only settings transaction over the shared device-control core.
///
/// The firmware owns the monotonic revision. Type never invents a local version:
/// each write receives the GATT response first and then proves the persisted
/// revision through the dedicated read-only characteristic.
pub(crate) struct BleDeviceSettingsTransaction {
    core: DeviceControlCore,
    capability_mask: u64,
    started_at: Instant,
}

impl BleDeviceSettingsTransaction {
    pub(crate) fn begin(timeout: Duration) -> Result<Self, String> {
        let status = crate::embedded_ble::read_embedded_audio_status(timeout).map_err(|err| {
            let category = classify_ble_error_category(&err);
            format!("Listener BLE settings control unavailable category={category:?}: {err}")
        })?;
        if !status.connected {
            return Err(format!(
                "Listener BLE settings control unavailable category={:?}: {}",
                ErrorCategory::Transport,
                status
                    .detail
                    .unwrap_or_else(|| "fresh GATT status did not confirm a live link".to_string())
            ));
        }
        let capabilities = status.capabilities.as_deref().ok_or_else(|| {
            format!(
                "Listener BLE settings control unavailable category={:?}: missing capability readback",
                ErrorCategory::Protocol
            )
        })?;

        let mut transaction = Self::from_capabilities(capabilities)?;
        transaction.report_capabilities()?;
        transaction.read_settings_revision(timeout)?;
        Ok(transaction)
    }

    fn from_capabilities(capabilities: &str) -> Result<Self, String> {
        let capability_mask = listener_capability_mask(capabilities);
        if capability_mask & LISTENER_CAPABILITY_DEVICE_CONTROL_V1 == 0 {
            return Err(format!(
                "Listener BLE settings control unavailable category={:?}: firmware does not advertise device_control_v1",
                ErrorCategory::Protocol
            ));
        }

        let mut core = DeviceControlCore::new(Transport::Ble);
        core.set_ownership(OwnershipState::Local);
        core.set_lifecycle(LifecycleState::Ready);
        Ok(Self {
            core,
            capability_mask,
            started_at: Instant::now(),
        })
    }

    pub(crate) fn write_setting(
        &mut self,
        id: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        let request = self.begin_operation(OperationKind::WriteSetting)?;
        let command = format!("{command} rev={}", self.core.settings_revision);
        if let Err(err) = crate::embedded_ble::send_device_settings_command(&command, timeout) {
            return Err(self.fail_operation(request, &err));
        }
        let acknowledged_revision = self.core.settings_revision;
        if !self.core.acknowledge_setting_write(
            request.operation_id,
            request.idempotency_key,
            acknowledged_revision,
        ) {
            return Err(self.fail_protocol_operation(
                request,
                format!("{id} write acknowledgement was rejected by DeviceControlCore"),
            ));
        }

        let observed_revision = match crate::embedded_ble::read_device_settings_revision(timeout) {
            Ok(revision) => revision,
            Err(err) => return Err(self.fail_operation(request, &err)),
        };
        if self.core.expire(self.elapsed_ms()) {
            return Err(format!(
                "Listener BLE settings write timed out id={id} category={:?}",
                ErrorCategory::Timeout
            ));
        }
        if !self.core.confirm_setting_readback(
            request.operation_id,
            request.idempotency_key,
            observed_revision,
        ) {
            return Err(self.fail_protocol_operation(
                request,
                format!(
                    "{id} write readback revision={observed_revision} did not confirm acknowledged revision={acknowledged_revision}"
                ),
            ));
        }
        log::info!(
            "[device-control] settings write succeeded id={id} operation_id={} revision={observed_revision}",
            request.operation_id
        );
        Ok(())
    }

    pub(crate) fn invoke_command<F>(&mut self, id: &str, operation: F) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        let request = self.begin_operation(OperationKind::InvokeCommand)?;
        if let Err(err) = operation() {
            return Err(self.fail_operation(request, &err));
        }
        if self.core.expire(self.elapsed_ms()) {
            return Err(format!(
                "Listener BLE command timed out id={id} category={:?}",
                ErrorCategory::Timeout
            ));
        }
        let terminal = self.core.complete(
            request.operation_id,
            request.idempotency_key,
            OperationResult::Succeeded,
            ErrorCategory::None,
        );
        if terminal.result != OperationResult::Succeeded {
            return Err(format!(
                "Listener BLE command completion rejected id={id} category={:?}",
                terminal.error
            ));
        }
        log::info!(
            "[device-control] command succeeded id={id} operation_id={}",
            request.operation_id
        );
        Ok(())
    }

    fn report_capabilities(&mut self) -> Result<(), String> {
        let request = self.begin_operation(OperationKind::ReadCapabilities)?;
        if !self.core.report_capabilities(
            request.operation_id,
            request.idempotency_key,
            LISTENER_DEVICE_CONTROL_CAPABILITY_REVISION,
            self.capability_mask,
        ) {
            return Err(self.fail_protocol_operation(
                request,
                "capability readback was rejected by DeviceControlCore".to_string(),
            ));
        }
        Ok(())
    }

    fn read_settings_revision(&mut self, timeout: Duration) -> Result<u32, String> {
        let request = self.begin_operation(OperationKind::ReadSetting)?;
        let observed_revision = match crate::embedded_ble::read_device_settings_revision(timeout) {
            Ok(revision) => revision,
            Err(err) => return Err(self.fail_operation(request, &err)),
        };
        if self.core.expire(self.elapsed_ms()) {
            return Err(format!(
                "Listener BLE settings read timed out category={:?}",
                ErrorCategory::Timeout
            ));
        }
        if !self.core.confirm_setting_readback(
            request.operation_id,
            request.idempotency_key,
            observed_revision,
        ) {
            return Err(self.fail_protocol_operation(
                request,
                format!("settings readback revision={observed_revision} was rejected by DeviceControlCore"),
            ));
        }
        log::info!(
            "[device-control] settings read succeeded operation_id={} revision={observed_revision}",
            request.operation_id
        );
        Ok(observed_revision)
    }

    fn begin_operation(&mut self, kind: OperationKind) -> Result<OperationRequest, String> {
        let operation_id = next_operation_id(&NEXT_DEVICE_CONTROL_OPERATION_ID);
        let request = OperationRequest {
            operation_id,
            idempotency_key: operation_id,
            target_id: LISTENER_DEVICE_SETTINGS_TARGET,
            expected_settings_revision: self.core.settings_revision,
            timeout_ms: LISTENER_DEVICE_SETTINGS_TIMEOUT_MS,
            kind,
            automatic: false,
            recovery_authorized: false,
        };
        let decision = self.core.begin(request, self.elapsed_ms());
        if !decision.execute {
            return Err(format!(
                "Listener BLE {kind:?} rejected category={:?} result={:?}",
                decision.error, decision.result
            ));
        }
        Ok(request)
    }

    fn fail_operation(&mut self, request: OperationRequest, error: &str) -> String {
        if self.core.expire(self.elapsed_ms()) {
            return format!(
                "Listener BLE {:?} timed out category={:?}: {error}",
                request.kind,
                ErrorCategory::Timeout
            );
        }
        let category = classify_ble_error_category(error);
        let terminal = self.core.complete(
            request.operation_id,
            request.idempotency_key,
            OperationResult::Failed,
            category,
        );
        format!(
            "Listener BLE {:?} failed category={:?} result={:?}: {error}",
            request.kind, terminal.error, terminal.result
        )
    }

    fn fail_protocol_operation(&mut self, request: OperationRequest, detail: String) -> String {
        let terminal = self.core.complete(
            request.operation_id,
            request.idempotency_key,
            OperationResult::Failed,
            ErrorCategory::Protocol,
        );
        format!(
            "Listener BLE {:?} failed category={:?} result={:?}: {detail}",
            request.kind, terminal.error, terminal.result
        )
    }

    fn elapsed_ms(&self) -> u32 {
        self.started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u32::MAX)) as u32
    }
}

fn next_operation_id(sequence: &AtomicU64) -> u64 {
    let operation_id = sequence.fetch_add(1, Ordering::Relaxed);
    if operation_id == 0 {
        sequence.fetch_add(1, Ordering::Relaxed)
    } else {
        operation_id
    }
}

fn listener_capability_mask(capabilities: &str) -> u64 {
    capabilities
        .split(';')
        .map(str::trim)
        .fold(0u64, |mask, capability| {
            mask | match capability {
                "ble_audio_vka1" => LISTENER_CAPABILITY_BLE_AUDIO_VKA1,
                "ble_audio_control_v1" => LISTENER_CAPABILITY_BLE_AUDIO_CONTROL_V1,
                "voice_record_toggle" => LISTENER_CAPABILITY_VOICE_RECORD_TOGGLE,
                "denzic_ota_v1" => LISTENER_CAPABILITY_OTA_V1,
                "device_control_v1" => LISTENER_CAPABILITY_DEVICE_CONTROL_V1,
                _ => 0,
            }
        })
}

pub(crate) fn classify_ble_error_category(error: &str) -> ErrorCategory {
    use crate::embedded_ble::BleFailureKind;

    match crate::embedded_ble::classify_ble_failure(error).kind {
        BleFailureKind::DeviceMissing | BleFailureKind::DeviceAsleep => ErrorCategory::Device,
        BleFailureKind::AccessDenied => ErrorCategory::Security,
        BleFailureKind::MissingDisFirmwareRevision => ErrorCategory::Protocol,
        BleFailureKind::UnsupportedPlatform => ErrorCategory::Unsupported,
        BleFailureKind::WindowsBluetoothServiceResetNeeded => ErrorCategory::Host,
        BleFailureKind::MissingPairing
        | BleFailureKind::LowPowerIdleDisconnect
        | BleFailureKind::PairedButDisconnected
        | BleFailureKind::StaleGattService
        | BleFailureKind::CccdProtocolError
        | BleFailureKind::BackgroundListenerContention
        | BleFailureKind::OtaRebootWindow
        | BleFailureKind::Unknown => ErrorCategory::Transport,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorized_type_owned_recovery_executes_once_and_reaches_terminal_success() {
        let mut transaction = BackgroundPairingRecovery::begin(true, false, true);
        assert!(transaction.may_execute());
        assert_eq!(
            transaction.complete_pairing().result,
            OperationResult::Succeeded
        );
    }

    #[test]
    fn manual_unpair_and_unproven_recovery_cannot_run_pairasync() {
        let manual = BackgroundPairingRecovery::begin(true, true, true);
        assert!(!manual.may_execute());
        assert_eq!(manual.decision().error, ErrorCategory::Ownership);

        let unproven = BackgroundPairingRecovery::begin(false, false, false);
        assert!(!unproven.may_execute());
        assert_eq!(unproven.decision().error, ErrorCategory::Ownership);
    }

    #[test]
    fn failed_pairing_is_terminal_and_not_a_reclaim_loop() {
        let mut transaction = BackgroundPairingRecovery::begin(true, false, true);
        let terminal = transaction.fail_without_reclaim(ErrorCategory::Ownership);
        assert_eq!(terminal.result, OperationResult::Failed);
        assert_eq!(terminal.error, ErrorCategory::Ownership);
        assert!(!terminal.execute);
    }

    #[test]
    fn ble_settings_transaction_uses_firmware_capability_and_revision_readback() {
        let mut transaction = BleDeviceSettingsTransaction::from_capabilities(
            "ble_audio_vka1;ble_audio_control_v1;device_control_v1;denzic_ota_v1",
        )
        .expect("device-control firmware capability");
        transaction
            .report_capabilities()
            .expect("capability transaction");
        assert_eq!(
            transaction.core.capability_mask,
            LISTENER_CAPABILITY_BLE_AUDIO_VKA1
                | LISTENER_CAPABILITY_BLE_AUDIO_CONTROL_V1
                | LISTENER_CAPABILITY_OTA_V1
                | LISTENER_CAPABILITY_DEVICE_CONTROL_V1
        );
        assert_eq!(
            transaction.core.capability_revision,
            LISTENER_DEVICE_CONTROL_CAPABILITY_REVISION
        );

        let initial_read = transaction
            .begin_operation(OperationKind::ReadSetting)
            .expect("initial revision read");
        assert!(transaction.core.confirm_setting_readback(
            initial_read.operation_id,
            initial_read.idempotency_key,
            41
        ));

        let write = transaction
            .begin_operation(OperationKind::WriteSetting)
            .expect("settings write");
        assert_eq!(write.expected_settings_revision, 41);
        assert!(transaction.core.acknowledge_setting_write(
            write.operation_id,
            write.idempotency_key,
            41
        ));
        assert!(transaction.core.confirm_setting_readback(
            write.operation_id,
            write.idempotency_key,
            42
        ));
        assert_eq!(transaction.core.settings_revision, 42);
    }

    #[test]
    fn ble_settings_transaction_rejects_legacy_capabilities_and_classifies_access() {
        let error = BleDeviceSettingsTransaction::from_capabilities("ble_audio_vka1")
            .err()
            .expect("legacy firmware must not be treated as device-control capable");
        assert!(error.contains("device_control_v1"));
        assert_eq!(
            classify_ble_error_category("access denied by Windows Bluetooth"),
            ErrorCategory::Security
        );
    }
}
