//! Windows BLE receiver for the embedded VKA1 audio service.
//!
//! This module only owns BLE discovery/subscription. Protocol parsing and
//! dictation finalization stay in `embedded_audio` and `coordinator`.

use std::time::Duration;

use serde::Serialize;

pub const DIAGNOSTIC_SERVICE_UUID_TEXT: &str =
    denzic_observability_v1_core::DIAG_LOG_GATT_SERVICE_UUID;
pub const DIAGNOSTIC_CONTROL_UUID_TEXT: &str =
    denzic_observability_v1_core::DIAG_LOG_GATT_CONTROL_UUID;
pub const DIAGNOSTIC_DATA_UUID_TEXT: &str = denzic_observability_v1_core::DIAG_LOG_GATT_DATA_UUID;
pub const DIAGNOSTIC_COUNT_UUID_TEXT: &str = denzic_observability_v1_core::DIAG_LOG_GATT_COUNT_UUID;
pub const DEVICE_SETTINGS_REVISION_UUID_TEXT: &str =
    denzic_device_control_v1_core::SETTINGS_REVISION_CHARACTERISTIC_UUID;
pub const LISTENER_OTA_V1_SERVICE_UUID_TEXT: &str = denzic_ota_core::GATT_SERVICE_UUID;
pub const LISTENER_OTA_V1_CONTROL_UUID_TEXT: &str = denzic_ota_core::GATT_CONTROL_UUID;
pub const LISTENER_OTA_V1_DATA_UUID_TEXT: &str = denzic_ota_core::GATT_DATA_UUID;
pub const LISTENER_OTA_V1_STATUS_UUID_TEXT: &str = denzic_ota_core::GATT_STATUS_UUID;
pub const DIAGNOSTIC_EVENT_BYTES: usize = denzic_observability_v1_core::DIAG_LOG_EVENT_WIRE_BYTES;
pub const DIAGNOSTIC_CHUNK_HEADER_BYTES: usize =
    denzic_observability_v1_core::DIAG_LOG_CHUNK_HEADER_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleNotificationEvent {
    pub notification: Vec<u8>,
    pub terminal: bool,
}

pub type BleNotificationHandler<'a> = dyn FnMut(BleNotificationEvent) -> Result<(), String> + 'a;
pub type BleReadyHandler<'a> = dyn FnMut() -> Result<(), String> + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareOtaTransferStats {
    pub bytes_transferred: usize,
    pub chunks_sent: usize,
    pub transport: &'static str,
    pub data_write_elapsed_ms: u64,
    pub control_write_elapsed_ms: u64,
    pub status_read_elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaDeviceSnapshot {
    pub connected: bool,
    pub hardware_revision: Option<String>,
    pub firmware_version: Option<String>,
    pub capabilities: Vec<String>,
    pub battery_percent: Option<u8>,
    pub usb_powered: Option<bool>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedAudioBleStatus {
    pub connected: bool,
    pub readiness: Option<String>,
    pub capabilities: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettingsStatus {
    pub brightness_percent: u8,
    pub plugged_brightness_percent: u8,
    pub battery_brightness_percent: u8,
    pub status_led_brightness_percent: u8,
    pub key_led_brightness_percent: u8,
    pub knob_led_brightness_percent: u8,
    pub edge_led_brightness_percent: u8,
    pub led_zone_brightness_supported: bool,
    pub compact_set_supported: bool,
    pub low_power_idle_minutes: u32,
    pub plugged_low_power_idle_minutes: u32,
    pub battery_low_power_idle_minutes: u32,
    pub plugged_low_power_enabled: bool,
    pub voice_auto_start_enabled: bool,
    pub voice_auto_stop_enabled: bool,
    pub plugged_auto_shutdown_minutes: u32,
    pub battery_auto_shutdown_minutes: u32,
    pub settings_revision: u32,
    pub knob_rotation_action: String,
    pub ble_name: String,
    pub ble_name_pending_restart: bool,
    pub external_power_present: bool,
    pub usb_power_present: bool,
    pub charging: bool,
    pub charge_full: bool,
    pub raw_line: String,
}

pub(crate) fn parse_device_settings_revision_characteristic(value: &str) -> Result<u32, String> {
    denzic_device_control_v1_core::parse_settings_revision_value(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSettingsCommandTransport {
    ActiveCapture,
    UsbSerial,
    FreshGatt,
    StatusReadback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BleFailureKind {
    DeviceMissing,
    DeviceAsleep,
    MissingPairing,
    LowPowerIdleDisconnect,
    PairedButDisconnected,
    StaleGattService,
    CccdProtocolError,
    MissingDisFirmwareRevision,
    BackgroundListenerContention,
    OtaRebootWindow,
    WindowsBluetoothServiceResetNeeded,
    AccessDenied,
    UnsupportedPlatform,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleFailureClassification {
    pub kind: BleFailureKind,
    pub retryable: bool,
    pub automatic_recovery: bool,
    pub user_action: &'static str,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BleDeviceUnpairStatus {
    NotFound,
    Removed,
    AlreadyClean,
    NeedsUserAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleDeviceUnpairResult {
    pub status: BleDeviceUnpairStatus,
    pub attempted: bool,
    pub matched_devices: u32,
    pub unpaired_devices: u32,
    pub already_unpaired_devices: u32,
    pub failed_devices: u32,
    pub needs_user_action: bool,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BleDevicePairingPromptStatus {
    NotFound,
    Paired,
    AlreadyPaired,
    NeedsUserAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleDevicePairingPromptResult {
    pub status: BleDevicePairingPromptStatus,
    pub attempted: bool,
    pub matched_devices: u32,
    pub prompted_devices: u32,
    pub already_paired_devices: u32,
    pub failed_devices: u32,
    pub open_bluetooth_settings: bool,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenerRecoveryPairingAdvertisementProbe {
    pub visible: bool,
    pub has_random_identity: bool,
    pub addresses: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleDiagnosticServiceEntry {
    pub selector: &'static str,
    pub service_uuid: &'static str,
    pub index: u32,
    pub name: String,
    pub id: String,
    pub bluetooth_address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleDiagnosticSnapshot {
    pub captured_at: String,
    pub platform: &'static str,
    pub audio_service_uuid: &'static str,
    pub ota_service_uuid: &'static str,
    pub diagnostic_service_uuid: &'static str,
    pub dis_service_uuid: &'static str,
    pub configured_device_address: Option<String>,
    pub audio_services: Vec<BleDiagnosticServiceEntry>,
    pub ota_services: Vec<BleDiagnosticServiceEntry>,
    pub diagnostic_services: Vec<BleDiagnosticServiceEntry>,
    pub firmware_snapshot: FirmwareOtaDeviceSnapshot,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareDiagnosticLogChunk {
    pub offset: usize,
    pub event_count: u16,
    pub value_bytes: usize,
    pub events_crc32: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareDiagnosticLogPull {
    pub captured_at: String,
    pub status: &'static str,
    pub platform: &'static str,
    pub service_uuid: &'static str,
    pub control_uuid: &'static str,
    pub data_uuid: &'static str,
    pub count_uuid: &'static str,
    pub event_wire_bytes: usize,
    pub initial_count: Option<u32>,
    pub export_count_snapshot: Option<u32>,
    pub final_count: Option<u32>,
    pub exported_event_count: u32,
    pub chunk_count: usize,
    pub max_events_per_chunk_observed: u16,
    pub max_value_bytes_observed: usize,
    pub event_bytes: usize,
    pub aggregate_crc32: Option<String>,
    pub events_sha256: Option<String>,
    pub chunks: Vec<FirmwareDiagnosticLogChunk>,
    pub error: Option<String>,
    #[serde(skip_serializing)]
    pub raw_event_bytes: Vec<u8>,
}

impl FirmwareDiagnosticLogPull {
    pub fn offline(platform: &'static str, error: impl Into<String>) -> Self {
        Self {
            captured_at: utc_now_rfc3339(),
            status: "offline",
            platform,
            service_uuid: DIAGNOSTIC_SERVICE_UUID_TEXT,
            control_uuid: DIAGNOSTIC_CONTROL_UUID_TEXT,
            data_uuid: DIAGNOSTIC_DATA_UUID_TEXT,
            count_uuid: DIAGNOSTIC_COUNT_UUID_TEXT,
            event_wire_bytes: DIAGNOSTIC_EVENT_BYTES,
            initial_count: None,
            export_count_snapshot: None,
            final_count: None,
            exported_event_count: 0,
            chunk_count: 0,
            max_events_per_chunk_observed: 0,
            max_value_bytes_observed: 0,
            event_bytes: 0,
            aggregate_crc32: None,
            events_sha256: None,
            chunks: Vec::new(),
            error: Some(error.into()),
            raw_event_bytes: Vec::new(),
        }
    }

    pub fn from_events(
        platform: &'static str,
        initial_count: u32,
        export_count_snapshot: u32,
        final_count: u32,
        chunks: Vec<FirmwareDiagnosticLogChunk>,
        raw_event_bytes: Vec<u8>,
    ) -> Self {
        let max_events_per_chunk_observed = chunks
            .iter()
            .map(|chunk| chunk.event_count)
            .max()
            .unwrap_or(0);
        let max_value_bytes_observed = chunks
            .iter()
            .map(|chunk| chunk.value_bytes)
            .max()
            .unwrap_or(0);
        Self {
            captured_at: utc_now_rfc3339(),
            status: "ok",
            platform,
            service_uuid: DIAGNOSTIC_SERVICE_UUID_TEXT,
            control_uuid: DIAGNOSTIC_CONTROL_UUID_TEXT,
            data_uuid: DIAGNOSTIC_DATA_UUID_TEXT,
            count_uuid: DIAGNOSTIC_COUNT_UUID_TEXT,
            event_wire_bytes: DIAGNOSTIC_EVENT_BYTES,
            initial_count: Some(initial_count),
            export_count_snapshot: Some(export_count_snapshot),
            final_count: Some(final_count),
            exported_event_count: raw_event_bytes
                .len()
                .checked_div(DIAGNOSTIC_EVENT_BYTES)
                .unwrap_or(0) as u32,
            chunk_count: chunks.len(),
            max_events_per_chunk_observed,
            max_value_bytes_observed,
            event_bytes: raw_event_bytes.len(),
            aggregate_crc32: Some(format_crc32(denzic_ota_core::crc32_ieee(&raw_event_bytes))),
            events_sha256: Some(crate::firmware_ota::sha256_hex(&raw_event_bytes)),
            chunks,
            error: None,
            raw_event_bytes,
        }
    }
}

pub(crate) const LISTENER_BLE_FAILURE_HINTS: denzic_ble_windows::failure::BleFailureHints =
    denzic_ble_windows::failure::BleFailureHints {
        background_contention: &["background listener"],
        missing_pairing: &["no paired listener", "pair the listener"],
        device_asleep: &["press key4", "key4"],
        device_missing: &["no writable listener ble ota"],
    };

pub fn classify_ble_failure(error: &str) -> BleFailureClassification {
    let classification = denzic_ble_windows::failure::classify_ble_failure_with_hints(
        error,
        &LISTENER_BLE_FAILURE_HINTS,
    );
    let kind = map_platform_ble_failure_kind(classification.kind);
    BleFailureClassification {
        kind,
        retryable: classification.retryable,
        automatic_recovery: classification.automatic_recovery,
        user_action: listener_ble_failure_user_action(kind),
        evidence: classification.evidence,
    }
}

fn map_platform_ble_failure_kind(kind: denzic_ble_windows::BleFailureKind) -> BleFailureKind {
    match kind {
        denzic_ble_windows::BleFailureKind::DeviceMissing => BleFailureKind::DeviceMissing,
        denzic_ble_windows::BleFailureKind::DeviceAsleep => BleFailureKind::DeviceAsleep,
        denzic_ble_windows::BleFailureKind::MissingPairing => BleFailureKind::MissingPairing,
        denzic_ble_windows::BleFailureKind::LowPowerIdleDisconnect => {
            BleFailureKind::LowPowerIdleDisconnect
        }
        denzic_ble_windows::BleFailureKind::PairedButDisconnected => {
            BleFailureKind::PairedButDisconnected
        }
        denzic_ble_windows::BleFailureKind::StaleGattService => BleFailureKind::StaleGattService,
        denzic_ble_windows::BleFailureKind::CccdProtocolError => BleFailureKind::CccdProtocolError,
        denzic_ble_windows::BleFailureKind::MissingDisFirmwareRevision => {
            BleFailureKind::MissingDisFirmwareRevision
        }
        denzic_ble_windows::BleFailureKind::BackgroundContention => {
            BleFailureKind::BackgroundListenerContention
        }
        denzic_ble_windows::BleFailureKind::OtaRebootWindow => BleFailureKind::OtaRebootWindow,
        denzic_ble_windows::BleFailureKind::WindowsBluetoothServiceResetNeeded => {
            BleFailureKind::WindowsBluetoothServiceResetNeeded
        }
        denzic_ble_windows::BleFailureKind::AccessDenied => BleFailureKind::AccessDenied,
        denzic_ble_windows::BleFailureKind::UnsupportedPlatform => {
            BleFailureKind::UnsupportedPlatform
        }
        denzic_ble_windows::BleFailureKind::Unknown => BleFailureKind::Unknown,
    }
}

fn listener_ble_failure_user_action(kind: BleFailureKind) -> &'static str {
    match kind {
        BleFailureKind::DeviceMissing => {
            "Wake the Listener device, confirm it is paired, then retry or re-pair."
        }
        BleFailureKind::DeviceAsleep => {
            "Press KEY4 or the wake key, wait for the device to return online, then retry."
        }
        BleFailureKind::MissingPairing => {
            "Pair the Listener device in Windows Bluetooth, then return and refresh Listener BLE."
        }
        BleFailureKind::LowPowerIdleDisconnect => {
            "Listener BLE entered offline state; retrying will reconnect, or press KEY4 if it is offline."
        }
        BleFailureKind::PairedButDisconnected => {
            "Wait for automatic reconnect; press the wake key if it stays disconnected."
        }
        BleFailureKind::StaleGattService => {
            "Retry after Listener Type refreshes the GATT path; re-pair if stale services persist."
        }
        BleFailureKind::CccdProtocolError => {
            "Retry after the notify subscription is reopened; reboot Type if repeated."
        }
        BleFailureKind::MissingDisFirmwareRevision => {
            "Collect diagnostics and update firmware readiness/DIS exposure before release."
        }
        BleFailureKind::BackgroundListenerContention => {
            "Pause the competing BLE operation and retry through the shared listener path."
        }
        BleFailureKind::OtaRebootWindow => {
            "Wait for the OTA reboot window to finish, then refresh device status."
        }
        BleFailureKind::WindowsBluetoothServiceResetNeeded => {
            "Toggle Windows Bluetooth or restart the Bluetooth Support Service, then retry."
        }
        BleFailureKind::AccessDenied => {
            "Allow Bluetooth/device access in Windows settings or re-pair the device."
        }
        BleFailureKind::UnsupportedPlatform => {
            "Use the supported Windows BLE path for this diagnostic."
        }
        BleFailureKind::Unknown => "Export diagnostics and retry after restarting Listener Type.",
    }
}

fn utc_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub(crate) use denzic_ble_windows::format_bluetooth_address;

fn format_crc32(value: u32) -> String {
    format!("0x{value:08x}")
}

// EC11 recovery handshake tokens come from the shared platform contract
// (`denzic_device_control_v1`); only the write-side aliases live here.
const EC11_HARDWARE_RECOVERY_ACK: &[u8] = denzic_device_control_v1_core::EC11_RECOVERY_ACK_WRITE;
const EC11_HARDWARE_RECOVERY_PREPARE_ACK: &[u8] =
    denzic_device_control_v1_core::EC11_RECOVERY_PREPARE_ACK_WRITE;

fn is_ec11_hardware_recovery_prepare_notice(notification: &[u8]) -> bool {
    denzic_device_control_v1_core::is_ec11_recovery_prepare_notice(notification)
}

fn is_ec11_hardware_recovery_notice(notification: &[u8]) -> bool {
    denzic_device_control_v1_core::is_ec11_recovery_notice(notification)
}

fn is_terminal_notification(notification: &[u8]) -> bool {
    crate::embedded_audio::parse_packet(notification)
        .map(|packet| {
            matches!(
                packet.header.packet_type,
                crate::embedded_audio::PacketType::SessionStop
                    | crate::embedded_audio::PacketType::SessionCancel
                    | crate::embedded_audio::PacketType::SessionError
            )
        })
        .unwrap_or(false)
}

// After STOP, keep the data plane open until the packet-count contract is
// satisfied. This timeout is rolling idle time after the last notification,
// not a hard cap from STOP, so tail packets can still arrive without UI linger.
const STOP_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

fn stop_drain_timeout_reason(stats: &crate::embedded_audio::SessionStats) -> String {
    format!(
        "BLE embedded audio stop drain idle timed out after {} ms (expected={:?}, received={}, missing={:?})",
        STOP_DRAIN_TIMEOUT.as_millis(),
        stats.expected_packet_count,
        stats.received_packet_count,
        stats.missing_packet_indices
    )
}

#[cfg(target_os = "windows")]
mod windows_ble;

#[cfg(target_os = "windows")]
pub fn capture_notifications_once(timeout: Duration) -> Result<Vec<Vec<u8>>, String> {
    windows_ble::capture_notifications_once(timeout)
}

#[cfg(target_os = "windows")]
pub fn probe_notify_subscription(timeout: Duration) -> Result<(), String> {
    windows_ble::probe_notify_subscription(timeout)
}

#[cfg(target_os = "windows")]
pub fn read_embedded_audio_status(timeout: Duration) -> Result<EmbeddedAudioBleStatus, String> {
    windows_ble::read_embedded_audio_status(timeout)
}

#[cfg(target_os = "windows")]
pub fn read_embedded_audio_status_for_device(
    address: u64,
    timeout: Duration,
) -> Result<EmbeddedAudioBleStatus, String> {
    windows_ble::read_embedded_audio_status_for_device(address, timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_toggle(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_toggle(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_cancel(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_cancel(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_activate(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_activate(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_stop(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_stop(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_recovery(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_recovery(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_manual_pairing(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_manual_pairing(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_silent_recovery(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_silent_recovery(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_type_bye(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_type_bye(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_processing_state(active: bool, timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_processing_state(active, timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_processing_done(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_processing_done(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_processing_warning(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_processing_warning(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_ec11_rotation_mode(mode: &str, timeout: Duration) -> Result<(), String> {
    windows_ble::send_ec11_rotation_mode(mode, timeout)
}

#[cfg(target_os = "windows")]
pub fn send_device_settings_command(command: &str, timeout: Duration) -> Result<(), String> {
    windows_ble::send_device_settings_command(command, timeout)
}

#[cfg(target_os = "windows")]
pub fn send_device_settings_command_via_active_capture_only(
    command: &str,
    timeout: Duration,
    label: &str,
) -> Result<(), String> {
    windows_ble::send_device_settings_command_via_active_capture_only(command, timeout, label)
}

#[cfg(target_os = "windows")]
pub fn apply_pending_ble_name(timeout: Duration) -> Result<DeviceSettingsCommandTransport, String> {
    windows_ble::apply_pending_ble_name(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_status_led_command(command: &str, timeout: Duration) -> Result<(), String> {
    windows_ble::send_status_led_command(command, timeout)
}

#[cfg(all(target_os = "windows", test))]
pub fn read_status_led_brightness_status(timeout: Duration) -> Result<String, String> {
    windows_ble::read_status_led_brightness_status(timeout)
}

#[cfg(target_os = "windows")]
pub fn read_device_settings_status(timeout: Duration) -> Result<DeviceSettingsStatus, String> {
    windows_ble::read_device_settings_status(timeout)
}

#[cfg(target_os = "windows")]
pub fn read_device_settings_revision(timeout: Duration) -> Result<u32, String> {
    windows_ble::read_device_settings_revision(timeout)
}

#[cfg(target_os = "windows")]
pub fn capture_notification_events(
    timeout: Duration,
    on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    windows_ble::capture_notification_events(timeout, on_event)
}

#[cfg(target_os = "windows")]
pub fn notify_capture_session_active() -> bool {
    windows_ble::notify_capture_session_active()
}

#[cfg(target_os = "windows")]
pub fn capture_notification_events_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    on_ready: &mut BleReadyHandler<'_>,
    on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    windows_ble::capture_notification_events_until_cancelled(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
    )
}

#[cfg(target_os = "windows")]
pub fn capture_notification_events_continuous_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    on_ready: &mut BleReadyHandler<'_>,
    on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    windows_ble::capture_notification_events_continuous_until_cancelled(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
    )
}

#[cfg(target_os = "windows")]
pub fn capture_notification_events_continuous_with_connection_handoff_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    leave_notify_cccd_enabled_on_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    on_ready: &mut BleReadyHandler<'_>,
    on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    windows_ble::capture_notification_events_continuous_with_connection_handoff_until_cancelled(
        idle_timeout,
        cancel_requested,
        leave_notify_cccd_enabled_on_cancel,
        on_ready,
        on_event,
    )
}

#[cfg(target_os = "windows")]
pub fn is_background_listener_deferred_for_ota_error(err: &str) -> bool {
    windows_ble::is_background_listener_deferred_for_ota_error(err)
}

#[cfg(target_os = "windows")]
pub fn transfer_stm32wb_st_ota(
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("STM32WB ST BLE OTA is handled by the separate Companion-Type app.".to_string())
}

#[cfg(target_os = "windows")]
pub fn request_listener_ota_v1_active_link(
    observability_correlation_id: Option<u64>,
) -> Result<(), String> {
    windows_ble::request_listener_ota_v1_active_link(observability_correlation_id)
}

#[cfg(not(target_os = "windows"))]
pub fn request_listener_ota_v1_active_link(
    _observability_correlation_id: Option<u64>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn request_listener_ota_post_confirm_notify_fast_retry() {
    windows_ble::request_listener_ota_post_confirm_notify_fast_retry()
}

#[cfg(not(target_os = "windows"))]
pub fn request_listener_ota_post_confirm_notify_fast_retry() {}

#[cfg(target_os = "windows")]
pub fn transfer_listener_ota_v1(
    firmware_sha256: &str,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_listener_ota_v1(
        firmware_sha256,
        firmware_bytes,
        manifest_chunk_bytes,
        on_progress,
    )
}

#[cfg(target_os = "windows")]
pub fn transfer_listener_ota_v1_after_active_link_hint(
    firmware_sha256: &str,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_listener_ota_v1_after_active_link_hint(
        firmware_sha256,
        firmware_bytes,
        manifest_chunk_bytes,
        on_progress,
    )
}

#[cfg(target_os = "windows")]
pub fn transfer_listener_ota_v1_after_active_link_hint_staged(
    target_ready: std::sync::mpsc::SyncSender<Result<(), String>>,
    start_transfer: std::sync::mpsc::Receiver<()>,
    target_prepare_timeout: std::time::Duration,
    firmware_sha256: &str,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_listener_ota_v1_after_active_link_hint_staged(
        target_ready,
        start_transfer,
        target_prepare_timeout,
        firmware_sha256,
        firmware_bytes,
        manifest_chunk_bytes,
        on_progress,
    )
}

#[cfg(target_os = "windows")]
pub struct Stm32wbStOtaPreparedTransfer;

#[cfg(target_os = "windows")]
pub struct ListenerOtaV1PreparedTransfer(windows_ble::PreparedListenerOtaV1Transfer);

#[cfg(target_os = "windows")]
impl Stm32wbStOtaPreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_stm32wb_st_ota_transfer is handled by Companion-Type")
    }

    pub fn transfer(
        self,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
        _on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err("STM32WB ST BLE OTA is handled by the separate Companion-Type app.".to_string())
    }
}

#[cfg(target_os = "windows")]
impl ListenerOtaV1PreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        self.0.snapshot()
    }

    pub fn transfer(
        self,
        firmware_sha256: &str,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        self.0.transfer(
            firmware_sha256,
            firmware_bytes,
            manifest_chunk_bytes,
            on_progress,
        )
    }
}

#[cfg(target_os = "windows")]
pub fn prepare_stm32wb_st_ota_transfer() -> Result<Stm32wbStOtaPreparedTransfer, String> {
    Err("STM32WB ST BLE OTA is handled by the separate Companion-Type app.".to_string())
}

#[cfg(target_os = "windows")]
pub fn prepare_listener_ota_v1_transfer() -> Result<ListenerOtaV1PreparedTransfer, String> {
    windows_ble::prepare_listener_ota_v1_transfer().map(ListenerOtaV1PreparedTransfer)
}

#[cfg(target_os = "windows")]
pub fn firmware_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::firmware_ota_device_snapshot()
}

#[cfg(target_os = "windows")]
pub fn stm32wb_st_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some(
            "STM32WB ST BLE OTA is handled by the separate Companion-Type app.".to_string(),
        ),
    }
}

#[cfg(target_os = "windows")]
pub fn listener_ota_v1_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::listener_ota_v1_device_snapshot()
}

#[cfg(target_os = "windows")]
pub fn listener_ota_v1_gatt_probe_snapshot(timeout: Duration) -> FirmwareOtaDeviceSnapshot {
    windows_ble::listener_ota_v1_gatt_probe_snapshot(timeout)
}

#[cfg(target_os = "windows")]
pub fn listener_ota_v1_gatt_probe_after_active_link_hint(
    timeout: Duration,
) -> FirmwareOtaDeviceSnapshot {
    windows_ble::listener_ota_v1_gatt_probe_after_active_link_hint(timeout)
}

#[cfg(target_os = "windows")]
pub fn listener_ota_v1_service_reachable_snapshot(timeout: Duration) -> FirmwareOtaDeviceSnapshot {
    windows_ble::listener_ota_v1_service_reachable_snapshot(timeout)
}

#[cfg(target_os = "windows")]
pub fn pull_firmware_diagnostic_log(timeout: Duration) -> FirmwareDiagnosticLogPull {
    windows_ble::pull_firmware_diagnostic_log(timeout)
}

#[cfg(target_os = "windows")]
pub fn ble_diagnostic_snapshot() -> BleDiagnosticSnapshot {
    windows_ble::diagnostic_snapshot()
}

#[cfg(target_os = "windows")]
pub fn unpair_listener_devices() -> BleDeviceUnpairResult {
    windows_ble::unpair_listener_devices()
}

#[cfg(target_os = "windows")]
pub fn unpair_listener_devices_for_names(extra_names: &[String]) -> BleDeviceUnpairResult {
    windows_ble::unpair_listener_devices_for_names(extra_names)
}

#[cfg(target_os = "windows")]
pub fn unpair_listener_devices_for_known_addresses(
    extra_names: &[String],
    addresses: &[u64],
) -> BleDeviceUnpairResult {
    windows_ble::unpair_listener_devices_for_known_addresses(extra_names, addresses)
}

#[cfg(target_os = "windows")]
pub fn unpair_listener_pairing_for_known_addresses(
    extra_names: &[String],
    addresses: &[u64],
) -> BleDeviceUnpairResult {
    windows_ble::unpair_listener_pairing_for_known_addresses(extra_names, addresses)
}

#[cfg(target_os = "windows")]
pub fn clear_listener_bthport_cache_for_known_addresses(
    extra_names: &[String],
    addresses: &[u64],
) -> BleDeviceUnpairResult {
    windows_ble::clear_listener_bthport_cache_for_known_addresses(extra_names, addresses)
}

#[cfg(target_os = "windows")]
pub fn prompt_listener_pairing(expected_name: Option<&str>) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing(expected_name)
}

#[cfg(target_os = "windows")]
pub fn prompt_listener_pairing_for_recovery(
    expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing_for_recovery(expected_name)
}

#[cfg(target_os = "windows")]
pub fn prompt_listener_pairing_for_recovery_without_user_prompt(
    expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing_for_recovery_without_user_prompt(expected_name)
}

#[cfg(target_os = "windows")]
pub fn prompt_listener_pairing_after_type_recovery(
    expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing_after_type_recovery(expected_name)
}

#[cfg(target_os = "windows")]
pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt(
    expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt(expected_name)
}

#[cfg(target_os = "windows")]
pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup(
    expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup(
        expected_name,
    )
}

#[cfg(target_os = "windows")]
pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
    expected_name: Option<&str>,
    observed_recovery_addresses: &[u64],
) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
        expected_name,
        observed_recovery_addresses,
    )
}

#[cfg(target_os = "windows")]
pub fn query_listener_pairing(expected_name: Option<&str>) -> BleDevicePairingPromptResult {
    windows_ble::query_listener_pairing(expected_name)
}

#[cfg(target_os = "windows")]
pub fn native_windows_hid_pairing_addresses() -> Result<Vec<u64>, String> {
    windows_ble::native_windows_hid_pairing_addresses()
}

#[cfg(target_os = "windows")]
pub fn native_windows_hid_present_pairing_addresses() -> Result<Vec<u64>, String> {
    windows_ble::native_windows_hid_present_pairing_addresses()
}

#[cfg(target_os = "windows")]
pub fn warm_native_windows_hid_present_pairing_snapshot() {
    windows_ble::warm_native_windows_hid_present_pairing_snapshot()
}

#[cfg(target_os = "windows")]
pub fn native_windows_hid_present_pairing_addresses_for_startup() -> Result<Vec<u64>, String> {
    windows_ble::native_windows_hid_present_pairing_addresses_for_startup()
}

#[cfg(target_os = "windows")]
pub fn native_windows_hid_pairing_active_connection(
    addresses: &[u64],
) -> Result<Option<u64>, String> {
    windows_ble::native_windows_hid_pairing_active_connection(addresses)
}

#[cfg(target_os = "windows")]
pub fn listener_pairing_maintenance_active() -> bool {
    windows_ble::listener_pairing_maintenance_active()
}

#[cfg(target_os = "windows")]
pub fn listener_recovery_pairing_advertisement_visible(
    expected_name: Option<&str>,
    timeout: Duration,
) -> bool {
    windows_ble::listener_recovery_pairing_advertisement_visible(expected_name, timeout)
}

#[cfg(target_os = "windows")]
pub fn listener_recovery_pairing_advertisement_probe(
    expected_name: Option<&str>,
    timeout: Duration,
) -> ListenerRecoveryPairingAdvertisementProbe {
    windows_ble::listener_recovery_pairing_advertisement_probe(expected_name, timeout)
}

#[cfg(target_os = "windows")]
pub fn listener_ble_name_cache_needs_cleanup(expected_name: &str) -> bool {
    windows_ble::listener_ble_name_cache_needs_cleanup(expected_name)
}

#[cfg(target_os = "windows")]
pub fn listener_ble_name_cache_needs_cleanup_for_names(
    expected_name: &str,
    extra_names: &[String],
) -> bool {
    windows_ble::listener_ble_name_cache_needs_cleanup_for_names(expected_name, extra_names)
}

#[cfg(target_os = "windows")]
pub fn set_configured_bluetooth_target_name(name: &str) {
    windows_ble::set_configured_bluetooth_target_name(name)
}

#[cfg(target_os = "windows")]
pub fn wait_for_bluetooth_target_advertisement_by_name(
    target_name: &str,
    timeout: Duration,
    context: &str,
) -> Result<u64, String> {
    windows_ble::wait_for_bluetooth_target_advertisement_by_name(target_name, timeout, context)
}

#[cfg(target_os = "windows")]
pub fn verified_bluetooth_target_rename_handoff_address() -> Option<u64> {
    windows_ble::verified_bluetooth_target_rename_handoff_address()
}

#[cfg(not(target_os = "windows"))]
pub fn capture_notifications_once(_timeout: Duration) -> Result<Vec<Vec<u8>>, String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn probe_notify_subscription(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn read_embedded_audio_status(_timeout: Duration) -> Result<EmbeddedAudioBleStatus, String> {
    Err("Embedded BLE audio status is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_toggle(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_cancel(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording cancel is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_activate(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording activation is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_stop(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording stop is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_recovery(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recovery is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_manual_pairing(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE manual pairing recovery is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_silent_recovery(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE silent recovery is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_type_bye(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE Type heartbeat is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_processing_state(_active: bool, _timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording processing control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_processing_done(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording processing control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_processing_warning(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording processing control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_ec11_rotation_mode(_mode: &str, _timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE EC11 rotation control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_device_settings_command(_command: &str, _timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE device settings control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_device_settings_command_via_active_capture_only(
    _command: &str,
    _timeout: Duration,
    _label: &str,
) -> Result<(), String> {
    Err("Embedded BLE active control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn apply_pending_ble_name(
    _timeout: Duration,
) -> Result<DeviceSettingsCommandTransport, String> {
    Err("Embedded BLE name apply is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_status_led_command(_command: &str, _timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE status LED control is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn read_device_settings_status(_timeout: Duration) -> Result<DeviceSettingsStatus, String> {
    Err("Listener device settings refresh is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn read_device_settings_revision(_timeout: Duration) -> Result<u32, String> {
    Err("Listener device settings revision is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn capture_notification_events(
    _timeout: Duration,
    _on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn notify_capture_session_active() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn capture_notification_events_until_cancelled(
    _idle_timeout: Option<Duration>,
    _cancel_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _on_ready: &mut BleReadyHandler<'_>,
    _on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn capture_notification_events_continuous_until_cancelled(
    _idle_timeout: Option<Duration>,
    _cancel_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _on_ready: &mut BleReadyHandler<'_>,
    _on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn capture_notification_events_continuous_with_connection_handoff_until_cancelled(
    _idle_timeout: Option<Duration>,
    _cancel_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _leave_notify_cccd_enabled_on_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _on_ready: &mut BleReadyHandler<'_>,
    _on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn is_background_listener_deferred_for_ota_error(_err: &str) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn transfer_listener_ota_v1(
    _firmware_sha256: &str,
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("Listener OTA v1 over BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn transfer_listener_ota_v1_after_active_link_hint(
    _firmware_sha256: &str,
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("Listener OTA v1 over BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn transfer_listener_ota_v1_after_active_link_hint_staged(
    _target_ready: std::sync::mpsc::SyncSender<Result<(), String>>,
    _start_transfer: std::sync::mpsc::Receiver<()>,
    _target_prepare_timeout: std::time::Duration,
    _firmware_sha256: &str,
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("Listener OTA v1 over BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub struct ListenerOtaV1PreparedTransfer;

#[cfg(not(target_os = "windows"))]
impl ListenerOtaV1PreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_listener_ota_v1_transfer is unsupported on this platform")
    }

    pub fn transfer(
        self,
        _firmware_sha256: &str,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
        _on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err("Listener OTA v1 over BLE is only supported on Windows".to_string())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn prepare_listener_ota_v1_transfer() -> Result<ListenerOtaV1PreparedTransfer, String> {
    Err("Listener OTA v1 over BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn firmware_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("Firmware OTA over Listener BLE is only supported on Windows".to_string()),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn listener_ota_v1_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("Listener OTA v1 over BLE is only supported on Windows".to_string()),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn listener_ota_v1_gatt_probe_snapshot(_timeout: Duration) -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("Listener OTA v1 over BLE is only supported on Windows".to_string()),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn listener_ota_v1_gatt_probe_after_active_link_hint(
    _timeout: Duration,
) -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("Listener OTA v1 over BLE is only supported on Windows".to_string()),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn listener_ota_v1_service_reachable_snapshot(_timeout: Duration) -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("Listener OTA v1 over BLE is only supported on Windows".to_string()),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn pull_firmware_diagnostic_log(_timeout: Duration) -> FirmwareDiagnosticLogPull {
    FirmwareDiagnosticLogPull::offline(
        std::env::consts::OS,
        "Firmware diagnostic log over Listener BLE is only supported on Windows",
    )
}

#[cfg(not(target_os = "windows"))]
pub fn ble_diagnostic_snapshot() -> BleDiagnosticSnapshot {
    let detail = "Listener BLE diagnostics are only supported on Windows".to_string();
    BleDiagnosticSnapshot {
        captured_at: utc_now_rfc3339(),
        platform: std::env::consts::OS,
        audio_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3091a",
        ota_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3092a",
        diagnostic_service_uuid: DIAGNOSTIC_SERVICE_UUID_TEXT,
        dis_service_uuid: "0000180a-0000-1000-8000-00805f9b34fb",
        configured_device_address: None,
        audio_services: Vec::new(),
        ota_services: Vec::new(),
        diagnostic_services: Vec::new(),
        firmware_snapshot: firmware_ota_device_snapshot(),
        errors: vec![detail],
    }
}

#[cfg(not(target_os = "windows"))]
pub fn unpair_listener_devices() -> BleDeviceUnpairResult {
    BleDeviceUnpairResult {
        status: BleDeviceUnpairStatus::NeedsUserAction,
        attempted: false,
        matched_devices: 0,
        unpaired_devices: 0,
        already_unpaired_devices: 0,
        failed_devices: 0,
        needs_user_action: true,
        details: vec!["Listener BLE device recovery is only supported on Windows".to_string()],
    }
}

#[cfg(not(target_os = "windows"))]
pub fn unpair_listener_devices_for_names(_extra_names: &[String]) -> BleDeviceUnpairResult {
    unpair_listener_devices()
}

#[cfg(not(target_os = "windows"))]
pub fn unpair_listener_devices_for_known_addresses(
    _extra_names: &[String],
    _addresses: &[u64],
) -> BleDeviceUnpairResult {
    unpair_listener_devices()
}

#[cfg(not(target_os = "windows"))]
pub fn unpair_listener_pairing_for_known_addresses(
    _extra_names: &[String],
    _addresses: &[u64],
) -> BleDeviceUnpairResult {
    unpair_listener_devices()
}

#[cfg(not(target_os = "windows"))]
pub fn clear_listener_bthport_cache_for_known_addresses(
    _extra_names: &[String],
    _addresses: &[u64],
) -> BleDeviceUnpairResult {
    unpair_listener_devices()
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_listener_pairing(_expected_name: Option<&str>) -> BleDevicePairingPromptResult {
    BleDevicePairingPromptResult {
        status: BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: false,
        matched_devices: 0,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 0,
        open_bluetooth_settings: true,
        details: vec!["Listener BLE pairing prompt is only supported on Windows".to_string()],
    }
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_listener_pairing_for_recovery(
    _expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    prompt_listener_pairing(_expected_name)
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_listener_pairing_for_recovery_without_user_prompt(
    _expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    let mut result = prompt_listener_pairing(_expected_name);
    result.open_bluetooth_settings = false;
    result
        .details
        .push("User pairing prompt is suppressed for this Type-controlled recovery.".to_string());
    result
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_listener_pairing_after_type_recovery(
    _expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    prompt_listener_pairing(_expected_name)
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt(
    _expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    let mut result = prompt_listener_pairing(_expected_name);
    result.open_bluetooth_settings = false;
    result
        .details
        .push("User pairing prompt is suppressed for this Type-controlled recovery.".to_string());
    result
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup(
    _expected_name: Option<&str>,
) -> BleDevicePairingPromptResult {
    prompt_listener_pairing_after_type_recovery_without_user_prompt(_expected_name)
}

#[cfg(not(target_os = "windows"))]
pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
    _expected_name: Option<&str>,
    _observed_recovery_addresses: &[u64],
) -> BleDevicePairingPromptResult {
    prompt_listener_pairing_after_type_recovery_without_user_prompt(_expected_name)
}

#[cfg(not(target_os = "windows"))]
pub fn query_listener_pairing(_expected_name: Option<&str>) -> BleDevicePairingPromptResult {
    BleDevicePairingPromptResult {
        status: BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: false,
        matched_devices: 0,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 0,
        open_bluetooth_settings: false,
        details: vec!["Listener BLE pairing query is only supported on Windows".to_string()],
    }
}

#[cfg(not(target_os = "windows"))]
pub fn native_windows_hid_pairing_addresses() -> Result<Vec<u64>, String> {
    Ok(Vec::new())
}

#[cfg(not(target_os = "windows"))]
pub fn native_windows_hid_present_pairing_addresses() -> Result<Vec<u64>, String> {
    Ok(Vec::new())
}

#[cfg(not(target_os = "windows"))]
pub fn native_windows_hid_pairing_active_connection(
    _addresses: &[u64],
) -> Result<Option<u64>, String> {
    Ok(None)
}

#[cfg(not(target_os = "windows"))]
pub fn listener_pairing_maintenance_active() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn listener_recovery_pairing_advertisement_visible(
    _expected_name: Option<&str>,
    _timeout: Duration,
) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn listener_recovery_pairing_advertisement_probe(
    _expected_name: Option<&str>,
    _timeout: Duration,
) -> ListenerRecoveryPairingAdvertisementProbe {
    ListenerRecoveryPairingAdvertisementProbe::default()
}

#[cfg(not(target_os = "windows"))]
pub fn listener_ble_name_cache_needs_cleanup(_expected_name: &str) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn listener_ble_name_cache_needs_cleanup_for_names(
    _expected_name: &str,
    _extra_names: &[String],
) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn set_configured_bluetooth_target_name(_name: &str) {}

#[cfg(not(target_os = "windows"))]
pub fn wait_for_bluetooth_target_advertisement_by_name(
    _target_name: &str,
    _timeout: Duration,
    _context: &str,
) -> Result<u64, String> {
    Err("Embedded BLE advertisement scan is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn verified_bluetooth_target_rename_handoff_address() -> Option<u64> {
    None
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
