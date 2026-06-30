//! Windows BLE receiver for the embedded VKA1 audio service.
//!
//! This module only owns BLE discovery/subscription. Protocol parsing and
//! dictation finalization stay in `embedded_audio` and `coordinator`.

use std::time::Duration;

use serde::Serialize;

pub const DIAGNOSTIC_SERVICE_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3093a";
pub const DIAGNOSTIC_CONTROL_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3093b";
pub const DIAGNOSTIC_DATA_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3093c";
pub const DIAGNOSTIC_COUNT_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3093d";
#[cfg(feature = "companion-dev")]
pub const STM32WB_ST_OTA_SERVICE_UUID_TEXT: &str = "8f7a0007-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const STM32WB_ST_OTA_BASE_UUID_TEXT: &str = "8f7a7002-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const STM32WB_ST_OTA_CONFIRM_UUID_TEXT: &str = "8f7a7003-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const STM32WB_ST_OTA_RAW_UUID_TEXT: &str = "8f7a7004-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const OTA_V2_SERVICE_UUID_TEXT: &str = "8f7a8007-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const OTA_V2_CONTROL_UUID_TEXT: &str = "8f7a8002-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const OTA_V2_DATA_UUID_TEXT: &str = "8f7a8004-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const OTA_V2_STATUS_UUID_TEXT: &str = "8f7a8005-7b7d-4f3d-9d6f-6c2d1b7c0000";
#[cfg(feature = "companion-dev")]
pub const COMPANION_OTA_V2_SERVICE_UUID: &str = OTA_V2_SERVICE_UUID_TEXT;
#[cfg(feature = "companion-dev")]
pub const COMPANION_OTA_V2_STATUS_UUID: &str = OTA_V2_STATUS_UUID_TEXT;
pub const LISTENER_OTA_V2_SERVICE_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092a";
pub const LISTENER_OTA_V2_CONTROL_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092b";
pub const LISTENER_OTA_V2_DATA_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092c";
pub const LISTENER_OTA_V2_STATUS_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092b";
pub const DIAGNOSTIC_EVENT_BYTES: usize = 24;
pub const DIAGNOSTIC_CHUNK_HEADER_BYTES: usize = 8;

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
    pub status_led_brightness_percent: u8,
    pub key_led_brightness_percent: u8,
    pub knob_led_brightness_percent: u8,
    pub edge_led_brightness_percent: u8,
    pub led_zone_brightness_supported: bool,
    pub low_power_idle_minutes: u32,
    pub plugged_low_power_idle_minutes: u32,
    pub battery_low_power_idle_minutes: u32,
    pub plugged_low_power_enabled: bool,
    pub plugged_auto_shutdown_minutes: u32,
    pub battery_auto_shutdown_minutes: u32,
    pub knob_rotation_action: String,
    pub ble_name: String,
    pub ble_name_pending_restart: bool,
    pub external_power_present: bool,
    pub usb_power_present: bool,
    pub charging: bool,
    pub charge_full: bool,
    pub raw_line: String,
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
            aggregate_crc32: Some(format_crc32(crc32(&raw_event_bytes))),
            events_sha256: Some(crate::firmware_ota::sha256_hex(&raw_event_bytes)),
            chunks,
            error: None,
            raw_event_bytes,
        }
    }
}

pub fn classify_ble_failure(error: &str) -> BleFailureClassification {
    let lower = error.to_ascii_lowercase();
    let kind = if lower.contains("only supported on windows") {
        BleFailureKind::UnsupportedPlatform
    } else if lower.contains("ota reboot")
        || lower.contains("after ota")
        || lower.contains("confirm")
        || lower.contains("reboot window")
    {
        BleFailureKind::OtaRebootWindow
    } else if lower.contains("background listener")
        || lower.contains("background capture")
        || lower.contains("already active")
        || lower.contains("foreground probe skipped")
        || lower.contains("cancelled")
        || lower.contains("canceled")
    {
        BleFailureKind::BackgroundListenerContention
    } else if lower.contains("firmware revision")
        || lower.contains("dis firmware")
        || lower.contains("firmware version")
    {
        BleFailureKind::MissingDisFirmwareRevision
    } else if lower.contains("cccd")
        || lower.contains("protocol_error")
        || lower.contains("protocol error")
        || lower.contains("notify write")
        || lower.contains("notify characteristic")
    {
        BleFailureKind::CccdProtocolError
    } else if lower.contains("bluetooth service")
        || lower.contains("radio")
        || lower.contains("adapter")
        || lower.contains("0x8007048f")
        || lower.contains("0x800710df")
        || lower.contains("service reset")
    {
        BleFailureKind::WindowsBluetoothServiceResetNeeded
    } else if lower.contains("access denied") || lower.contains("denied") {
        BleFailureKind::AccessDenied
    } else if ble_error_suggests_missing_pairing(&lower) {
        BleFailureKind::MissingPairing
    } else if ble_error_suggests_device_asleep(&lower) {
        BleFailureKind::DeviceAsleep
    } else if lower.contains("stale")
        || lower.contains("unknown gatt")
        || (lower.contains("cached") && !lower.contains("uncached"))
        || lower.contains("gatt cache")
        || lower.contains("service changed")
    {
        BleFailureKind::StaleGattService
    } else if ble_error_suggests_low_power_idle_disconnect(&lower) {
        BleFailureKind::LowPowerIdleDisconnect
    } else if lower.contains("unreachable")
        || lower.contains("disconnected")
        || (lower.contains("transport_not_ready") && lower.contains("disconnect"))
        || lower.contains("timed out")
        || lower.contains("timeout")
    {
        BleFailureKind::PairedButDisconnected
    } else if lower.contains("not found")
        || lower.contains("no subscribable")
        || lower.contains("no writable listener ble ota")
        || lower.contains("selector returned no")
        || lower.contains("returned no devices")
        || lower.contains("no devices")
        || lower.contains("no paired ble device")
    {
        BleFailureKind::DeviceMissing
    } else {
        BleFailureKind::Unknown
    };

    let (retryable, automatic_recovery, user_action) = match kind {
        BleFailureKind::DeviceMissing => (
            true,
            false,
            "Wake the Listener device, confirm it is paired, then retry or re-pair.",
        ),
        BleFailureKind::DeviceAsleep => (
            true,
            false,
            "Press KEY4 or the wake key, wait for the device to return online, then retry.",
        ),
        BleFailureKind::MissingPairing => (
            true,
            false,
            "Pair the Listener device in Windows Bluetooth, then return and refresh Listener BLE.",
        ),
        BleFailureKind::LowPowerIdleDisconnect => (
            true,
            true,
            "Listener BLE entered offline state; retrying will reconnect, or press KEY4 if it is offline.",
        ),
        BleFailureKind::PairedButDisconnected => (
            true,
            true,
            "Wait for automatic reconnect; press the wake key if it stays disconnected.",
        ),
        BleFailureKind::StaleGattService => (
            true,
            true,
            "Retry after Listener Type refreshes the GATT path; re-pair if stale services persist.",
        ),
        BleFailureKind::CccdProtocolError => (
            true,
            true,
            "Retry after the notify subscription is reopened; reboot Type if repeated.",
        ),
        BleFailureKind::MissingDisFirmwareRevision => (
            false,
            false,
            "Collect diagnostics and update firmware readiness/DIS exposure before release.",
        ),
        BleFailureKind::BackgroundListenerContention => (
            true,
            true,
            "Pause the competing BLE operation and retry through the shared listener path.",
        ),
        BleFailureKind::OtaRebootWindow => (
            true,
            true,
            "Wait for the OTA reboot window to finish, then refresh device status.",
        ),
        BleFailureKind::WindowsBluetoothServiceResetNeeded => (
            true,
            false,
            "Toggle Windows Bluetooth or restart the Bluetooth Support Service, then retry.",
        ),
        BleFailureKind::AccessDenied => (
            false,
            false,
            "Allow Bluetooth/device access in Windows settings or re-pair the device.",
        ),
        BleFailureKind::UnsupportedPlatform => (
            false,
            false,
            "Use the supported Windows BLE path for this diagnostic.",
        ),
        BleFailureKind::Unknown => (
            true,
            false,
            "Export diagnostics and retry after restarting Listener Type.",
        ),
    };

    BleFailureClassification {
        kind,
        retryable,
        automatic_recovery,
        user_action,
        evidence: error.chars().take(480).collect(),
    }
}

fn ble_error_suggests_missing_pairing(lower: &str) -> bool {
    lower.contains("no paired ble device")
        || lower.contains("no paired listener")
        || lower.contains("not paired")
        || lower.contains("missing pairing")
        || lower.contains("pairing missing")
        || lower.contains("pair the listener")
}

fn ble_error_suggests_device_asleep(lower: &str) -> bool {
    lower.contains("deep sleep")
        || lower.contains("asleep")
        || lower.contains("sleeping")
        || lower.contains("wake key")
        || lower.contains("press key4")
        || lower.contains("key4")
}

fn ble_error_suggests_low_power_idle_disconnect(lower: &str) -> bool {
    let reason_546 = lower.contains("reason=546")
        || lower.contains("reason: 546")
        || lower.contains("reason 546")
        || lower.contains("reason=0x222")
        || lower.contains("reason: 0x222");
    let idle_label = lower.contains("low-power idle")
        || lower.contains("low power idle")
        || lower.contains("idle disconnect")
        || lower.contains("idle-disconnect")
        || lower.contains("intentional idle");
    let transport_not_ready =
        lower.contains("transport_not_ready") || lower.contains("transport not ready");

    reason_546 || idle_label || (transport_not_ready && lower.contains("low power"))
}

fn utc_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn format_bluetooth_address(address: u64) -> String {
    format!("{address:012X}")
}

fn format_crc32(value: u32) -> String {
    format!("0x{value:08x}")
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
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
mod windows_ble {
    use std::fmt;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};
    use std::time::{Duration, Instant};

    use serde::Deserialize;
    use serialport::{SerialPortInfo, SerialPortType};
    use windows::core::{IInspectable, GUID, HSTRING, PCWSTR};
    use windows::Devices::Bluetooth::Advertisement::{
        BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
        BluetoothLEScanningMode,
    };
    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCharacteristic, GattCharacteristicProperties,
        GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
        GattDeviceService, GattSession, GattSessionStatus, GattSessionStatusChangedEventArgs,
        GattValueChangedEventArgs, GattWriteOption, GattWriteResult,
    };
    use windows::Devices::Bluetooth::{
        BluetoothAddressType, BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice,
    };
    use windows::Devices::Enumeration::{
        DeviceAccessStatus, DeviceClass, DeviceInformation, DeviceInformationCustomPairing,
        DeviceInformationPairing, DevicePairingKinds, DevicePairingProtectionLevel,
        DevicePairingRequestedEventArgs, DevicePairingResultStatus, DeviceUnpairingResultStatus,
    };
    use windows::Foundation::{
        AsyncStatus, EventRegistrationToken, IAsyncOperation, TypedEventHandler,
    };
    use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Locate_DevNodeW, CM_Query_And_Remove_SubTreeW, CM_LOCATE_DEVNODE_NORMAL,
        CM_REMOVE_NO_RESTART, CM_REMOVE_UI_NOT_OK, CONFIGRET, CR_ACCESS_DENIED, CR_NO_SUCH_DEVINST,
        CR_NO_SUCH_DEVNODE, CR_QUERY_VETOED, CR_REMOVE_VETOED, CR_SUCCESS, PNP_VETO_TYPE,
    };
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const SERVICE_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091a);
    const NOTIFY_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091b);
    const AUDIO_CONTROL_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091e);
    const OTA_SERVICE_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3092a);
    const OTA_CONTROL_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3092b);
    const OTA_DATA_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3092c);
    const LISTENER_OTA_V2_SERVICE_UUID: GUID = OTA_SERVICE_UUID;
    const LISTENER_OTA_V2_CONTROL_UUID: GUID = OTA_CONTROL_UUID;
    const LISTENER_OTA_V2_DATA_UUID: GUID = OTA_DATA_UUID;
    const LISTENER_OTA_V2_STATUS_UUID: GUID = OTA_CONTROL_UUID;
    const OTA_READINESS_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091c);
    const OTA_CAPABILITIES_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091d);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_SERVICE_UUID: GUID =
        GUID::from_u128(0x8f7a0007_7b7d_4f3d_9d6f_6c2d1b7c0000);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_BASE_UUID: GUID = GUID::from_u128(0x8f7a7002_7b7d_4f3d_9d6f_6c2d1b7c0000);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_CONFIRM_UUID: GUID =
        GUID::from_u128(0x8f7a7003_7b7d_4f3d_9d6f_6c2d1b7c0000);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_RAW_UUID: GUID = GUID::from_u128(0x8f7a7004_7b7d_4f3d_9d6f_6c2d1b7c0000);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_LEGACY_OTA_SERVICE_UUID: GUID =
        GUID::from_u128(0x0000fe20_cc7a_482a_984a_7f2ed5b3e58f);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_LEGACY_OTA_BASE_UUID: GUID =
        GUID::from_u128(0x0000fe22_8e22_4541_9d4c_21edae82ed19);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_LEGACY_OTA_CONFIRM_UUID: GUID =
        GUID::from_u128(0x0000fe23_8e22_4541_9d4c_21edae82ed19);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_LEGACY_OTA_RAW_UUID: GUID =
        GUID::from_u128(0x0000fe24_8e22_4541_9d4c_21edae82ed19);
    #[cfg(feature = "companion-dev")]
    const OTA_V2_SERVICE_UUID: GUID = GUID::from_u128(0x8f7a8007_7b7d_4f3d_9d6f_6c2d1b7c0000);
    #[cfg(feature = "companion-dev")]
    const OTA_V2_CONTROL_UUID: GUID = GUID::from_u128(0x8f7a8002_7b7d_4f3d_9d6f_6c2d1b7c0000);
    #[cfg(feature = "companion-dev")]
    const OTA_V2_DATA_UUID: GUID = GUID::from_u128(0x8f7a8004_7b7d_4f3d_9d6f_6c2d1b7c0000);
    #[cfg(feature = "companion-dev")]
    const OTA_V2_STATUS_UUID: GUID = GUID::from_u128(0x8f7a8005_7b7d_4f3d_9d6f_6c2d1b7c0000);

    #[cfg(feature = "companion-dev")]
    #[derive(Clone, Copy)]
    struct Stm32wbStOtaUuidSet {
        label: &'static str,
        service: GUID,
        base: GUID,
        confirm: GUID,
        raw: GUID,
    }

    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_UUID_SETS: [Stm32wbStOtaUuidSet; 2] = [
        Stm32wbStOtaUuidSet {
            label: "companion",
            service: STM32WB_ST_OTA_SERVICE_UUID,
            base: STM32WB_ST_OTA_BASE_UUID,
            confirm: STM32WB_ST_OTA_CONFIRM_UUID,
            raw: STM32WB_ST_OTA_RAW_UUID,
        },
        Stm32wbStOtaUuidSet {
            label: "legacy-st",
            service: STM32WB_ST_LEGACY_OTA_SERVICE_UUID,
            base: STM32WB_ST_LEGACY_OTA_BASE_UUID,
            confirm: STM32WB_ST_LEGACY_OTA_CONFIRM_UUID,
            raw: STM32WB_ST_LEGACY_OTA_RAW_UUID,
        },
    ];
    const DIAGNOSTIC_SERVICE_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3093a);
    const DIAGNOSTIC_CONTROL_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3093b);
    const DIAGNOSTIC_DATA_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3093c);
    const DIAGNOSTIC_COUNT_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3093d);
    const DIS_SERVICE_UUID: GUID = GUID::from_u128(0x0000180a_0000_1000_8000_00805f9b34fb);
    const DIS_MODEL_NUMBER_UUID: GUID = GUID::from_u128(0x00002a24_0000_1000_8000_00805f9b34fb);
    const DIS_FIRMWARE_REVISION_UUID: GUID =
        GUID::from_u128(0x00002a26_0000_1000_8000_00805f9b34fb);
    const DIS_HARDWARE_REVISION_UUID: GUID =
        GUID::from_u128(0x00002a27_0000_1000_8000_00805f9b34fb);
    const BATTERY_SERVICE_UUID: GUID = GUID::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb);
    const BATTERY_LEVEL_UUID: GUID = GUID::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);
    const RECONNECT_COOLDOWN: Duration = Duration::from_millis(350);
    const RECEIVE_POLL_INTERVAL: Duration = Duration::from_millis(100);
    const TYPE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);
    const TYPE_HEARTBEAT_WRITE_TIMEOUT: Duration = Duration::from_millis(3000);
    const CAPTURE_NOTIFICATION_INFO_LOG_LIMIT: usize = 4;
    const GATT_READY_TIMEOUT: Duration = Duration::from_secs(8);
    const GATT_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
    const CCCD_ENABLE_TIMEOUT: Duration = Duration::from_secs(8);
    const CCCD_ENABLE_RETRY_DELAYS: [Duration; 3] = [
        Duration::from_millis(250),
        Duration::from_millis(750),
        Duration::from_millis(1500),
    ];
    const NOTIFY_TARGET_OPEN_RETRY_DELAYS: [Duration; 7] = [
        Duration::from_millis(250),
        Duration::from_millis(500),
        Duration::from_millis(1000),
        Duration::from_millis(2000),
        Duration::from_millis(3000),
        Duration::from_millis(5000),
        Duration::from_millis(8000),
    ];
    const AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS: [Duration; 4] = [
        Duration::from_millis(150),
        Duration::from_millis(350),
        Duration::from_millis(750),
        Duration::from_millis(1500),
    ];
    const ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);
    const DIAGNOSTIC_PULL_CANDIDATE_DELAY: Duration = Duration::from_millis(350);
    const OTA_WRITE_TIMEOUT: Duration = Duration::from_secs(8);
    const OTA_FINISH_WRITE_TIMEOUT: Duration = Duration::from_secs(45);
    const OTA_DATA_WRITE_OPTION_ENV: &str = "LISTENER_OTA_DATA_WRITE_OPTION";
    const TYPE_READY_COMMAND_ENV: &str = "LISTENER_TYPE_EMBEDDED_BLE_READY_COMMAND";
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_ADVERTISEMENT_NAME: &str = "companion";
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_ADVERTISEMENT_SCAN_TIMEOUT: Duration = Duration::from_secs(12);
    const OTA_DATA_CHUNK_BYTES_ENV: &str = "LISTENER_OTA_DATA_CHUNK_BYTES";
    const OTA_DATA_INTER_CHUNK_DELAY_MS_ENV: &str = "LISTENER_OTA_DATA_INTER_CHUNK_DELAY_MS";
    const AUDIO_ADVERTISEMENT_SCAN_TIMEOUT: Duration = Duration::from_secs(12);
    const BLE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
    const BLE_PAIRING_PROMPT_TIMEOUT: Duration = Duration::from_secs(45);
    const BLE_PAIRING_PROMPT_SUPPRESS_WINDOW: Duration = Duration::from_secs(60 * 60);
    const WINDOWS_CREATE_NO_WINDOW: u32 = 0x08000000;
    const DEVICE_SETTINGS_SERIAL_BAUD_RATE: u32 = 115_200;
    const DEVICE_SETTINGS_SERIAL_READ_CHUNK_BYTES: usize = 256;
    const DEFAULT_BLUETOOTH_TARGET_NAME: &str = "listener";
    const BTHPORT_DEVICE_CACHE_REGISTRY_PATH: &str =
        r"SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices";
    const ATT_WRITE_HEADER_BYTES: usize = 3;
    const ATT_DEFAULT_PAYLOAD_BYTES: usize = 20;
    const SERVICE_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3091a";
    const OTA_SERVICE_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092a";
    const LISTENER_SERVICE_UUID_TEXTS: [&str; 3] = [
        SERVICE_UUID_TEXT,
        OTA_SERVICE_UUID_TEXT,
        crate::embedded_ble::DIAGNOSTIC_SERVICE_UUID_TEXT,
    ];
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_APP_BASE_ADDRESS: u32 = 0x0800_7000;
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_RAW_DATA_SIZE: usize = 248;
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_CONFIRM_TIMEOUT: Duration = Duration::from_secs(45);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_REBOOT_HANDOFF_TIMEOUT: Duration = Duration::from_secs(12);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_NO_RESPONSE_CHUNK_DELAY: Duration = Duration::from_millis(50);
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_APPLICATION_UPLOAD: u8 = 0x02;
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_UPLOAD_FINISHED: u8 = 0x07;
    #[cfg(feature = "companion-dev")]
    const STM32WB_ST_OTA_REBOOT_CONFIRMED: u8 = 0x01;
    #[cfg(feature = "companion-dev")]
    const OTA_V2_MAGIC: &[u8; 4] = b"COV2";
    const LISTENER_OTA_V2_MAGIC: &[u8; 4] = b"LOV2";
    const OTA_V2_PROTOCOL_VERSION: u8 = 1;
    const OTA_V2_OP_BEGIN: u8 = 1;
    const OTA_V2_OP_SYNC: u8 = 2;
    const OTA_V2_OP_FINISH: u8 = 3;
    const OTA_V2_OP_ABORT: u8 = 4;
    const OTA_V2_STATUS_BYTES: usize = 24;
    const OTA_V2_PACKET_HEADER_BYTES: usize = 4;
    const OTA_V2_DATA_PACKET_BYTES: usize = 244;
    #[cfg(feature = "companion-dev")]
    const OTA_V2_CHUNK_PAYLOAD_BYTES: usize = OTA_V2_DATA_PACKET_BYTES - OTA_V2_PACKET_HEADER_BYTES;
    #[cfg(feature = "companion-dev")]
    const OTA_V2_DEFAULT_WINDOW_CHUNKS: usize = 24;
    #[cfg(feature = "companion-dev")]
    const COMPANION_OTA_V2_WINDOW_ENV: &str = "LISTENER_COMPANION_DEV_OTA_V2_WINDOW_CHUNKS";
    #[cfg(feature = "companion-dev")]
    const OTA_V2_WINDOW_ENV: &str = COMPANION_OTA_V2_WINDOW_ENV;
    const LISTENER_OTA_V2_CHUNK_PAYLOAD_BYTES: usize = 500;
    const LISTENER_OTA_V2_DEFAULT_WINDOW_CHUNKS: usize = 14;
    const LISTENER_OTA_V2_WINDOW_ENV: &str = "LISTENER_OTA_V2_WINDOW_CHUNKS";
    const OTA_V2_STATUS_POLL_TIMEOUT: Duration = Duration::from_secs(15);
    const OTA_V2_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(150);
    const OTA_V2_STATE_IDLE: u8 = 0;
    const OTA_V2_STATE_ERASING: u8 = 1;
    const OTA_V2_STATE_RECEIVING: u8 = 2;
    const OTA_V2_STATE_COMPLETE: u8 = 3;
    const OTA_V2_STATE_ERROR: u8 = 4;
    const OTA_V2_ERROR_NONE: u8 = 0;
    const OTA_V2_ERROR_OFFSET_MISMATCH: u8 = 4;
    #[cfg(feature = "companion-dev")]
    const COMPANION_OTA_V2_ERROR_OFFSET_MISMATCH: u8 = OTA_V2_ERROR_OFFSET_MISMATCH;
    const DIS_SERVICE_UUID_TEXT: &str = "0000180a-0000-1000-8000-00805f9b34fb";
    const OTA_REQUIRED_DATA_CHUNK_BYTES: usize = 500;
    static RUNTIME_BLUETOOTH_TARGET_NAME: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    static LAST_PAIRING_PROMPT: OnceLock<Mutex<Option<PairingPromptThrottleState>>> =
        OnceLock::new();

    enum BleCaptureSignal {
        Notification(Vec<u8>),
        Disconnected(String),
        AudioControl(AudioControlRequest),
    }

    struct AudioControlRequest {
        bytes: Vec<u8>,
        label: String,
        timeout: Duration,
        result_tx: mpsc::Sender<Result<(), String>>,
    }

    #[derive(Clone)]
    struct ActiveAudioControlSender {
        capture_id: u64,
        tx: mpsc::Sender<BleCaptureSignal>,
    }

    fn active_audio_control_slot() -> &'static Mutex<Option<ActiveAudioControlSender>> {
        static SLOT: OnceLock<Mutex<Option<ActiveAudioControlSender>>> = OnceLock::new();
        SLOT.get_or_init(|| Mutex::new(None))
    }

    fn active_audio_control_sender() -> Option<ActiveAudioControlSender> {
        active_audio_control_slot().lock().ok()?.clone()
    }

    fn clear_active_audio_control_sender(capture_id: u64) {
        let Ok(mut slot) = active_audio_control_slot().lock() else {
            return;
        };
        if slot
            .as_ref()
            .is_some_and(|active| active.capture_id == capture_id)
        {
            *slot = None;
            log::info!("[embedded-ble] capture #{capture_id}: audio control sender cleared");
        }
    }

    struct ActiveAudioControlRegistration {
        capture_id: u64,
    }

    impl ActiveAudioControlRegistration {
        fn install(capture_id: u64, tx: mpsc::Sender<BleCaptureSignal>) -> Self {
            if let Ok(mut slot) = active_audio_control_slot().lock() {
                *slot = Some(ActiveAudioControlSender { capture_id, tx });
                log::info!("[embedded-ble] capture #{capture_id}: audio control sender registered");
            }
            Self { capture_id }
        }
    }

    impl Drop for ActiveAudioControlRegistration {
        fn drop(&mut self) {
            clear_active_audio_control_sender(self.capture_id);
        }
    }

    #[derive(Default)]
    struct BleFreshGattGateState {
        last_closed_at: Option<Instant>,
    }

    fn fresh_gatt_gate() -> &'static Mutex<BleFreshGattGateState> {
        static GATE: OnceLock<Mutex<BleFreshGattGateState>> = OnceLock::new();
        GATE.get_or_init(|| Mutex::new(BleFreshGattGateState::default()))
    }

    struct BleFreshGattGuard {
        guard: MutexGuard<'static, BleFreshGattGateState>,
        label: &'static str,
        started_at: Instant,
    }

    impl BleFreshGattGuard {
        fn enter(label: &'static str) -> Result<Self, String> {
            let guard = fresh_gatt_gate()
                .lock()
                .map_err(|_| "BLE fresh GATT gate is poisoned".to_string())?;
            if let Some(last_closed_at) = guard.last_closed_at {
                let elapsed = last_closed_at.elapsed();
                if elapsed < RECONNECT_COOLDOWN {
                    let delay = RECONNECT_COOLDOWN - elapsed;
                    log::info!(
                        "[embedded-ble] {label}: waiting {} ms before fresh GATT operation",
                        delay.as_millis()
                    );
                    std::thread::sleep(delay);
                }
            }
            log::info!("[embedded-ble] {label}: entered fresh GATT operation gate");
            Ok(Self {
                guard,
                label,
                started_at: Instant::now(),
            })
        }
    }

    impl Drop for BleFreshGattGuard {
        fn drop(&mut self) {
            self.guard.last_closed_at = Some(Instant::now());
            log::info!(
                "[embedded-ble] {}: released fresh GATT operation gate after {} ms",
                self.label,
                self.started_at.elapsed().as_millis()
            );
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum CaptureTerminalBehavior {
        StopCapture,
        ContinueListening,
    }

    pub fn capture_notifications_once(timeout: Duration) -> Result<Vec<Vec<u8>>, String> {
        let mut notifications = Vec::new();
        capture_notification_events(timeout, &mut |event| {
            notifications.push(event.notification);
            Ok(())
        })?;
        Ok(notifications)
    }

    pub(super) fn diagnostic_snapshot() -> crate::embedded_ble::BleDiagnosticSnapshot {
        let mut errors = Vec::new();
        let audio_services = match diagnostic_service_entries(
            "audio",
            SERVICE_UUID_TEXT,
            SERVICE_UUID,
            "audio service discovery",
        ) {
            Ok(entries) => entries,
            Err(err) => {
                errors.push(err);
                Vec::new()
            }
        };
        let ota_services = match diagnostic_service_entries(
            "ota",
            OTA_SERVICE_UUID_TEXT,
            OTA_SERVICE_UUID,
            "OTA service discovery",
        ) {
            Ok(entries) => entries,
            Err(err) => {
                errors.push(err);
                Vec::new()
            }
        };
        let diagnostic_services = match diagnostic_service_entries(
            "diagnostic",
            crate::embedded_ble::DIAGNOSTIC_SERVICE_UUID_TEXT,
            DIAGNOSTIC_SERVICE_UUID,
            "diagnostic log service discovery",
        ) {
            Ok(entries) => entries,
            Err(err) => {
                errors.push(err);
                Vec::new()
            }
        };
        let firmware_snapshot = firmware_ota_device_snapshot();
        if let Some(detail) = firmware_snapshot.detail.as_ref() {
            errors.push(detail.clone());
        }

        crate::embedded_ble::BleDiagnosticSnapshot {
            captured_at: crate::embedded_ble::utc_now_rfc3339(),
            platform: "windows",
            audio_service_uuid: SERVICE_UUID_TEXT,
            ota_service_uuid: OTA_SERVICE_UUID_TEXT,
            diagnostic_service_uuid: crate::embedded_ble::DIAGNOSTIC_SERVICE_UUID_TEXT,
            dis_service_uuid: DIS_SERVICE_UUID_TEXT,
            configured_device_address: configured_bluetooth_address()
                .map(crate::embedded_ble::format_bluetooth_address),
            audio_services,
            ota_services,
            diagnostic_services,
            firmware_snapshot,
            errors,
        }
    }

    fn diagnostic_service_entries(
        selector_name: &'static str,
        service_uuid_text: &'static str,
        service_uuid: GUID,
        label: &str,
    ) -> Result<Vec<crate::embedded_ble::BleDiagnosticServiceEntry>, String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(service_uuid)
            .map_err(|err| format!("BLE diagnostic {label} selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE diagnostic {label} query failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, label))?;
        let count = devices
            .Size()
            .map_err(|err| format!("BLE diagnostic {label} collection size failed: {err}"))?;
        let mut entries = Vec::new();
        for index in 0..count {
            let info = devices.GetAt(index).map_err(|err| {
                format!("BLE diagnostic {label} entry {index} read failed: {err}")
            })?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            entries.push(crate::embedded_ble::BleDiagnosticServiceEntry {
                selector: selector_name,
                service_uuid: service_uuid_text,
                index,
                name,
                bluetooth_address: parse_bluetooth_address_from_device_id(&id)
                    .map(crate::embedded_ble::format_bluetooth_address),
                id,
            });
        }
        Ok(entries)
    }

    pub fn unpair_listener_devices() -> crate::embedded_ble::BleDeviceUnpairResult {
        unpair_listener_devices_for_names(&[])
    }

    pub fn unpair_listener_devices_for_names(
        extra_names: &[String],
    ) -> crate::embedded_ble::BleDeviceUnpairResult {
        match unpair_listener_devices_inner(extra_names) {
            Ok(result) => result,
            Err(err) => {
                log::warn!("[embedded-ble] automatic Listener unpair unavailable: {err}");
                crate::embedded_ble::BleDeviceUnpairResult {
                    status: crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction,
                    attempted: false,
                    matched_devices: 0,
                    unpaired_devices: 0,
                    already_unpaired_devices: 0,
                    failed_devices: 0,
                    needs_user_action: true,
                    details: vec![err],
                }
            }
        }
    }

    fn unpair_listener_devices_inner(
        extra_names: &[String],
    ) -> Result<crate::embedded_ble::BleDeviceUnpairResult, String> {
        let target_names = listener_target_names(extra_names);
        let mut target_addresses = listener_recovery_target_addresses();
        push_listener_recovery_advertised_addresses(&mut target_addresses, &target_names);
        let candidates = listener_unpair_candidates(&target_names, &mut target_addresses)?;
        push_listener_recovery_target_addresses_from_candidates(&mut target_addresses, &candidates);
        let mut result = crate::embedded_ble::BleDeviceUnpairResult {
            status: crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction,
            attempted: true,
            matched_devices: candidates.len() as u32,
            unpaired_devices: 0,
            already_unpaired_devices: 0,
            failed_devices: 0,
            needs_user_action: true,
            details: Vec::new(),
        };

        for candidate in candidates {
            match unpair_listener_candidate(&candidate) {
                Ok(DeviceUnpairOutcome::Unpaired) => {
                    result.unpaired_devices = result.unpaired_devices.saturating_add(1);
                    result.details.push(format!(
                        "Removed stale Listener pairing: {}",
                        candidate.label
                    ));
                }
                Ok(DeviceUnpairOutcome::AlreadyUnpaired) => {
                    result.already_unpaired_devices =
                        result.already_unpaired_devices.saturating_add(1);
                    result.details.push(format!(
                        "Listener pairing was already removed: {}",
                        candidate.label
                    ));
                }
                Err(err) => {
                    result.failed_devices = result.failed_devices.saturating_add(1);
                    result
                        .details
                        .push(format!("Could not remove {}: {err}", candidate.label));
                }
            }
        }

        match listener_pnp_remove_candidates(&target_addresses, &target_names) {
            Ok(pnp_candidates) => {
                result.matched_devices = result
                    .matched_devices
                    .saturating_add(pnp_candidates.len() as u32);
                for candidate in pnp_candidates {
                    if let Some(address) =
                        parse_bluetooth_address_from_device_id(&candidate.instance_id)
                    {
                        push_unique_address(&mut target_addresses, address);
                    }
                    match remove_pnp_device_candidate(&candidate) {
                        Ok(DeviceUnpairOutcome::Unpaired) => {
                            result.unpaired_devices = result.unpaired_devices.saturating_add(1);
                            result.details.push(format!(
                                "Removed stale Listener device node: {}",
                                candidate.label
                            ));
                        }
                        Ok(DeviceUnpairOutcome::AlreadyUnpaired) => {
                            result.already_unpaired_devices =
                                result.already_unpaired_devices.saturating_add(1);
                            result.details.push(format!(
                                "Listener device node was already removed: {}",
                                candidate.label
                            ));
                        }
                        Err(err) => {
                            result.failed_devices = result.failed_devices.saturating_add(1);
                            result.details.push(format!(
                                "Could not remove stale Listener device node {}: {err}",
                                candidate.label
                            ));
                        }
                    }
                }
            }
            Err(err) => {
                log::warn!("[embedded-ble] Listener PnP stale-node cleanup unavailable: {err}");
                result.details.push(format!(
                    "Listener PnP stale-node cleanup unavailable: {err}"
                ));
            }
        }

        match bthport_listener_cache_candidates(&target_addresses, &target_names) {
            Ok(cache_candidates) => {
                result.matched_devices = result
                    .matched_devices
                    .saturating_add(cache_candidates.len() as u32);
                for candidate in cache_candidates {
                    if let Some(address) = candidate.address {
                        push_unique_address(&mut target_addresses, address);
                    }
                    match delete_bthport_cache_candidate(&candidate) {
                        Ok(DeviceUnpairOutcome::Unpaired) => {
                            result.unpaired_devices = result.unpaired_devices.saturating_add(1);
                            result.details.push(format!(
                                "Removed stale Windows Bluetooth cache: {}",
                                candidate.label
                            ));
                        }
                        Ok(DeviceUnpairOutcome::AlreadyUnpaired) => {
                            result.already_unpaired_devices =
                                result.already_unpaired_devices.saturating_add(1);
                            result.details.push(format!(
                                "Windows Bluetooth cache was already removed: {}",
                                candidate.label
                            ));
                        }
                        Err(err) => {
                            result.failed_devices = result.failed_devices.saturating_add(1);
                            result.details.push(format!(
                                "Could not remove Windows Bluetooth cache {}: {err}",
                                candidate.label
                            ));
                        }
                    }
                }
            }
            Err(err) => {
                log::warn!("[embedded-ble] Listener BTHPORT cache cleanup unavailable: {err}");
                result
                    .details
                    .push(format!("Listener BTHPORT cache cleanup unavailable: {err}"));
            }
        }

        if result.matched_devices == 0 {
            return Ok(crate::embedded_ble::BleDeviceUnpairResult {
                status: crate::embedded_ble::BleDeviceUnpairStatus::NotFound,
                attempted: false,
                matched_devices: 0,
                unpaired_devices: 0,
                already_unpaired_devices: 0,
                failed_devices: 0,
                needs_user_action: true,
                details: vec![
                    "No Listener pairing entry was found. Windows Bluetooth will open for manual pairing."
                        .to_string(),
                ],
            });
        }

        result.status = if result.unpaired_devices > 0 && result.failed_devices == 0 {
            crate::embedded_ble::BleDeviceUnpairStatus::Removed
        } else if result.failed_devices == 0
            && result.already_unpaired_devices == result.matched_devices
        {
            crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean
        } else {
            crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction
        };
        result.needs_user_action =
            result.status == crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction;
        Ok(result)
    }

    #[derive(Clone)]
    struct ListenerUnpairCandidate {
        label: String,
        info: DeviceInformation,
    }

    #[derive(Clone)]
    struct ListenerPairingCandidate {
        label: String,
        info: DeviceInformation,
    }

    #[derive(Clone)]
    struct PairingPromptThrottleState {
        attempted_at: Instant,
    }

    #[derive(Clone)]
    struct ListenerPnpRemoveCandidate {
        label: String,
        instance_id: String,
        name: String,
        address: Option<u64>,
        is_ble_device_root: bool,
    }

    #[derive(Clone)]
    pub(super) struct ListenerPnpEntry {
        pub(super) name: String,
        pub(super) instance_id: String,
        pub(super) address: Option<u64>,
        pub(super) has_listener_service_signature: bool,
        pub(super) is_ble_device_root: bool,
    }

    #[derive(Deserialize)]
    struct PowerShellPnpDeviceEntry {
        #[serde(rename = "FriendlyName")]
        friendly_name: Option<String>,
        #[serde(rename = "InstanceId")]
        instance_id: Option<String>,
    }

    #[derive(Clone)]
    struct ListenerBthPortCacheCandidate {
        label: String,
        address_key: String,
        name: String,
        address: Option<u64>,
    }

    enum DeviceUnpairOutcome {
        Unpaired,
        AlreadyUnpaired,
    }

    enum DevicePairingOutcome {
        Paired,
        AlreadyPaired,
    }

    pub fn prompt_listener_pairing(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match prompt_listener_pairing_inner(expected_name) {
            Ok(result) => result,
            Err(err) => {
                log::warn!("[embedded-ble] automatic Listener pairing prompt unavailable: {err}");
                crate::embedded_ble::BleDevicePairingPromptResult {
                    status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
                    attempted: false,
                    matched_devices: 0,
                    prompted_devices: 0,
                    already_paired_devices: 0,
                    failed_devices: 0,
                    open_bluetooth_settings: true,
                    details: vec![err],
                }
            }
        }
    }

    fn prompt_listener_pairing_inner(
        expected_name: Option<&str>,
    ) -> Result<crate::embedded_ble::BleDevicePairingPromptResult, String> {
        let target_name = effective_bluetooth_target_name(expected_name);
        let now = Instant::now();
        if let Some(remaining) = pairing_prompt_suppression_remaining(&target_name, now) {
            log::info!(
                "[embedded-ble] suppressing repeated Windows pairing prompt target={target_name:?} remaining_ms={}",
                remaining.as_millis()
            );
            return Ok(suppressed_pairing_prompt_result(&target_name, remaining));
        }
        remember_pairing_prompt_attempt(&target_name, now);

        let candidates = listener_pairing_candidates(Some(&target_name))?;
        let mut result = crate::embedded_ble::BleDevicePairingPromptResult {
            status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
            attempted: true,
            matched_devices: candidates.len() as u32,
            prompted_devices: 0,
            already_paired_devices: 0,
            failed_devices: 0,
            open_bluetooth_settings: true,
            details: Vec::new(),
        };

        for candidate in candidates {
            match pair_listener_candidate(&candidate, Some(&target_name)) {
                Ok(DevicePairingOutcome::Paired) => {
                    result.prompted_devices = result.prompted_devices.saturating_add(1);
                    result
                        .details
                        .push(format!("Windows pairing completed for {}", candidate.label));
                }
                Ok(DevicePairingOutcome::AlreadyPaired) => {
                    result.already_paired_devices = result.already_paired_devices.saturating_add(1);
                    result
                        .details
                        .push(format!("Listener was already paired: {}", candidate.label));
                }
                Err(err) => {
                    result.failed_devices = result.failed_devices.saturating_add(1);
                    result.details.push(format!(
                        "Could not start Windows pairing for {}: {err}",
                        candidate.label
                    ));
                }
            }
        }

        if result.matched_devices == 0 {
            result.status = crate::embedded_ble::BleDevicePairingPromptStatus::NotFound;
            result.details.push(
                "No pairable Listener device object was visible to Windows. Bluetooth settings will open for native pairing."
                    .to_string(),
            );
            return Ok(result);
        }

        result.status = if result.prompted_devices > 0 && result.failed_devices == 0 {
            result.open_bluetooth_settings = false;
            crate::embedded_ble::BleDevicePairingPromptStatus::Paired
        } else if result.failed_devices == 0
            && result.already_paired_devices == result.matched_devices
        {
            result.open_bluetooth_settings = false;
            crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
        } else {
            result.open_bluetooth_settings = true;
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
        };
        Ok(result)
    }

    fn pairing_prompt_suppression_remaining(_target_name: &str, now: Instant) -> Option<Duration> {
        let prompt_lock = LAST_PAIRING_PROMPT.get_or_init(|| Mutex::new(None));
        let guard = prompt_lock.lock().ok()?;
        let last = guard.as_ref()?;
        let elapsed = now.saturating_duration_since(last.attempted_at);
        if elapsed >= BLE_PAIRING_PROMPT_SUPPRESS_WINDOW {
            return None;
        }
        Some(BLE_PAIRING_PROMPT_SUPPRESS_WINDOW - elapsed)
    }

    fn remember_pairing_prompt_attempt(_target_name: &str, now: Instant) {
        let prompt_lock = LAST_PAIRING_PROMPT.get_or_init(|| Mutex::new(None));
        if let Ok(mut guard) = prompt_lock.lock() {
            *guard = Some(PairingPromptThrottleState { attempted_at: now });
        }
    }

    fn suppressed_pairing_prompt_result(
        target_name: &str,
        remaining: Duration,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        crate::embedded_ble::BleDevicePairingPromptResult {
            status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
            attempted: false,
            matched_devices: 0,
            prompted_devices: 0,
            already_paired_devices: 0,
            failed_devices: 0,
            open_bluetooth_settings: false,
            details: vec![format!(
                "Windows pairing prompt for {target_name} was already requested recently; waiting {} ms before trying again.",
                remaining.as_millis()
            )],
        }
    }

    fn listener_pairing_candidates(
        expected_name: Option<&str>,
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(false)
            .map_err(|err| format!("unpaired BLE device selector failed: {err}"))?;
        let expected_name = expected_name
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let target_addresses = listener_recovery_target_addresses();
        let mut candidates = Vec::new();
        let mut seen_ids = Vec::new();
        let query_result = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("unpaired BLE device query failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "unpaired BLE device query")
            });
        match query_result {
            Ok(devices) => {
                let count = devices
                    .Size()
                    .map_err(|err| format!("unpaired BLE device collection size failed: {err}"))?;
                for index in 0..count {
                    let info = devices.GetAt(index).map_err(|err| {
                        format!("unpaired BLE device entry {index} read failed: {err}")
                    })?;
                    push_listener_pairing_candidate_if_matching(
                        &mut candidates,
                        &mut seen_ids,
                        info,
                        expected_name,
                        &target_addresses,
                    );
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] unpaired BLE device query failed before pairing prompt; trying advertisement fallback: {err}"
                );
            }
        }

        if candidates.is_empty() {
            match push_listener_pairing_advertisement_candidates(
                &mut candidates,
                &mut seen_ids,
                expected_name,
            ) {
                Ok(()) => {}
                Err(err) => {
                    log::warn!("[embedded-ble] pairing advertisement fallback failed: {err}");
                }
            }
        }

        Ok(candidates)
    }

    fn push_listener_pairing_candidate_if_matching(
        candidates: &mut Vec<ListenerPairingCandidate>,
        seen_ids: &mut Vec<String>,
        info: DeviceInformation,
        expected_name: Option<&str>,
        target_addresses: &[u64],
    ) {
        let name = info
            .Name()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let id = info
            .Id()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let address = parse_bluetooth_address_from_device_id(&id);
        let address_matches = address.is_some_and(|value| target_addresses.contains(&value));
        if !address_matches && !listener_pairing_name_matches(&name, expected_name) {
            return;
        }
        if id.is_empty() || seen_ids.iter().any(|seen| seen == &id) {
            return;
        }
        seen_ids.push(id);
        let label = listener_pairing_candidate_label(&name, address);
        candidates.push(ListenerPairingCandidate { label, info });
    }

    fn push_listener_pairing_advertisement_candidates(
        candidates: &mut Vec<ListenerPairingCandidate>,
        seen_ids: &mut Vec<String>,
        expected_name: Option<&str>,
    ) -> Result<(), String> {
        let addresses = scan_listener_pairing_advertisements(expected_name)?;
        for (address, address_type, name) in addresses {
            let Some(info) = pairing_device_information_from_bluetooth_address(
                address,
                Some(address_type),
                "advertised BLE pairing device query",
            )?
            else {
                log::warn!(
                    "[embedded-ble] advertised pairing candidate {} had no DeviceInformation entry yet",
                    crate::embedded_ble::format_bluetooth_address(address)
                );
                continue;
            };
            let id = info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if id.is_empty() || seen_ids.iter().any(|seen| seen == &id) {
                continue;
            }
            seen_ids.push(id);
            let label = listener_pairing_candidate_label(&name, Some(address));
            candidates.push(ListenerPairingCandidate { label, info });
        }
        Ok(())
    }

    fn pairing_device_information_from_bluetooth_address(
        address: u64,
        address_type: Option<BluetoothAddressType>,
        label: &str,
    ) -> Result<Option<DeviceInformation>, String> {
        let address_text = crate::embedded_ble::format_bluetooth_address(address);
        let selector = match address_type.filter(|kind| *kind != BluetoothAddressType::Unspecified)
        {
            Some(kind) => {
                BluetoothLEDevice::GetDeviceSelectorFromBluetoothAddressWithBluetoothAddressType(
                    address, kind,
                )
                .map_err(|err| {
                    format!(
                        "build typed BLE selector for {address_text} type={kind:?} failed: {err}"
                    )
                })?
            }
            None => BluetoothLEDevice::GetDeviceSelectorFromBluetoothAddress(address)
                .map_err(|err| format!("build BLE selector for {address_text} failed: {err}"))?,
        };
        let operation = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("{label} for {address_text} failed to start: {err}"))?;
        match wait_async_operation(operation, BLE_DISCOVERY_TIMEOUT, label) {
            Ok(devices) => {
                let count = devices
                    .Size()
                    .map_err(|err| format!("{label} collection size failed: {err}"))?;
                for index in 0..count {
                    let info = devices
                        .GetAt(index)
                        .map_err(|err| format!("{label} entry {index} read failed: {err}"))?;
                    let id = info
                        .Id()
                        .map(|value| value.to_string_lossy())
                        .unwrap_or_default();
                    if parse_bluetooth_address_from_device_id(&id) == Some(address) {
                        return Ok(Some(info));
                    }
                    if count == 1 {
                        return Ok(Some(info));
                    }
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] BLE pairing selector query for {address_text} failed: {err}; leaving pairing to Windows Bluetooth settings"
                );
            }
        }

        log::warn!(
            "[embedded-ble] BLE pairing selector had no pairable DeviceInformation for {address_text}; opening Bluetooth settings instead of creating a transient GATT connection"
        );
        Ok(None)
    }

    fn listener_pairing_candidate_label(name: &str, address: Option<u64>) -> String {
        match (name.trim().is_empty(), address) {
            (false, Some(address)) => format!(
                "{} ({})",
                name,
                crate::embedded_ble::format_bluetooth_address(address)
            ),
            (false, None) => name.to_string(),
            (true, Some(address)) => crate::embedded_ble::format_bluetooth_address(address),
            (true, None) => "unpaired Listener BLE device".to_string(),
        }
    }

    fn listener_pairing_name_matches(name: &str, expected_name: Option<&str>) -> bool {
        let expected_name = effective_bluetooth_target_name(expected_name);
        bluetooth_name_matches_expected(name, &expected_name)
    }

    fn pair_listener_candidate(
        candidate: &ListenerPairingCandidate,
        expected_name: Option<&str>,
    ) -> Result<DevicePairingOutcome, String> {
        let mut candidate = candidate.clone();
        for stale_cleanup_attempt in 0..3 {
            let pairing = candidate
                .info
                .Pairing()
                .map_err(|err| format!("read pairing info failed: {err}"))?;
            if pairing
                .IsPaired()
                .map_err(|err| format!("read pairing state failed: {err}"))?
            {
                if stale_cleanup_attempt >= 2 {
                    return Err(format!(
                        "Windows still reports {} as paired after stale pairing cleanup",
                        candidate.label
                    ));
                }
                log::warn!(
                    "[embedded-ble] Windows reports {} is already paired during explicit pairing prompt; unpairing stale address cache before pairing",
                    candidate.label
                );
                match unpair_device_information_pairing(&pairing, &candidate.label)? {
                    DeviceUnpairOutcome::Unpaired => {}
                    DeviceUnpairOutcome::AlreadyUnpaired => {}
                }
                let refresh_delay = if stale_cleanup_attempt == 0 {
                    Duration::from_millis(1500)
                } else {
                    Duration::from_millis(3000)
                };
                std::thread::sleep(refresh_delay);
                match refresh_listener_pairing_candidate(&candidate, expected_name)? {
                    Some(refreshed) => {
                        log::info!(
                            "[embedded-ble] refreshed Windows pairing candidate after stale unpair: {} -> {}",
                            candidate.label,
                            refreshed.label
                        );
                        candidate = refreshed;
                        continue;
                    }
                    None => {
                        return Err(format!(
                            "Windows removed stale pairing for {}, but a fresh pairable advertisement is not visible yet",
                            candidate.label
                        ));
                    }
                }
            }

            return pair_unpaired_listener_candidate(&candidate, &pairing, expected_name);
        }

        Err(
            "Windows stale pairing cleanup exhausted without a pairable Listener candidate"
                .to_string(),
        )
    }

    fn pair_unpaired_listener_candidate(
        candidate: &ListenerPairingCandidate,
        pairing: &DeviceInformationPairing,
        expected_name: Option<&str>,
    ) -> Result<DevicePairingOutcome, String> {
        if !pairing
            .CanPair()
            .map_err(|err| format!("read pairing capability failed: {err}"))?
        {
            return pairing_status_or_reachable(
                DevicePairingResultStatus::NotReadyToPair,
                expected_name,
                "Windows reports the device is not ready to pair",
            );
        }

        match custom_pair_listener_candidate(&pairing, &candidate.label, expected_name) {
            Ok(outcome) => return Ok(outcome),
            Err(err) => {
                if pairing_error_should_skip_standard_fallback(&err) {
                    return Err(format!(
                        "Windows custom pairing did not complete for {}: {err}",
                        candidate.label
                    ));
                }
                log::warn!(
                    "[embedded-ble] Windows custom pairing path failed for {}: {err}; falling back to standard PairAsync",
                    candidate.label
                );
            }
        }

        let operation = pairing
            .PairWithProtectionLevelAsync(DevicePairingProtectionLevel::Default)
            .map_err(|err| format!("Windows encrypted pairing operation failed to start: {err}"))?;
        let pair = wait_async_operation(operation, BLE_PAIRING_PROMPT_TIMEOUT, "device pair")?;
        let status = pair
            .Status()
            .map_err(|err| format!("Windows pairing status read failed: {err}"))?;
        pairing_status_or_reachable(
            status,
            expected_name,
            &format!("Windows returned pairing status={status:?}"),
        )
    }

    fn custom_pair_listener_candidate(
        pairing: &windows::Devices::Enumeration::DeviceInformationPairing,
        candidate_label: &str,
        expected_name: Option<&str>,
    ) -> Result<DevicePairingOutcome, String> {
        let custom = pairing
            .Custom()
            .map_err(|err| format!("Windows custom pairing interface unavailable: {err}"))?;
        let handler_label = candidate_label.to_string();
        let handler = TypedEventHandler::<
            DeviceInformationCustomPairing,
            DevicePairingRequestedEventArgs,
        >::new(move |_sender, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let kind = args.PairingKind()?;
            if pairing_kind_accepts_without_user(kind) {
                log::info!(
                    "[embedded-ble] accepting Windows custom BLE pairing request for {handler_label} kind={kind:?}"
                );
                args.Accept()?;
            } else {
                log::warn!(
                    "[embedded-ble] Windows custom BLE pairing for {handler_label} needs unsupported ceremony kind={kind:?}"
                );
            }
            Ok(())
        });
        let token = custom
            .PairingRequested(&handler)
            .map_err(|err| format!("register Windows custom pairing handler failed: {err}"))?;
        let supported_pairing_kinds =
            DevicePairingKinds::ConfirmOnly | DevicePairingKinds::ConfirmPinMatch;
        let pair_result = custom
            .PairWithProtectionLevelAsync(
                supported_pairing_kinds,
                DevicePairingProtectionLevel::Default,
            )
            .map_err(|err| {
                format!("Windows encrypted custom pairing operation failed to start: {err}")
            })
            .and_then(|operation| {
                wait_async_operation(operation, BLE_PAIRING_PROMPT_TIMEOUT, "custom device pair")
            });
        if let Err(err) = custom.RemovePairingRequested(token) {
            log::warn!("[embedded-ble] remove Windows custom pairing handler failed: {err}");
        }
        let pair = pair_result?;
        let status = pair
            .Status()
            .map_err(|err| format!("Windows custom pairing status read failed: {err}"))?;
        pairing_status_or_reachable(
            status,
            expected_name,
            &format!("Windows custom pairing returned status={status:?}"),
        )
    }

    fn pairing_kind_accepts_without_user(kind: DevicePairingKinds) -> bool {
        kind == DevicePairingKinds::ConfirmOnly || kind == DevicePairingKinds::ConfirmPinMatch
    }

    fn pairing_error_should_skip_standard_fallback(error: &str) -> bool {
        let lower = error.to_ascii_lowercase();
        lower.contains("timed out after")
            || lower.contains("already pairing")
            || lower.contains("operationalreadyinprogress")
            || lower.contains("operation already in progress")
    }

    fn pairing_status_or_reachable(
        status: DevicePairingResultStatus,
        _expected_name: Option<&str>,
        error_message: &str,
    ) -> Result<DevicePairingOutcome, String> {
        match status {
            DevicePairingResultStatus::Paired => Ok(DevicePairingOutcome::Paired),
            DevicePairingResultStatus::AlreadyPaired => Ok(DevicePairingOutcome::AlreadyPaired),
            DevicePairingResultStatus::OperationAlreadyInProgress => {
                Err("Windows is already pairing this device".to_string())
            }
            DevicePairingResultStatus::AccessDenied => Err(
                "Windows requires user confirmation in Bluetooth settings before pairing"
                    .to_string(),
            ),
            DevicePairingResultStatus::PairingCanceled => {
                Err("Windows pairing was canceled".to_string())
            }
            _ => Err(error_message.to_string()),
        }
    }

    fn refresh_listener_pairing_candidate(
        stale_candidate: &ListenerPairingCandidate,
        expected_name: Option<&str>,
    ) -> Result<Option<ListenerPairingCandidate>, String> {
        let stale_id = stale_candidate
            .info
            .Id()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let stale_address = parse_bluetooth_address_from_device_id(&stale_id);

        let candidates = listener_pairing_candidates(expected_name)?;
        for candidate in candidates {
            let candidate_id = candidate
                .info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let candidate_address = parse_bluetooth_address_from_device_id(&candidate_id);
            let same_address = stale_address.is_some() && candidate_address == stale_address;
            let same_id = !stale_id.is_empty() && candidate_id == stale_id;
            if same_address || same_id {
                return Ok(Some(candidate));
            }
        }

        let Some(address) = stale_address else {
            return Ok(None);
        };
        let Some(info) = pairing_device_information_from_bluetooth_address(
            address,
            None,
            "refreshed BLE pairing device query",
        )?
        else {
            return Ok(None);
        };
        let name = info
            .Name()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let label = listener_pairing_candidate_label(&name, Some(address));
        Ok(Some(ListenerPairingCandidate { label, info }))
    }

    fn unpair_listener_candidate(
        candidate: &ListenerUnpairCandidate,
    ) -> Result<DeviceUnpairOutcome, String> {
        let pairing = candidate
            .info
            .Pairing()
            .map_err(|err| format!("read pairing info failed: {err}"))?;
        unpair_device_information_pairing(&pairing, &candidate.label)
    }

    fn unpair_device_information_pairing(
        pairing: &DeviceInformationPairing,
        label: &str,
    ) -> Result<DeviceUnpairOutcome, String> {
        let is_paired = pairing
            .IsPaired()
            .map_err(|err| format!("read pairing state failed: {err}"))?;
        if !is_paired {
            return Ok(DeviceUnpairOutcome::AlreadyUnpaired);
        }
        log::info!("[embedded-ble] unpairing Windows BLE device cache for {label}");

        let operation = pairing
            .UnpairAsync()
            .map_err(|err| format!("Windows unpair operation failed to start: {err}"))?;
        let unpair = wait_async_operation(operation, BLE_DISCOVERY_TIMEOUT, "device unpair")?;
        let status = unpair
            .Status()
            .map_err(|err| format!("Windows unpair status read failed: {err}"))?;
        match status {
            DeviceUnpairingResultStatus::Unpaired => Ok(DeviceUnpairOutcome::Unpaired),
            DeviceUnpairingResultStatus::AlreadyUnpaired => {
                Ok(DeviceUnpairOutcome::AlreadyUnpaired)
            }
            DeviceUnpairingResultStatus::OperationAlreadyInProgress => {
                Err("Windows is already removing or pairing this device".to_string())
            }
            DeviceUnpairingResultStatus::AccessDenied => Err(
                "Windows requires the user to remove this device in Bluetooth settings".to_string(),
            ),
            DeviceUnpairingResultStatus::Failed => {
                Err("Windows failed to remove this Bluetooth pairing".to_string())
            }
            other => Err(format!("Windows returned unpair status={other:?}")),
        }
    }

    fn listener_unpair_candidates(
        target_names: &[String],
        target_addresses: &mut Vec<u64>,
    ) -> Result<Vec<ListenerUnpairCandidate>, String> {
        let mut candidates = Vec::new();
        let mut seen_ids = Vec::new();
        let mut errors = Vec::new();

        for (label, service_uuid) in [
            ("audio service", SERVICE_UUID),
            ("OTA service", OTA_SERVICE_UUID),
            ("diagnostic service", DIAGNOSTIC_SERVICE_UUID),
        ] {
            match push_service_unpair_candidates(
                &mut candidates,
                &mut seen_ids,
                target_addresses,
                label,
                service_uuid,
            ) {
                Ok(()) => {}
                Err(err) => errors.push(err),
            }
        }

        match push_address_unpair_candidates(&mut candidates, &mut seen_ids, target_addresses) {
            Ok(()) => {}
            Err(err) => errors.push(err),
        }

        match push_ble_device_unpair_candidates(
            &mut candidates,
            &mut seen_ids,
            target_addresses,
            target_names,
        ) {
            Ok(()) => {}
            Err(err) => errors.push(err),
        }

        if candidates.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(candidates)
    }

    fn listener_recovery_target_addresses() -> Vec<u64> {
        configured_bluetooth_address().into_iter().collect()
    }

    fn push_listener_recovery_target_addresses_from_candidates(
        target_addresses: &mut Vec<u64>,
        candidates: &[ListenerUnpairCandidate],
    ) {
        for candidate in candidates {
            let id = candidate
                .info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if let Some(address) = parse_bluetooth_address_from_device_id(&id) {
                push_unique_address(target_addresses, address);
            }
        }
    }

    fn push_listener_recovery_advertised_addresses(
        target_addresses: &mut Vec<u64>,
        target_names: &[String],
    ) {
        let mut scanned_names: Vec<String> = Vec::new();
        for target_name in target_names.iter().rev() {
            if target_name.trim().is_empty()
                || scanned_names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(target_name))
            {
                continue;
            }
            scanned_names.push(target_name.clone());
            match scan_listener_pairing_advertisements(Some(target_name)) {
                Ok(candidates) => {
                    if candidates.is_empty() {
                        continue;
                    }
                    for (address, address_type, advertised_name) in candidates {
                        log::info!(
                            "[embedded-ble] matched Listener advertisement for stale-cache cleanup target={target_name:?} advertised_name={advertised_name:?} address={} address_type={address_type:?}",
                            crate::embedded_ble::format_bluetooth_address(address)
                        );
                        push_unique_address(target_addresses, address);
                    }
                    break;
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] Listener advertisement address scan for stale-cache cleanup failed target={target_name:?}: {err}"
                    );
                }
            }
        }
    }

    fn push_service_unpair_candidates(
        candidates: &mut Vec<ListenerUnpairCandidate>,
        seen_ids: &mut Vec<String>,
        target_addresses: &mut Vec<u64>,
        label: &str,
        service_uuid: GUID,
    ) -> Result<(), String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(service_uuid)
            .map_err(|err| format!("BLE {label} selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE {label} query failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, label))?;
        let count = devices
            .Size()
            .map_err(|err| format!("BLE {label} collection size failed: {err}"))?;
        for index in 0..count {
            let info = devices
                .GetAt(index)
                .map_err(|err| format!("BLE {label} entry {index} read failed: {err}"))?;
            let id = info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if let Some(address) = parse_bluetooth_address_from_device_id(&id) {
                push_unique_address(target_addresses, address);
            }
            push_unpair_candidate(candidates, seen_ids, info, label);
        }
        Ok(())
    }

    fn push_address_unpair_candidates(
        candidates: &mut Vec<ListenerUnpairCandidate>,
        seen_ids: &mut Vec<String>,
        target_addresses: &[u64],
    ) -> Result<(), String> {
        for address in target_addresses.iter().copied() {
            let address_text = crate::embedded_ble::format_bluetooth_address(address);
            let operation =
                BluetoothLEDevice::FromBluetoothAddressAsync(address).map_err(|err| {
                    format!("BLE address cleanup query {address_text} failed to start: {err}")
                })?;
            let device = match wait_async_operation(
                operation,
                BLE_DISCOVERY_TIMEOUT,
                "BLE address cleanup query",
            ) {
                Ok(device) => device,
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] BLE address cleanup query for {address_text} failed: {err}"
                    );
                    continue;
                }
            };
            let info = match device.DeviceInformation() {
                Ok(info) => info,
                Err(err) => {
                    let _ = device.Close();
                    log::warn!(
                        "[embedded-ble] BLE address cleanup DeviceInformation for {address_text} failed: {err}"
                    );
                    continue;
                }
            };
            let _ = device.Close();
            log::info!(
                "[embedded-ble] opened BLE address object for stale pairing cleanup: {address_text}"
            );
            push_unpair_candidate(candidates, seen_ids, info, "BLE address");
        }
        Ok(())
    }

    fn push_ble_device_unpair_candidates(
        candidates: &mut Vec<ListenerUnpairCandidate>,
        seen_ids: &mut Vec<String>,
        target_addresses: &[u64],
        target_names: &[String],
    ) -> Result<(), String> {
        let selector = BluetoothLEDevice::GetDeviceSelector()
            .map_err(|err| format!("BLE device selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE device query failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "BLE device query"))?;
        let count = devices
            .Size()
            .map_err(|err| format!("BLE device collection size failed: {err}"))?;
        for index in 0..count {
            let info = devices
                .GetAt(index)
                .map_err(|err| format!("BLE device entry {index} read failed: {err}"))?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let address_matches = parse_bluetooth_address_from_device_id(&id)
                .is_some_and(|address| target_addresses.contains(&address));
            if address_matches || bluetooth_name_matches_any(&name, target_names) {
                push_unpair_candidate(candidates, seen_ids, info, "BLE device");
            }
        }
        Ok(())
    }

    fn push_unpair_candidate(
        candidates: &mut Vec<ListenerUnpairCandidate>,
        seen_ids: &mut Vec<String>,
        info: DeviceInformation,
        source: &str,
    ) {
        let name = info
            .Name()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let id = info
            .Id()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        if id.is_empty() || seen_ids.iter().any(|seen| seen == &id) {
            return;
        }
        seen_ids.push(id.clone());
        let address = parse_bluetooth_address_from_device_id(&id)
            .map(crate::embedded_ble::format_bluetooth_address);
        let label = match (name.trim().is_empty(), address) {
            (false, Some(address)) => format!("{source} {name} ({address})"),
            (false, None) => format!("{source} {name}"),
            (true, Some(address)) => format!("{source} {address}"),
            (true, None) => source.to_string(),
        };
        candidates.push(ListenerUnpairCandidate { label, info });
    }

    fn listener_pnp_remove_candidates(
        target_addresses: &[u64],
        target_names: &[String],
    ) -> Result<Vec<ListenerPnpRemoveCandidate>, String> {
        let devices = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::All)
            .map_err(|err| format!("Windows PnP device query failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "PnP device query"))?;
        let count = devices
            .Size()
            .map_err(|err| format!("Windows PnP device collection size failed: {err}"))?;
        let mut entries = Vec::new();
        let mut known_addresses = target_addresses.to_vec();
        for index in 0..count {
            let info = devices
                .GetAt(index)
                .map_err(|err| format!("Windows PnP device entry {index} read failed: {err}"))?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let raw_id = info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if let Some(entry) = listener_pnp_entry_from_name_and_id(name, raw_id) {
                push_listener_pnp_entry(&mut entries, &mut known_addresses, target_names, entry);
            }
        }

        match powershell_listener_pnp_entries() {
            Ok(powershell_entries) => {
                for entry in powershell_entries {
                    push_listener_pnp_entry(
                        &mut entries,
                        &mut known_addresses,
                        target_names,
                        entry,
                    );
                }
            }
            Err(err) => {
                log::warn!("[embedded-ble] PowerShell PnP fallback enumeration failed: {err}");
            }
        }

        let mut candidates = Vec::new();
        let mut seen_ids = Vec::new();
        for entry in entries {
            let address_matches = entry
                .address
                .is_some_and(|value| known_addresses.contains(&value));
            let name_matches = bluetooth_name_matches_any(&entry.name, target_names);
            if !listener_pnp_entry_matches_cleanup(&entry, address_matches, name_matches) {
                continue;
            }
            if seen_ids.iter().any(|seen| seen == &entry.instance_id) {
                continue;
            }
            seen_ids.push(entry.instance_id.clone());
            let label = match (entry.name.trim().is_empty(), entry.address) {
                (false, Some(address)) => format!(
                    "{} ({}) [{}]",
                    entry.name,
                    crate::embedded_ble::format_bluetooth_address(address),
                    entry.instance_id
                ),
                (false, None) => format!("{} [{}]", entry.name, entry.instance_id),
                (true, Some(address)) => format!(
                    "{} [{}]",
                    crate::embedded_ble::format_bluetooth_address(address),
                    entry.instance_id
                ),
                (true, None) => entry.instance_id.clone(),
            };
            if name_matches {
                log::info!("[embedded-ble] matched Listener PnP node by name: {label}");
            } else if entry.has_listener_service_signature {
                log::info!("[embedded-ble] matched Listener PnP node by service UUID: {label}");
            }
            candidates.push(ListenerPnpRemoveCandidate {
                label,
                instance_id: entry.instance_id,
                name: entry.name,
                address: entry.address,
                is_ble_device_root: entry.is_ble_device_root,
            });
        }
        Ok(candidates)
    }

    fn push_listener_pnp_entry(
        entries: &mut Vec<ListenerPnpEntry>,
        known_addresses: &mut Vec<u64>,
        target_names: &[String],
        entry: ListenerPnpEntry,
    ) {
        if bluetooth_name_matches_any(&entry.name, target_names)
            || entry.has_listener_service_signature
        {
            if let Some(address) = entry.address {
                push_unique_address(known_addresses, address);
            }
        }
        if entries.iter().any(|existing| {
            existing
                .instance_id
                .eq_ignore_ascii_case(&entry.instance_id)
        }) {
            return;
        }
        entries.push(entry);
    }

    fn listener_pnp_entry_from_name_and_id(
        name: String,
        raw_id: String,
    ) -> Option<ListenerPnpEntry> {
        let instance_id = normalize_pnp_device_instance_id(&raw_id)?;
        let address = parse_bluetooth_address_from_device_id(&instance_id);
        Some(ListenerPnpEntry {
            name,
            is_ble_device_root: pnp_instance_is_ble_device_root(&instance_id),
            has_listener_service_signature: pnp_instance_has_listener_service_signature(
                &instance_id,
            ),
            instance_id,
            address,
        })
    }

    fn powershell_listener_pnp_entries() -> Result<Vec<ListenerPnpEntry>, String> {
        let script = r#"
$ProgressPreference = 'SilentlyContinue'
Get-PnpDevice -ErrorAction SilentlyContinue |
  Where-Object { $_.InstanceId -match '^(BTHLE|BTHLEDEVICE|HID)\\' } |
  Select-Object FriendlyName,InstanceId |
  ConvertTo-Json -Compress
"#;
        let mut command = hidden_command("powershell.exe");
        let output = command
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .output()
            .map_err(|err| format!("start powershell.exe failed: {err}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("powershell.exe exited with status {}", output.status)
            } else {
                format!(
                    "powershell.exe exited with status {}: {stderr}",
                    output.status
                )
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Ok(Vec::new());
        }
        let value: serde_json::Value = serde_json::from_str(&stdout)
            .map_err(|err| format!("parse Get-PnpDevice JSON failed: {err}; output={stdout}"))?;
        let raw_entries = match value {
            serde_json::Value::Array(values) => values,
            serde_json::Value::Null => Vec::new(),
            other => vec![other],
        };

        let mut entries = Vec::new();
        for value in raw_entries {
            let device: PowerShellPnpDeviceEntry = serde_json::from_value(value)
                .map_err(|err| format!("decode Get-PnpDevice entry failed: {err}"))?;
            let Some(instance_id) = device.instance_id else {
                continue;
            };
            if let Some(entry) = listener_pnp_entry_from_name_and_id(
                device.friendly_name.unwrap_or_default(),
                instance_id,
            ) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    pub(super) fn listener_pnp_entry_matches_cleanup(
        entry: &ListenerPnpEntry,
        address_matches: bool,
        name_matches: bool,
    ) -> bool {
        address_matches || name_matches || entry.has_listener_service_signature
    }

    pub(super) fn pnp_instance_has_listener_service_signature(instance_id: &str) -> bool {
        let upper = instance_id.to_ascii_uppercase();
        LISTENER_SERVICE_UUID_TEXTS.iter().any(|uuid| {
            let uuid_upper = uuid.to_ascii_uppercase();
            upper.contains(&uuid_upper)
        })
    }

    pub(super) fn pnp_instance_is_ble_device_root(instance_id: &str) -> bool {
        instance_id.to_ascii_uppercase().starts_with(r"BTHLE\DEV_")
    }

    fn bthport_listener_cache_candidates(
        target_addresses: &[u64],
        target_names: &[String],
    ) -> Result<Vec<ListenerBthPortCacheCandidate>, String> {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let devices = hklm
            .open_subkey_with_flags(BTHPORT_DEVICE_CACHE_REGISTRY_PATH, KEY_READ)
            .map_err(|err| format!("open BTHPORT device cache failed: {err}"))?;
        let mut candidates = Vec::new();
        let mut seen_keys = Vec::new();

        for key_result in devices.enum_keys() {
            let address_key = key_result
                .map_err(|err| format!("enumerate BTHPORT device cache failed: {err}"))?;
            let address = parse_bluetooth_address_hex_exact(&address_key);
            let subkey = match devices.open_subkey_with_flags(&address_key, KEY_READ) {
                Ok(value) => value,
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] could not open BTHPORT cache key {address_key}: {err}"
                    );
                    continue;
                }
            };
            let name = read_bthport_device_name(&subkey).unwrap_or_default();
            let address_matches = address.is_some_and(|value| target_addresses.contains(&value));
            if !address_matches && !bluetooth_name_matches_any(&name, target_names) {
                continue;
            }
            if seen_keys.iter().any(|seen| seen == &address_key) {
                continue;
            }
            seen_keys.push(address_key.clone());
            let label = match (name.trim().is_empty(), address) {
                (false, Some(address)) => format!(
                    "{} ({}) [BTHPORT\\{}]",
                    name,
                    crate::embedded_ble::format_bluetooth_address(address),
                    address_key
                ),
                (false, None) => format!("{name} [BTHPORT\\{address_key}]"),
                (true, Some(address)) => format!(
                    "{} [BTHPORT\\{}]",
                    crate::embedded_ble::format_bluetooth_address(address),
                    address_key
                ),
                (true, None) => format!("BTHPORT\\{address_key}"),
            };
            candidates.push(ListenerBthPortCacheCandidate {
                label,
                address_key,
                name,
                address,
            });
        }

        Ok(candidates)
    }

    fn read_bthport_device_name(key: &RegKey) -> Option<String> {
        let raw = key.get_raw_value("Name").ok()?;
        Some(decode_bthport_device_name(&raw.bytes))
    }

    pub(super) fn decode_bthport_device_name(bytes: &[u8]) -> String {
        let mut utf16_end = bytes.len();
        while utf16_end >= 2 && bytes[utf16_end - 1] == 0 && bytes[utf16_end - 2] == 0 {
            utf16_end -= 2;
        }
        let utf16_candidate = &bytes[..utf16_end];
        if utf16_candidate.len() >= 2 && utf16_candidate.len() % 2 == 0 {
            let zero_high_bytes = utf16_candidate
                .chunks_exact(2)
                .filter(|pair| pair[1] == 0)
                .count();
            if zero_high_bytes * 2 >= utf16_candidate.len() {
                let utf16: Vec<u16> = utf16_candidate
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                return String::from_utf16_lossy(&utf16)
                    .trim_matches('\0')
                    .trim()
                    .to_string();
            }
        }

        let end = bytes
            .iter()
            .rposition(|byte| *byte != 0)
            .map(|index| index + 1)
            .unwrap_or(0);
        let trimmed = &bytes[..end];
        String::from_utf8_lossy(trimmed)
            .trim_matches('\0')
            .trim()
            .to_string()
    }

    fn delete_bthport_cache_candidate(
        candidate: &ListenerBthPortCacheCandidate,
    ) -> Result<DeviceUnpairOutcome, String> {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let devices = hklm
            .open_subkey_with_flags(BTHPORT_DEVICE_CACHE_REGISTRY_PATH, KEY_READ | KEY_WRITE)
            .map_err(|err| format!("open BTHPORT device cache for write failed: {err}"))?;
        match devices.delete_subkey_all(&candidate.address_key) {
            Ok(()) => Ok(DeviceUnpairOutcome::Unpaired),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(DeviceUnpairOutcome::AlreadyUnpaired)
            }
            Err(err) => Err(format!("delete BTHPORT cache key failed: {err}")),
        }
    }

    pub fn listener_ble_name_cache_needs_cleanup(expected_name: &str) -> bool {
        listener_ble_name_cache_needs_cleanup_for_names(expected_name, &[])
    }

    pub fn listener_ble_name_cache_needs_cleanup_for_names(
        expected_name: &str,
        extra_names: &[String],
    ) -> bool {
        let expected_name = expected_name.trim();
        if expected_name.is_empty() {
            return false;
        }
        let target_names = listener_target_names(extra_names);
        let target_addresses = listener_recovery_target_addresses();
        match listener_pnp_remove_candidates(&target_addresses, &target_names) {
            Ok(candidates) => {
                if candidates.iter().any(|candidate| {
                    let name = candidate.name.trim();
                    candidate.is_ble_device_root
                        && !name.is_empty()
                        && !name.eq_ignore_ascii_case(expected_name)
                }) {
                    return true;
                }
            }
            Err(err) => {
                log::warn!("[embedded-ble] BLE PnP stale-node mismatch check failed: {err}");
            }
        }
        match bthport_listener_cache_candidates(&target_addresses, &target_names) {
            Ok(candidates) => candidates.iter().any(|candidate| {
                let name = candidate.name.trim();
                !name.is_empty() && !name.eq_ignore_ascii_case(expected_name)
            }),
            Err(err) => {
                log::warn!("[embedded-ble] BLE name cache mismatch check failed: {err}");
                false
            }
        }
    }

    pub(super) fn normalize_pnp_device_instance_id(raw_id: &str) -> Option<String> {
        let trimmed = raw_id.trim().trim_matches('\0');
        if trimmed.is_empty() {
            return None;
        }
        let upper = trimmed.to_ascii_uppercase();
        let start = [
            "BTHLE\\",
            "BTHLE#",
            "BTHLEDEVICE\\",
            "BTHLEDEVICE#",
            "HID\\",
            "HID#",
        ]
        .iter()
        .filter_map(|marker| upper.find(marker))
        .min()?;
        let mut value = trimmed[start..].to_string();
        if let Some(guid_marker) = value.find("#{") {
            value.truncate(guid_marker);
        }
        if value.contains('#') {
            value = value.replace('#', "\\");
        }
        let normalized_upper = value.to_ascii_uppercase();
        if !normalized_upper.starts_with("BTHLE\\")
            && !normalized_upper.starts_with("BTHLEDEVICE\\")
            && !normalized_upper.starts_with("HID\\")
        {
            return None;
        }
        Some(value)
    }

    fn remove_pnp_device_candidate(
        candidate: &ListenerPnpRemoveCandidate,
    ) -> Result<DeviceUnpairOutcome, String> {
        let cm_outcome = remove_pnp_device_candidate_with_cfgmgr(candidate);
        if !candidate.is_ble_device_root {
            return cm_outcome;
        }

        match remove_pnp_device_candidate_with_pnputil(candidate) {
            Ok(DeviceUnpairOutcome::Unpaired) => Ok(DeviceUnpairOutcome::Unpaired),
            Ok(DeviceUnpairOutcome::AlreadyUnpaired) => cm_outcome,
            Err(err) => {
                log::warn!(
                    "[embedded-ble] pnputil root-node cleanup failed for {}: {err}",
                    candidate.label
                );
                cm_outcome
            }
        }
    }

    fn remove_pnp_device_candidate_with_cfgmgr(
        candidate: &ListenerPnpRemoveCandidate,
    ) -> Result<DeviceUnpairOutcome, String> {
        let wide_id: Vec<u16> = candidate
            .instance_id
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut devinst = 0u32;
        let locate = unsafe {
            CM_Locate_DevNodeW(
                &mut devinst,
                PCWSTR(wide_id.as_ptr()),
                CM_LOCATE_DEVNODE_NORMAL,
            )
        };
        if locate == CR_NO_SUCH_DEVINST || locate == CR_NO_SUCH_DEVNODE {
            return Ok(DeviceUnpairOutcome::AlreadyUnpaired);
        }
        if locate != CR_SUCCESS {
            return Err(format!("locate failed: {}", configret_detail(locate, None)));
        }

        let mut veto_type = PNP_VETO_TYPE(0);
        let mut veto_name = vec![0u16; 260];
        let remove = unsafe {
            CM_Query_And_Remove_SubTreeW(
                devinst,
                Some(&mut veto_type as *mut PNP_VETO_TYPE),
                Some(veto_name.as_mut_slice()),
                CM_REMOVE_UI_NOT_OK | CM_REMOVE_NO_RESTART,
            )
        };
        if remove == CR_SUCCESS {
            return Ok(DeviceUnpairOutcome::Unpaired);
        }
        if remove == CR_NO_SUCH_DEVINST || remove == CR_NO_SUCH_DEVNODE {
            return Ok(DeviceUnpairOutcome::AlreadyUnpaired);
        }
        Err(format!(
            "remove failed: {}",
            configret_detail(remove, Some((&veto_type, &veto_name)))
        ))
    }

    fn remove_pnp_device_candidate_with_pnputil(
        candidate: &ListenerPnpRemoveCandidate,
    ) -> Result<DeviceUnpairOutcome, String> {
        let mut command = hidden_command("pnputil");
        let output = command
            .args(["/remove-device", &candidate.instance_id])
            .output()
            .map_err(|err| format!("start pnputil failed: {err}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        let lower = combined.to_ascii_lowercase();
        if output.status.success() {
            if lower.contains("no devices were removed")
                || lower.contains("not found")
                || lower.contains("no matching devices")
            {
                return Ok(DeviceUnpairOutcome::AlreadyUnpaired);
            }
            return Ok(DeviceUnpairOutcome::Unpaired);
        }
        Err(if combined.trim().is_empty() {
            format!("pnputil exited with status {}", output.status)
        } else {
            format!(
                "pnputil exited with status {}: {}",
                output.status,
                combined.trim()
            )
        })
    }

    fn hidden_command(program: &str) -> Command {
        let mut command = Command::new(program);
        command.creation_flags(WINDOWS_CREATE_NO_WINDOW);
        command
    }

    fn configret_detail(status: CONFIGRET, veto: Option<(&PNP_VETO_TYPE, &[u16])>) -> String {
        let label = if status == CR_ACCESS_DENIED {
            "access denied"
        } else if status == CR_REMOVE_VETOED {
            "remove vetoed"
        } else if status == CR_QUERY_VETOED {
            "query vetoed"
        } else if status == CR_NO_SUCH_DEVINST || status == CR_NO_SUCH_DEVNODE {
            "device node not found"
        } else {
            "configuration manager error"
        };
        let mut detail = format!("{label} ({status:?})");
        if let Some((veto_type, veto_name)) = veto {
            let end = veto_name
                .iter()
                .position(|ch| *ch == 0)
                .unwrap_or(veto_name.len());
            let veto_name = String::from_utf16_lossy(&veto_name[..end]);
            if !veto_name.trim().is_empty() || veto_type.0 != 0 {
                detail.push_str(&format!(
                    ", veto_type={veto_type:?}, veto_name={}",
                    veto_name.trim()
                ));
            }
        }
        detail
    }

    fn push_unique_address(addresses: &mut Vec<u64>, address: u64) {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }

    pub fn pull_firmware_diagnostic_log(
        timeout: Duration,
    ) -> crate::embedded_ble::FirmwareDiagnosticLogPull {
        match pull_firmware_diagnostic_log_inner(timeout) {
            Ok(pull) => pull,
            Err(err) => {
                log::warn!("[embedded-ble] firmware diagnostic log pull unavailable: {err}");
                crate::embedded_ble::FirmwareDiagnosticLogPull::offline("windows", err)
            }
        }
    }

    fn pull_firmware_diagnostic_log_inner(
        timeout: Duration,
    ) -> Result<crate::embedded_ble::FirmwareDiagnosticLogPull, String> {
        let timeout = timeout.clamp(Duration::from_secs(3), Duration::from_secs(60));
        let candidates = diagnostic_target_candidates()?;
        let mut errors = Vec::new();

        for candidate in candidates {
            let label = candidate.label().to_string();
            match candidate
                .open()
                .and_then(|target| pull_firmware_diagnostic_log_from_target(timeout, target))
            {
                Ok(pull) => {
                    log::info!("[embedded-ble] firmware diagnostic log pull succeeded via {label}");
                    return Ok(pull);
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] firmware diagnostic log candidate {label} failed: {err}"
                    );
                    errors.push(format!("{label}: {err}"));
                }
            }

            std::thread::sleep(DIAGNOSTIC_PULL_CANDIDATE_DELAY);
        }

        Err(format!(
            "No usable Listener BLE diagnostic target completed export: {}",
            if errors.is_empty() {
                "no candidates attempted".to_string()
            } else {
                errors.join("; ")
            }
        ))
    }

    fn pull_firmware_diagnostic_log_from_target(
        timeout: Duration,
        target: OpenDiagnosticTarget,
    ) -> Result<crate::embedded_ble::FirmwareDiagnosticLogPull, String> {
        let control = target.control.clone();
        let data = target.data.clone();
        let count = target.count.clone();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let handler = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
            move |_sender, args| {
                if let Some(args) = args {
                    if let Ok(buffer) = args.CharacteristicValue() {
                        if let Ok(bytes) = buffer_to_vec(&buffer) {
                            let _ = tx.send(bytes);
                        }
                    }
                }
                Ok(())
            },
        );
        let mut cleanup = DiagnosticNotifyCleanup::new(target);
        let token = data
            .ValueChanged(&handler)
            .map_err(|err| format!("BLE diagnostic ValueChanged registration failed: {err}"))?;
        cleanup.set_token(token);
        let status = write_cccd_notify_with_retry(0, "diagnostic log", &data, CCCD_ENABLE_TIMEOUT)?;
        if status != GattCommunicationStatus::Success {
            return Err(format!(
                "BLE diagnostic notify CCCD write returned status={status:?}"
            ));
        }

        let initial_count = read_diagnostic_count(&count)?;
        let mut export_started = false;
        let result = (|| -> Result<crate::embedded_ble::FirmwareDiagnosticLogPull, String> {
            write_diagnostic_control(&control, "{\"op\":\"start\"}", timeout, "start")?;
            export_started = true;
            let export_count_snapshot = read_diagnostic_count(&count)?;
            let mut offset = 0usize;
            let mut raw_event_bytes = Vec::new();
            let mut chunks = Vec::new();

            while offset < export_count_snapshot as usize {
                let command = format!("{{\"op\":\"read\",\"offset\":{offset}}}");
                write_diagnostic_control(&control, &command, timeout, "read")?;
                let packet = rx.recv_timeout(timeout).map_err(|err| {
                    format!("BLE diagnostic notification timed out at offset {offset}: {err}")
                })?;
                let (event_count, header_offset, firmware_crc, payload) =
                    parse_diagnostic_chunk(&packet, offset)?;
                if header_offset != (offset & 0xFFFF) as u16 {
                    return Err(format!(
                        "BLE diagnostic chunk offset mismatch: host={offset} firmware_header={header_offset}"
                    ));
                }
                let host_crc = crate::embedded_ble::crc32(payload);
                if host_crc != firmware_crc {
                    return Err(format!(
                        "BLE diagnostic chunk CRC mismatch: offset={offset} firmware={} host={}",
                        crate::embedded_ble::format_crc32(firmware_crc),
                        crate::embedded_ble::format_crc32(host_crc)
                    ));
                }
                chunks.push(crate::embedded_ble::FirmwareDiagnosticLogChunk {
                    offset,
                    event_count,
                    value_bytes: packet.len(),
                    events_crc32: crate::embedded_ble::format_crc32(firmware_crc),
                });
                raw_event_bytes.extend_from_slice(payload);
                offset += usize::from(event_count);
                if offset < export_count_snapshot as usize {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }

            write_diagnostic_control(&control, "{\"op\":\"stop\"}", timeout, "stop")?;
            export_started = false;
            let final_count = read_diagnostic_count(&count)?;
            Ok(crate::embedded_ble::FirmwareDiagnosticLogPull::from_events(
                "windows",
                initial_count,
                export_count_snapshot,
                final_count,
                chunks,
                raw_event_bytes,
            ))
        })();

        if export_started {
            let _ = write_diagnostic_control(&control, "{\"op\":\"stop\"}", timeout, "stop");
        }
        cleanup.disable_notify();
        result
    }

    pub fn probe_notify_subscription(timeout: Duration) -> Result<(), String> {
        let capture_guard = BleCaptureGuard::enter(Some(timeout))?;
        let capture_id = capture_guard.session_id();
        let target = open_notify_target_with_retry(capture_id)?;
        let characteristic = target.characteristic.clone();
        let handler = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
            |_sender, _args| Ok(()),
        );
        let mut cleanup = NotifyCleanup::new(capture_id, target);
        let token = characteristic
            .ValueChanged(&handler)
            .map_err(|err| format!("BLE ValueChanged handler registration failed: {err}"))?;
        cleanup.set_token(token);
        log::info!("[embedded-ble] probe #{capture_id}: ValueChanged handler registered");

        let notify_timeout = timeout.clamp(Duration::from_secs(1), Duration::from_secs(10));
        log::info!("[embedded-ble] probe #{capture_id}: enabling notify CCCD without pre-reset");
        let status =
            write_cccd_notify_with_retry(capture_id, "probe", &characteristic, notify_timeout)?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("BLE CCCD notify write returned status={status:?}"));
        }
        log::info!("[embedded-ble] probe #{capture_id}: notify CCCD enabled");
        cleanup.finish(NotifyCccdTeardown::for_probe_success());
        Ok(())
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ActiveControlTransientFallback {
        TryFreshGatt,
        ReturnError,
    }

    fn recording_stop_active_transient_fallback() -> ActiveControlTransientFallback {
        ActiveControlTransientFallback::TryFreshGatt
    }

    fn processing_state_active_transient_fallback(active: bool) -> ActiveControlTransientFallback {
        if active {
            // PROCESSING:START is an LED hint. If the active capture is already
            // closing, a late fresh-GATT START can race after DONE and leave the
            // device in thinking state.
            ActiveControlTransientFallback::ReturnError
        } else {
            ActiveControlTransientFallback::TryFreshGatt
        }
    }

    fn send_recording_control_command(
        command: &[u8],
        timeout: Duration,
        label: &'static str,
        active_transient_fallback: ActiveControlTransientFallback,
    ) -> Result<(), String> {
        if let Some(result) = send_audio_control_via_active_capture(command, timeout, label) {
            match result {
                Ok(_) => {
                    log::info!("[embedded-ble] {label} sent via active capture");
                    return Ok(());
                }
                Err(err) if is_transient_audio_control_write_error(&err) => {
                    if active_transient_fallback == ActiveControlTransientFallback::ReturnError {
                        log::warn!(
                            "[embedded-ble] active {label} write failed with transient error; returning for caller recovery: {err}"
                        );
                        return Err(err);
                    }
                    log::warn!(
                        "[embedded-ble] active {label} write failed with transient error; retrying fresh GATT path: {err}"
                    );
                }
                Err(err) => return Err(err),
            }
        }
        let _fresh_guard = BleFreshGattGuard::enter(label)?;
        let target = open_audio_control_target_with_retry(label)?;
        write_audio_control_value_with_timeout(&target.control, command, timeout, label)?;
        log::info!("[embedded-ble] {label} sent");
        Ok(())
    }

    fn send_type_ready_keepalive_before_processing(timeout: Duration, label: &'static str) {
        let command = type_ready_command_bytes();
        if let Err(err) = send_recording_control_command(
            command.as_slice(),
            timeout,
            label,
            ActiveControlTransientFallback::TryFreshGatt,
        ) {
            log::warn!("[embedded-ble] {label} failed before processing LED sync: {err}");
        }
    }

    fn send_type_bye_after_processing(timeout: Duration, label: &'static str) {
        if let Err(err) = send_recording_control_command(
            b"TYPE:BYE\n",
            timeout,
            label,
            ActiveControlTransientFallback::TryFreshGatt,
        ) {
            log::warn!("[embedded-ble] {label} failed after processing LED sync: {err}");
        }
    }

    pub fn send_recording_control_toggle(timeout: Duration) -> Result<(), String> {
        send_recording_control_command(
            b"VREC:TOGGLE\n",
            timeout,
            "audio control toggle",
            ActiveControlTransientFallback::TryFreshGatt,
        )
    }

    pub fn send_recording_control_cancel(timeout: Duration) -> Result<(), String> {
        send_recording_control_command(
            b"VREC:CANCEL\n",
            timeout,
            "audio control cancel",
            ActiveControlTransientFallback::TryFreshGatt,
        )
    }

    pub fn send_recording_control_stop(timeout: Duration) -> Result<(), String> {
        send_recording_control_command(
            b"VREC:STOP\n",
            timeout,
            "audio control stop",
            recording_stop_active_transient_fallback(),
        )
    }

    pub fn send_recording_control_recovery(timeout: Duration) -> Result<(), String> {
        let serial_result = send_control_command_via_usb_serial("VREC:RECOVERY", timeout);
        match &serial_result {
            Ok(()) => {
                log::info!("[embedded-ble] audio control recovery sent via USB serial");
                return Ok(());
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] audio control recovery USB serial path unavailable; trying BLE control: {err}"
                );
            }
        }
        send_recording_control_command(
            b"VREC:RECOVERY\n",
            timeout,
            "audio control recovery",
            ActiveControlTransientFallback::TryFreshGatt,
        )
    }

    pub fn send_recording_processing_state(active: bool, timeout: Duration) -> Result<(), String> {
        send_type_ready_keepalive_before_processing(timeout, "audio type ready before processing");
        let command = if active {
            b"VREC:PROCESSING:START\n".as_slice()
        } else {
            b"VREC:PROCESSING:STOP\n".as_slice()
        };
        let label = if active {
            "audio processing start"
        } else {
            "audio processing stop"
        };
        let result = send_recording_control_command(
            command,
            timeout,
            label,
            processing_state_active_transient_fallback(active),
        );
        if !active {
            send_type_bye_after_processing(timeout, "audio type bye after processing stop");
        }
        result
    }

    pub fn send_recording_processing_done(timeout: Duration) -> Result<(), String> {
        send_type_ready_keepalive_before_processing(
            timeout,
            "audio type ready before processing done",
        );
        let result = send_recording_control_command(
            b"VREC:PROCESSING:DONE\n",
            timeout,
            "audio processing done",
            ActiveControlTransientFallback::TryFreshGatt,
        );
        send_type_bye_after_processing(timeout, "audio type bye after processing done");
        result
    }

    pub fn send_recording_processing_warning(timeout: Duration) -> Result<(), String> {
        send_type_ready_keepalive_before_processing(
            timeout,
            "audio type ready before processing warning",
        );
        let result = send_recording_control_command(
            b"VREC:PROCESSING:WARN\n",
            timeout,
            "audio processing warning",
            ActiveControlTransientFallback::TryFreshGatt,
        );
        send_type_bye_after_processing(timeout, "audio type bye after processing warning");
        result
    }

    pub fn send_ec11_rotation_mode(mode: &str, timeout: Duration) -> Result<(), String> {
        let command = format!("EC11:MODE:{mode}\n");
        if let Some(result) =
            send_audio_control_via_active_capture(command.as_bytes(), timeout, "EC11 rotation mode")
        {
            match result {
                Ok(()) => {
                    log::info!(
                        "[embedded-ble] EC11 rotation mode sent via active capture mode={mode}"
                    );
                    return Ok(());
                }
                Err(err) if is_transient_audio_control_write_error(&err) => {
                    log::warn!(
                        "[embedded-ble] active EC11 rotation write failed with transient error; retrying fresh GATT path: {err}"
                    );
                }
                Err(err) => return Err(err),
            }
        }
        let _fresh_guard = BleFreshGattGuard::enter("EC11 rotation mode")?;
        let target = open_audio_control_target_with_retry("EC11 rotation mode")?;
        write_gatt_value_with_timeout(
            &target.control,
            command.as_bytes(),
            GattWriteOption::WriteWithResponse,
            timeout,
            "EC11 rotation mode",
        )?;
        log::info!("[embedded-ble] EC11 rotation mode sent mode={mode}");
        Ok(())
    }

    pub fn send_device_settings_command(command: &str, timeout: Duration) -> Result<(), String> {
        if !command.starts_with("DEVICE:") {
            return Err("device settings command must start with DEVICE:".to_string());
        }
        if command.contains('\r') || command.contains('\n') {
            return Err("device settings command must be a single line".to_string());
        }
        let payload = format!("{command}\n");
        let payload_len = payload.as_bytes().len();
        let mut active_capture_error: Option<String> = None;

        if payload_len < 64 && device_settings_command_allows_active_capture(command) {
            if let Some(result) = send_audio_control_via_active_capture(
                payload.as_bytes(),
                timeout,
                "device settings",
            ) {
                match result {
                    Ok(()) => {
                        log::info!(
                            "[embedded-ble] device settings command sent via active capture"
                        );
                        return Ok(());
                    }
                    Err(err) if is_transient_audio_control_write_error(&err) => {
                        log::warn!(
                            "[embedded-ble] active device settings write failed with transient error; trying USB serial/fresh GATT fallback: {err}"
                        );
                        active_capture_error = Some(err);
                    }
                    Err(err) => {
                        log::warn!(
                            "[embedded-ble] active device settings write failed; trying USB serial/fresh GATT fallback: {err}"
                        );
                        active_capture_error = Some(err);
                    }
                }
            }
        }

        let serial_result = send_device_settings_via_usb_serial(command, timeout);
        match &serial_result {
            Ok(()) => {
                log::info!("[embedded-ble] device settings command acknowledged via USB serial");
                return Ok(());
            }
            Err(err) if err.is_firmware_rejection() => return Err(err.to_string()),
            Err(err) => {
                log::warn!(
                    "[embedded-ble] device settings USB serial path unavailable; trying BLE control: {err}"
                );
            }
        }

        if payload_len >= 64 {
            return Err(format!(
                "device settings command is too long for BLE control characteristic: {} bytes (max 63 including newline); USB serial fallback failed: {}",
                payload_len,
                serial_result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "not attempted".to_string())
            ));
        }

        let _fresh_guard = BleFreshGattGuard::enter("device settings")?;
        let target = open_audio_control_target_with_retry("device settings").map_err(|err| {
            format!(
                "{err}; active BLE fallback failed: {}; USB serial fallback failed: {}",
                active_capture_error.as_deref().unwrap_or("not attempted"),
                serial_result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "not attempted".to_string())
            )
        })?;
        write_gatt_value_with_timeout(
            &target.control,
            payload.as_bytes(),
            GattWriteOption::WriteWithResponse,
            timeout,
            "device settings",
        )
        .map_err(|err| {
            format!(
                "{err}; active BLE fallback failed: {}; USB serial fallback failed: {}",
                active_capture_error.as_deref().unwrap_or("not attempted"),
                serial_result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "not attempted".to_string())
            )
        })?;
        log::info!("[embedded-ble] device settings command sent");
        Ok(())
    }

    fn device_settings_command_allows_active_capture(command: &str) -> bool {
        let Some(arguments) = command.strip_prefix("DEVICE:SET ") else {
            return true;
        };
        !arguments
            .split_ascii_whitespace()
            .any(device_settings_token_updates_ble_name)
    }

    fn device_settings_token_updates_ble_name(token: &str) -> bool {
        token.starts_with("ble_name=") || token.starts_with("name=")
    }

    pub fn read_device_settings_status(
        timeout: Duration,
    ) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
        let line = exchange_device_settings_via_usb_serial("DEVICE:SETTINGS", timeout)
            .map_err(|err| format!("USB serial device settings refresh failed: {err}"))?;
        parse_device_settings_status_line(&line)
    }

    #[derive(Debug, Clone)]
    enum DeviceSettingsSerialError {
        Unavailable(String),
        Transport(String),
        FirmwareRejected(String),
    }

    impl DeviceSettingsSerialError {
        fn is_firmware_rejection(&self) -> bool {
            matches!(self, Self::FirmwareRejected(_))
        }
    }

    impl fmt::Display for DeviceSettingsSerialError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Unavailable(message)
                | Self::Transport(message)
                | Self::FirmwareRejected(message) => formatter.write_str(message),
            }
        }
    }

    fn send_device_settings_via_usb_serial(
        command: &str,
        timeout: Duration,
    ) -> Result<(), DeviceSettingsSerialError> {
        exchange_device_settings_via_usb_serial(command, timeout).map(|_| ())
    }

    fn send_control_command_via_usb_serial(
        command: &str,
        timeout: Duration,
    ) -> Result<(), DeviceSettingsSerialError> {
        let ports = serialport::available_ports().map_err(|err| {
            DeviceSettingsSerialError::Unavailable(format!(
                "USB serial port enumeration failed: {err}"
            ))
        })?;
        let candidates = listener_usb_serial_candidates(&ports);
        if candidates.is_empty() {
            return Err(DeviceSettingsSerialError::Unavailable(
                "no Listener USB serial port found".to_string(),
            ));
        }

        let mut errors = Vec::new();
        for port in candidates {
            match send_control_command_via_serial_port(&port.port_name, command, timeout) {
                Ok(()) => return Ok(()),
                Err(err) => errors.push(format!("{}: {err}", port.port_name)),
            }
        }

        Err(DeviceSettingsSerialError::Transport(format!(
            "all Listener USB serial candidates failed: {}",
            errors.join("; ")
        )))
    }

    fn exchange_device_settings_via_usb_serial(
        command: &str,
        timeout: Duration,
    ) -> Result<String, DeviceSettingsSerialError> {
        let ports = serialport::available_ports().map_err(|err| {
            DeviceSettingsSerialError::Unavailable(format!(
                "USB serial port enumeration failed: {err}"
            ))
        })?;
        let candidates = listener_usb_serial_candidates(&ports);
        if candidates.is_empty() {
            return Err(DeviceSettingsSerialError::Unavailable(
                "no Listener USB serial port found".to_string(),
            ));
        }

        let mut errors = Vec::new();
        for port in candidates {
            match exchange_device_settings_via_serial_port(&port.port_name, command, timeout) {
                Ok(line) => return Ok(line),
                Err(err) if err.is_firmware_rejection() => return Err(err),
                Err(err) => errors.push(format!("{}: {err}", port.port_name)),
            }
        }

        Err(DeviceSettingsSerialError::Transport(format!(
            "all Listener USB serial candidates failed: {}",
            errors.join("; ")
        )))
    }

    pub(super) fn listener_usb_serial_candidates(ports: &[SerialPortInfo]) -> Vec<SerialPortInfo> {
        let mut candidates: Vec<SerialPortInfo> = ports
            .iter()
            .filter(|port| is_listener_usb_serial_candidate(port))
            .cloned()
            .collect();
        candidates.sort_by(|left, right| left.port_name.cmp(&right.port_name));
        candidates
    }

    fn is_listener_usb_serial_candidate(port: &SerialPortInfo) -> bool {
        let SerialPortType::UsbPort(usb) = &port.port_type else {
            return false;
        };
        let text = listener_usb_serial_identity_text(port, usb);
        if text.contains("stlink")
            || text.contains("st-link")
            || text.contains("stmicroelectronics")
            || text.contains("nucleo")
            || text.contains("wb55")
        {
            return false;
        }
        if usb.vid == 0x303a {
            return true;
        }
        if usb.vid == 0x10c4 || usb.vid == 0x1a86 {
            return text.contains("listener")
                || text.contains("espressif")
                || text.contains("esp32")
                || text.contains("usb jtag")
                || text.contains("usb-serial")
                || text.contains("usb serial")
                || text.contains("cp210")
                || text.contains("ch340");
        }
        text.contains("listener")
            || text.contains("espressif")
            || text.contains("esp32")
            || text.contains("usb jtag")
            || text.contains("usb-serial")
            || text.contains("usb serial")
            || text.contains("cp210")
            || text.contains("ch340")
    }

    fn listener_usb_serial_identity_text(
        port: &SerialPortInfo,
        usb: &serialport::UsbPortInfo,
    ) -> String {
        format!(
            "{} {} {} {}",
            port.port_name,
            usb.manufacturer.as_deref().unwrap_or_default(),
            usb.product.as_deref().unwrap_or_default(),
            usb.serial_number.as_deref().unwrap_or_default()
        )
        .to_ascii_lowercase()
    }

    fn exchange_device_settings_via_serial_port(
        port_name: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<String, DeviceSettingsSerialError> {
        let serial_timeout = Duration::from_millis(120);
        let mut port = serialport::new(port_name, DEVICE_SETTINGS_SERIAL_BAUD_RATE)
            .dtr_on_open(false)
            .timeout(serial_timeout)
            .open()
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("open failed: {err}")))?;
        let _ = port.write_data_terminal_ready(false);
        let _ = port.write_request_to_send(false);

        drain_serial_input(&mut *port, Duration::from_millis(180));
        let payload = format!("~{command}\n");
        port.write_all(payload.as_bytes())
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("write failed: {err}")))?;
        port.flush()
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("flush failed: {err}")))?;

        let deadline = Instant::now() + timeout.max(Duration::from_secs(2));
        let mut response = String::new();
        let mut read_buf = [0_u8; DEVICE_SETTINGS_SERIAL_READ_CHUNK_BYTES];
        while Instant::now() < deadline {
            match port.read(&mut read_buf) {
                Ok(count) if count > 0 => {
                    response.push_str(&String::from_utf8_lossy(&read_buf[..count]));
                    if let Some(line) = response.lines().find(|line| line.contains("~DEVICE:ERROR"))
                    {
                        return Err(DeviceSettingsSerialError::FirmwareRejected(format!(
                            "firmware rejected device settings command: {line}"
                        )));
                    }
                    if let Some(line) = complete_device_settings_ok_line(&response) {
                        return Ok(line);
                    }
                }
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {}
                Err(err) => {
                    return Err(DeviceSettingsSerialError::Transport(format!(
                        "read failed: {err}"
                    )));
                }
            }
        }

        let tail = response_tail(&response, 320);
        Err(DeviceSettingsSerialError::Transport(format!(
            "timed out waiting for ~DEVICE:SETTINGS result=OK; received={tail:?}"
        )))
    }

    fn send_control_command_via_serial_port(
        port_name: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<(), DeviceSettingsSerialError> {
        let serial_timeout = Duration::from_millis(120);
        let mut port = serialport::new(port_name, DEVICE_SETTINGS_SERIAL_BAUD_RATE)
            .dtr_on_open(false)
            .timeout(serial_timeout)
            .open()
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("open failed: {err}")))?;
        let _ = port.write_data_terminal_ready(false);
        let _ = port.write_request_to_send(false);

        drain_serial_input(&mut *port, Duration::from_millis(180));
        let payload = format!("~{command}\n");
        port.write_all(payload.as_bytes())
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("write failed: {err}")))?;
        port.flush()
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("flush failed: {err}")))?;
        std::thread::sleep(timeout.min(Duration::from_millis(500)));
        Ok(())
    }

    fn drain_serial_input(port: &mut dyn serialport::SerialPort, duration: Duration) {
        let deadline = Instant::now() + duration;
        let mut read_buf = [0_u8; DEVICE_SETTINGS_SERIAL_READ_CHUNK_BYTES];
        while Instant::now() < deadline {
            match port.read(&mut read_buf) {
                Ok(0) => {}
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
    }

    fn response_tail(response: &str, max_chars: usize) -> String {
        let mut tail: Vec<char> = response.chars().rev().take(max_chars).collect();
        tail.reverse();
        tail.into_iter().collect()
    }

    fn complete_device_settings_ok_line(response: &str) -> Option<String> {
        let mut lines: Vec<&str> = response.split('\n').collect();
        if !response.ends_with('\n') {
            let _ = lines.pop();
        }
        lines
            .into_iter()
            .map(str::trim)
            .find(|line| line.contains("~DEVICE:SETTINGS") && line.contains(" result=OK"))
            .map(ToString::to_string)
    }

    pub(super) fn parse_device_settings_status_line(
        line: &str,
    ) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
        if !line.contains("~DEVICE:SETTINGS") {
            return Err(format!(
                "device settings refresh returned unexpected line: {line}"
            ));
        }
        let fields = parse_device_settings_fields(line);
        let led_zone_brightness_supported = fields.contains_key("led_status")
            || fields.contains_key("led_key")
            || fields.contains_key("led_ec11")
            || fields.contains_key("led_edge");
        let default_zone_brightness = crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT;
        let status_led_brightness_percent =
            optional_u8_field(&fields, "led_status").unwrap_or(default_zone_brightness);
        let key_led_brightness_percent =
            optional_u8_field(&fields, "led_key").unwrap_or(default_zone_brightness);
        let knob_led_brightness_percent =
            optional_u8_field(&fields, "led_ec11").unwrap_or(default_zone_brightness);
        let edge_led_brightness_percent =
            optional_u8_field(&fields, "led_edge").unwrap_or(default_zone_brightness);
        let legacy_low_power_idle_minutes = optional_u32_field(&fields, "low_power_idle_ms")
            .map(low_power_minutes_from_ms)
            .unwrap_or(crate::types::DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES);
        let plugged_low_power_idle_minutes =
            optional_u32_field(&fields, "plugged_low_power_idle_ms")
                .or_else(|| optional_minutes_field(&fields, "plugged_low_power_idle_minutes"))
                .map(low_power_minutes_from_ms)
                .unwrap_or(legacy_low_power_idle_minutes);
        let battery_low_power_idle_minutes =
            optional_u32_field(&fields, "battery_low_power_idle_ms")
                .or_else(|| optional_minutes_field(&fields, "battery_low_power_idle_minutes"))
                .map(low_power_minutes_from_ms)
                .unwrap_or(legacy_low_power_idle_minutes);
        let plugged_low_power_enabled =
            optional_bool_field(&fields, "plugged_low_power_enabled").unwrap_or(true);
        let legacy_auto_shutdown_minutes = optional_u32_field(&fields, "auto_shutdown_ms")
            .map(auto_shutdown_minutes_from_ms)
            .unwrap_or(crate::types::DEFAULT_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES);
        let plugged_auto_shutdown_minutes = optional_u32_field(&fields, "plugged_auto_shutdown_ms")
            .or_else(|| optional_minutes_field(&fields, "plugged_auto_shutdown_minutes"))
            .map(auto_shutdown_minutes_from_ms)
            .unwrap_or(0);
        let battery_auto_shutdown_minutes = optional_u32_field(&fields, "battery_auto_shutdown_ms")
            .or_else(|| optional_minutes_field(&fields, "battery_auto_shutdown_minutes"))
            .map(auto_shutdown_minutes_from_ms)
            .unwrap_or(legacy_auto_shutdown_minutes);
        let low_power_idle_minutes =
            if require_field(&fields, "active_power").unwrap_or("battery") == "external" {
                plugged_low_power_idle_minutes
            } else {
                battery_low_power_idle_minutes
            };
        Ok(crate::embedded_ble::DeviceSettingsStatus {
            status_led_brightness_percent,
            key_led_brightness_percent,
            knob_led_brightness_percent,
            edge_led_brightness_percent,
            led_zone_brightness_supported,
            low_power_idle_minutes,
            plugged_low_power_idle_minutes,
            battery_low_power_idle_minutes,
            plugged_low_power_enabled,
            plugged_auto_shutdown_minutes,
            battery_auto_shutdown_minutes,
            knob_rotation_action: require_field(&fields, "knob_rotation")?.to_string(),
            ble_name: require_field(&fields, "ble_name")?.to_string(),
            ble_name_pending_restart: parse_bool_field(&fields, "ble_name_pending")?,
            external_power_present: parse_bool_field(&fields, "external_power_present")?,
            usb_power_present: parse_bool_field(&fields, "usb_power_present")?,
            charging: parse_bool_field(&fields, "charging")?,
            charge_full: parse_bool_field(&fields, "charge_full")?,
            raw_line: line.trim().to_string(),
        })
    }

    fn parse_device_settings_fields(line: &str) -> std::collections::HashMap<String, String> {
        let mut fields = std::collections::HashMap::new();
        let mut cursor = 0;
        let chars: Vec<char> = line.chars().collect();
        while cursor < chars.len() {
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            let key_start = cursor;
            while cursor < chars.len()
                && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '_')
            {
                cursor += 1;
            }
            if key_start == cursor || cursor >= chars.len() || chars[cursor] != '=' {
                cursor += 1;
                continue;
            }
            let key: String = chars[key_start..cursor].iter().collect();
            cursor += 1;
            let value = if cursor < chars.len() && chars[cursor] == '"' {
                cursor += 1;
                let value_start = cursor;
                while cursor < chars.len() && chars[cursor] != '"' {
                    cursor += 1;
                }
                let value: String = chars[value_start..cursor].iter().collect();
                if cursor < chars.len() {
                    cursor += 1;
                }
                value
            } else {
                let value_start = cursor;
                while cursor < chars.len() && !chars[cursor].is_whitespace() {
                    cursor += 1;
                }
                chars[value_start..cursor].iter().collect()
            };
            fields.insert(key, value);
        }
        fields
    }

    fn require_field<'a>(
        fields: &'a std::collections::HashMap<String, String>,
        key: &str,
    ) -> Result<&'a str, String> {
        fields
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("device settings status missing {key}"))
    }

    fn parse_u32_field(
        fields: &std::collections::HashMap<String, String>,
        key: &str,
    ) -> Result<u32, String> {
        require_field(fields, key)?
            .parse::<u32>()
            .map_err(|err| format!("device settings field {key} is not u32: {err}"))
    }

    fn optional_u8_field(
        fields: &std::collections::HashMap<String, String>,
        key: &str,
    ) -> Option<u8> {
        fields.get(key)?.parse::<u8>().ok()
    }

    fn optional_u32_field(
        fields: &std::collections::HashMap<String, String>,
        key: &str,
    ) -> Option<u32> {
        fields.get(key)?.parse::<u32>().ok()
    }

    fn optional_minutes_field(
        fields: &std::collections::HashMap<String, String>,
        key: &str,
    ) -> Option<u32> {
        fields
            .get(key)?
            .parse::<u32>()
            .ok()
            .map(|minutes| minutes.saturating_mul(60_000))
    }

    fn low_power_minutes_from_ms(ms: u32) -> u32 {
        minutes_from_ms_nearest(ms, crate::types::MAX_DEVICE_LOW_POWER_IDLE_MINUTES)
    }

    fn auto_shutdown_minutes_from_ms(ms: u32) -> u32 {
        minutes_from_ms_nearest(ms, crate::types::MAX_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES)
    }

    fn minutes_from_ms_nearest(ms: u32, max_minutes: u32) -> u32 {
        if ms == 0 {
            0
        } else {
            ms.saturating_add(30_000)
                .checked_div(60_000)
                .unwrap_or(0)
                .clamp(1, max_minutes)
        }
    }

    fn parse_bool_field(
        fields: &std::collections::HashMap<String, String>,
        key: &str,
    ) -> Result<bool, String> {
        match require_field(fields, key)? {
            "0" => Ok(false),
            "1" => Ok(true),
            value => Err(format!("device settings field {key} is not bool: {value}")),
        }
    }

    fn optional_bool_field(
        fields: &std::collections::HashMap<String, String>,
        key: &str,
    ) -> Option<bool> {
        match fields.get(key)?.as_str() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        }
    }

    fn send_audio_control_via_active_capture(
        bytes: &[u8],
        timeout: Duration,
        label: &str,
    ) -> Option<Result<(), String>> {
        let active = active_audio_control_sender()?;
        let (result_tx, result_rx) = mpsc::channel();
        let request = AudioControlRequest {
            bytes: bytes.to_vec(),
            label: label.to_string(),
            timeout,
            result_tx,
        };
        if active
            .tx
            .send(BleCaptureSignal::AudioControl(request))
            .is_err()
        {
            clear_active_audio_control_sender(active.capture_id);
            return None;
        }
        let result = result_rx
            .recv_timeout(timeout + Duration::from_secs(1))
            .unwrap_or_else(|_| {
                Err(format!(
                    "active Listener BLE audio control timed out after {} ms",
                    timeout.as_millis()
                ))
            });
        if let Err(err) = &result {
            if is_transient_audio_control_write_error(err) {
                clear_active_audio_control_sender(active.capture_id);
            }
        }
        Some(result)
    }

    pub fn capture_notification_events(
        timeout: Duration,
        on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
    ) -> Result<(), String> {
        let mut on_ready = || Ok(());
        capture_notification_events_until_cancelled(
            Some(timeout),
            Arc::new(AtomicBool::new(false)),
            &mut on_ready,
            on_event,
        )
    }

    pub fn capture_notification_events_until_cancelled(
        idle_timeout: Option<Duration>,
        cancel_requested: Arc<AtomicBool>,
        on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
        on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
    ) -> Result<(), String> {
        capture_notification_events_until_cancelled_impl(
            idle_timeout,
            cancel_requested,
            on_ready,
            on_event,
            CaptureTerminalBehavior::StopCapture,
        )
    }

    pub fn capture_notification_events_continuous_until_cancelled(
        idle_timeout: Option<Duration>,
        cancel_requested: Arc<AtomicBool>,
        on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
        on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
    ) -> Result<(), String> {
        capture_notification_events_until_cancelled_impl(
            idle_timeout,
            cancel_requested,
            on_ready,
            on_event,
            CaptureTerminalBehavior::ContinueListening,
        )
    }

    fn capture_notification_events_until_cancelled_impl(
        idle_timeout: Option<Duration>,
        cancel_requested: Arc<AtomicBool>,
        on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
        on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
        terminal_behavior: CaptureTerminalBehavior,
    ) -> Result<(), String> {
        let capture_guard = BleCaptureGuard::enter(idle_timeout)?;
        let capture_id = capture_guard.session_id();
        let target = open_notify_target_with_retry(capture_id)?;
        let characteristic = target.characteristic.clone();
        let (tx, rx) = mpsc::channel::<BleCaptureSignal>();
        let notification_tx = tx.clone();
        let notification_log_count = Arc::new(AtomicUsize::new(0));
        let notification_log_count_for_handler = Arc::clone(&notification_log_count);
        let handler = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
            move |_sender, args| {
                if let Some(args) = args {
                    if let Ok(buffer) = args.CharacteristicValue() {
                        if let Ok(bytes) = buffer_to_vec(&buffer) {
                            let log_index =
                                notification_log_count_for_handler.fetch_add(1, Ordering::Relaxed);
                            if log_index < CAPTURE_NOTIFICATION_INFO_LOG_LIMIT {
                                let prefix_len = bytes.len().min(4);
                                log::info!(
                                    "[embedded-ble] capture #{capture_id}: notification #{} bytes={} prefix={:02X?}",
                                    log_index + 1,
                                    bytes.len(),
                                    &bytes[..prefix_len]
                                );
                            }
                            let _ = notification_tx.send(BleCaptureSignal::Notification(bytes));
                        }
                    }
                }
                Ok(())
            },
        );

        let mut cleanup = NotifyCleanup::new(capture_id, target);
        if cleanup.target.control.is_some() {
            cleanup.set_audio_control_registration(ActiveAudioControlRegistration::install(
                capture_id,
                tx.clone(),
            ));
        } else {
            log::warn!(
                "[embedded-ble] capture #{capture_id}: audio control unavailable for active capture"
            );
        }
        let token = characteristic
            .ValueChanged(&handler)
            .map_err(|err| format!("BLE ValueChanged handler registration failed: {err}"))?;
        cleanup.set_token(token);
        log::info!("[embedded-ble] capture #{capture_id}: ValueChanged handler registered");
        let connection_token = cleanup.target.device.as_ref().and_then(|device| {
            register_device_connection_status_handler(capture_id, device, tx.clone())
        });
        if let Some(token) = connection_token {
            cleanup.set_connection_status_token(token);
        }
        let session_token = cleanup.target.session.as_ref().and_then(|session| {
            register_gatt_session_status_handler(capture_id, session, tx.clone())
        });
        if let Some(token) = session_token {
            cleanup.set_session_status_token(token);
        }
        #[cfg(debug_assertions)]
        register_validation_disconnect_injection_handler(
            capture_id,
            tx.clone(),
            Arc::clone(&cancel_requested),
        );

        log::info!("[embedded-ble] capture #{capture_id}: resetting notify CCCD before enable");
        match write_cccd_with_timeout(
            &characteristic,
            GattClientCharacteristicConfigurationDescriptorValue::None,
            Duration::from_secs(2),
        ) {
            Ok(status) => {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: notify CCCD reset status={status:?}"
                )
            }
            Err(err) => {
                log::warn!("[embedded-ble] capture #{capture_id}: notify CCCD reset skipped: {err}")
            }
        }
        std::thread::sleep(Duration::from_millis(150));
        log::info!("[embedded-ble] capture #{capture_id}: enabling notify CCCD");
        let status = write_cccd_notify_with_retry(
            capture_id,
            "capture",
            &characteristic,
            CCCD_ENABLE_TIMEOUT,
        )?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("BLE CCCD notify write returned status={status:?}"));
        }
        log::info!("[embedded-ble] capture #{capture_id}: notify CCCD enabled");
        on_ready()?;
        let type_heartbeat_enabled =
            type_heartbeat_enabled_for_terminal_behavior(terminal_behavior);
        let mut next_type_heartbeat = None;
        let type_ready_command = type_ready_command_bytes();
        match cleanup.write_type_heartbeat(&type_ready_command, "Type heartbeat ready") {
            Ok(()) => {
                cleanup.mark_type_heartbeat_open();
                if type_heartbeat_enabled {
                    next_type_heartbeat = Some(Instant::now() + TYPE_HEARTBEAT_INTERVAL);
                }
            }
            Err(err) => {
                if type_heartbeat_enabled {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: Type heartbeat ready failed; keeping notify open for retry: {err}"
                    );
                    next_type_heartbeat = Some(Instant::now() + TYPE_HEARTBEAT_INTERVAL);
                } else {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: Type ready failed; waiting for audio notifications anyway: {err}"
                    );
                }
            }
        }

        let deadline = idle_timeout.map(|timeout| Instant::now() + timeout);
        let mut collector = crate::embedded_audio::SessionCollector::default();
        let mut stop_drain_deadline: Option<Instant> = None;
        let mut link_recovery_deadline: Option<Instant> = None;
        let mut link_recovery_reason: Option<String> = None;
        let mut consecutive_type_heartbeat_failures = 0u32;
        loop {
            let now = Instant::now();
            if let Some(due) = next_type_heartbeat {
                if now >= due {
                    if let Err(err) = cleanup.write_type_heartbeat(b"TYPE:HB\n", "Type heartbeat") {
                        consecutive_type_heartbeat_failures =
                            consecutive_type_heartbeat_failures.saturating_add(1);
                        let reason = format!("{err}; BLE audio/control response missing");
                        if collector_has_active_recoverable_session(&collector) {
                            let stats = collector.stats();
                            if link_recovery_deadline.is_none() {
                                link_recovery_deadline =
                                    Some(now + ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT);
                                link_recovery_reason = Some(reason.clone());
                                log::warn!(
                                    "[embedded-ble] capture #{capture_id}: {reason}; waiting for audio notify recovery (session_id={:?}, packets={}, timeout_ms={})",
                                    stats.session_id,
                                    stats.received_packet_count,
                                    ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                                );
                            } else {
                                log::warn!(
                                    "[embedded-ble] capture #{capture_id}: additional heartbeat failure while waiting for recovery: {reason}"
                                );
                            }
                        } else {
                            let log_message =
                                format!("[embedded-ble] capture #{capture_id}: {reason}; keeping idle notify open for heartbeat retry");
                            if consecutive_type_heartbeat_failures == 1 {
                                log::info!("{log_message}");
                            } else {
                                log::warn!(
                                    "{log_message}; consecutive_failures={consecutive_type_heartbeat_failures}"
                                );
                            }
                        }
                    } else {
                        if consecutive_type_heartbeat_failures > 0 {
                            log::info!(
                                "[embedded-ble] capture #{capture_id}: Type heartbeat recovered after {consecutive_type_heartbeat_failures} failure(s)"
                            );
                        }
                        consecutive_type_heartbeat_failures = 0;
                        cleanup.mark_type_heartbeat_open();
                    }
                    next_type_heartbeat = Some(now + TYPE_HEARTBEAT_INTERVAL);
                }
            }
            if cancel_requested.load(Ordering::SeqCst) {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: cancelled by caller; closing notify"
                );
                cleanup.disable_notify();
                return Ok(());
            }
            if deadline.is_some_and(|deadline| now >= deadline) {
                cleanup.log_embedded_audio_status_snapshot("capture timeout");
                return Err(format!(
                    "BLE embedded audio capture timed out after {} ms",
                    idle_timeout
                        .expect("deadline exists when timeout is reported")
                        .as_millis()
                ));
            }
            if stop_drain_deadline.is_some_and(|drain_deadline| now >= drain_deadline) {
                let stats = collector.stats();
                let reason = super::stop_drain_timeout_reason(&stats);
                log::warn!("[embedded-ble] {reason}");
                cleanup.disable_notify();
                return if collector.has_stopped_with_audio() {
                    Ok(())
                } else {
                    Err(reason)
                };
            }
            if link_recovery_deadline.is_some_and(|recovery_deadline| now >= recovery_deadline) {
                let reason = link_recovery_reason
                    .as_deref()
                    .unwrap_or("BLE link recovery timed out");
                let message = format!(
                    "BLE embedded audio capture link recovery timed out after {} ms: {reason}",
                    ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                );
                log::warn!("[embedded-ble] capture #{capture_id}: {message}");
                cleanup.disable_notify();
                return Err(message);
            }
            let receive_timeout = [stop_drain_deadline, deadline, link_recovery_deadline]
                .into_iter()
                .flatten()
                .chain(next_type_heartbeat)
                .map(|deadline| deadline.saturating_duration_since(now))
                .min()
                .unwrap_or(RECEIVE_POLL_INTERVAL)
                .min(RECEIVE_POLL_INTERVAL);
            let signal = match rx.recv_timeout(receive_timeout) {
                Ok(signal) => signal,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if cancel_requested.load(Ordering::SeqCst) {
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: cancelled by caller; closing notify"
                        );
                        cleanup.disable_notify();
                        return Ok(());
                    }
                    if stop_drain_deadline.is_some_and(|drain_deadline| now >= drain_deadline) {
                        let stats = collector.stats();
                        let reason = super::stop_drain_timeout_reason(&stats);
                        log::warn!("[embedded-ble] {reason}");
                        cleanup.disable_notify();
                        return if collector.has_stopped_with_audio() {
                            Ok(())
                        } else {
                            Err(reason)
                        };
                    }
                    if link_recovery_deadline
                        .is_some_and(|recovery_deadline| now >= recovery_deadline)
                    {
                        let reason = link_recovery_reason
                            .as_deref()
                            .unwrap_or("BLE link recovery timed out");
                        let message = format!(
                            "BLE embedded audio capture link recovery timed out after {} ms: {reason}",
                            ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                        );
                        log::warn!("[embedded-ble] capture #{capture_id}: {message}");
                        cleanup.disable_notify();
                        return Err(message);
                    }
                    continue;
                }
                Err(err) => {
                    return Err(format!(
                        "BLE embedded audio notification wait failed: {err}"
                    ));
                }
            };
            let notification = match signal {
                BleCaptureSignal::Notification(notification) => {
                    if link_recovery_deadline.take().is_some() {
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: link recovered after active-session disconnect: {}",
                            link_recovery_reason
                                .take()
                                .unwrap_or_else(|| "unknown".to_string())
                        );
                    }
                    notification
                }
                BleCaptureSignal::Disconnected(reason) => {
                    if collector_has_active_recoverable_session(&collector) {
                        let stats = collector.stats();
                        if link_recovery_deadline.is_none() {
                            link_recovery_deadline =
                                Some(Instant::now() + ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT);
                            link_recovery_reason = Some(reason.clone());
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: {reason}; keeping notify open for active session recovery (session_id={:?}, packets={}, timeout_ms={})",
                                stats.session_id,
                                stats.received_packet_count,
                                ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                            );
                        } else {
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: additional active-session disconnect while waiting for recovery: {reason}"
                            );
                        }
                        continue;
                    }
                    log::warn!("[embedded-ble] capture #{capture_id}: {reason}");
                    cleanup.disable_notify();
                    return Err(reason);
                }
                BleCaptureSignal::AudioControl(request) => {
                    cleanup.handle_audio_control_request(request);
                    continue;
                }
            };
            let terminal = super::is_terminal_notification(&notification);
            let local_event = collector.handle_notification(&notification).ok();
            on_event(crate::embedded_ble::BleNotificationEvent {
                notification,
                terminal,
            })?;
            if matches!(
                local_event,
                Some(crate::embedded_audio::SessionEvent::Cancelled { .. })
                    | Some(crate::embedded_audio::SessionEvent::Error { .. })
            ) {
                cleanup.disable_notify();
                return Ok(());
            }
            if matches!(
                local_event,
                Some(crate::embedded_audio::SessionEvent::Stopped { .. })
            ) {
                stop_drain_deadline = Some(Instant::now() + super::STOP_DRAIN_TIMEOUT);
            }
            if collector.has_successful_complete_session() {
                if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                    let stats = collector.stats();
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: complete session received; keeping notify open for background listener (session_id={:?}, pcm_bytes={}, packets={})",
                        stats.session_id,
                        stats.received_pcm_bytes,
                        stats.received_packet_count
                    );
                    collector.reset();
                    stop_drain_deadline = None;
                    continue;
                }
                cleanup.defer_type_heartbeat_bye_until_processing_done();
                cleanup.disable_notify();
                return Ok(());
            }
            if collector.terminal_received() && stop_drain_deadline.is_some() {
                stop_drain_deadline = Some(Instant::now() + super::STOP_DRAIN_TIMEOUT);
            }
        }
    }

    pub(super) fn collector_has_active_recoverable_session(
        collector: &crate::embedded_audio::SessionCollector,
    ) -> bool {
        collector.session_id().is_some() && !collector.terminal_received()
    }

    fn type_heartbeat_enabled_for_terminal_behavior(
        terminal_behavior: CaptureTerminalBehavior,
    ) -> bool {
        // One-shot captures only need the initial TYPE:READY command. Repeating
        // TYPE:HB while the app is doing ASR/cleanup has proven to destabilize
        // Keep back-to-back hardware A1/A2 sessions from reusing a stale Windows BLE link.
        terminal_behavior == CaptureTerminalBehavior::ContinueListening
    }

    #[cfg(test)]
    pub(super) fn active_capture_recovery_timing_for_test() -> (Duration, Duration) {
        (
            TYPE_HEARTBEAT_INTERVAL,
            ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT,
        )
    }

    #[cfg(test)]
    pub(super) fn type_heartbeat_terminal_behavior_matrix_for_test() -> (bool, bool) {
        (
            type_heartbeat_enabled_for_terminal_behavior(CaptureTerminalBehavior::StopCapture),
            type_heartbeat_enabled_for_terminal_behavior(
                CaptureTerminalBehavior::ContinueListening,
            ),
        )
    }

    #[cfg(debug_assertions)]
    fn register_validation_disconnect_injection_handler(
        capture_id: u64,
        tx: mpsc::Sender<BleCaptureSignal>,
        cancel_requested: Arc<AtomicBool>,
    ) {
        let Ok(path) = std::env::var("LISTENER_TYPE_BLE_VALIDATION_DISCONNECT_SIGNAL_FILE") else {
            return;
        };
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        log::warn!(
            "[embedded-ble] capture #{capture_id}: validation disconnect injection armed path={path}"
        );
        let _ = std::thread::Builder::new()
            .name(format!("listener-ble-disconnect-inject-{capture_id}"))
            .spawn(move || {
                let path = std::path::PathBuf::from(path);
                while !cancel_requested.load(Ordering::SeqCst) {
                    if path.exists() {
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: validation disconnect injection triggered"
                        );
                        let _ = tx.send(BleCaptureSignal::Disconnected(
                            "BLE validation injected disconnect through notify wait; transport_not_ready".to_string(),
                        ));
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
    }

    fn register_device_connection_status_handler(
        capture_id: u64,
        device: &BluetoothLEDevice,
        tx: mpsc::Sender<BleCaptureSignal>,
    ) -> Option<EventRegistrationToken> {
        if let Ok(status) = device.ConnectionStatus() {
            log::info!("[embedded-ble] capture #{capture_id}: device connection status={status:?}");
            if status == BluetoothConnectionStatus::Disconnected {
                let _ = tx.send(BleCaptureSignal::Disconnected(format!(
                    "BLE device connection status changed to Disconnected before notify wait; transport_not_ready"
                )));
            }
        }

        let handler_tx = tx.clone();
        let handler = TypedEventHandler::<BluetoothLEDevice, IInspectable>::new(
            move |sender, _args| {
                if let Some(device) = sender {
                    match device.ConnectionStatus() {
                        Ok(status) => {
                            log::info!(
                                "[embedded-ble] capture #{capture_id}: device connection status changed to {status:?}"
                            );
                            if status == BluetoothConnectionStatus::Disconnected {
                                let _ = handler_tx.send(BleCaptureSignal::Disconnected(format!(
                                    "BLE device connection status changed to Disconnected; transport_not_ready"
                                )));
                            }
                        }
                        Err(err) => {
                            let _ = handler_tx.send(BleCaptureSignal::Disconnected(format!(
                                "BLE device connection status read failed after status change: {err}; transport_not_ready"
                            )));
                        }
                    }
                }
                Ok(())
            },
        );

        match device.ConnectionStatusChanged(&handler) {
            Ok(token) => {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: device connection status handler registered"
                );
                Some(token)
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: device connection status handler registration failed: {err}"
                );
                None
            }
        }
    }

    fn register_gatt_session_status_handler(
        capture_id: u64,
        session: &GattSession,
        tx: mpsc::Sender<BleCaptureSignal>,
    ) -> Option<EventRegistrationToken> {
        if let Ok(status) = session.SessionStatus() {
            log::info!("[embedded-ble] capture #{capture_id}: GATT session status={status:?}");
            if status != GattSessionStatus::Active {
                let _ = tx.send(BleCaptureSignal::Disconnected(format!(
                    "BLE GATT session status changed to {status:?} before notify wait; transport_not_ready"
                )));
            }
        }

        let handler_tx = tx.clone();
        let handler = TypedEventHandler::<GattSession, GattSessionStatusChangedEventArgs>::new(
            move |_sender, args| {
                if let Some(args) = args {
                    let status = args.Status().ok();
                    let error = args.Error().ok();
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: GATT session status changed status={status:?} error={error:?}"
                    );
                    if status.is_some_and(|status| status != GattSessionStatus::Active) {
                        let _ = handler_tx.send(BleCaptureSignal::Disconnected(format!(
                            "BLE GATT session status changed to {status:?} error={error:?}; transport_not_ready"
                        )));
                    }
                }
                Ok(())
            },
        );

        match session.SessionStatusChanged(&handler) {
            Ok(token) => {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: GATT session status handler registered"
                );
                Some(token)
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: GATT session status handler registration failed: {err}"
                );
                None
            }
        }
    }

    pub(super) struct PreparedFirmwareOtaTransfer {
        target: OpenOtaTarget,
        snapshot: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
        transfer_guard: BleCaptureGuard,
    }

    #[cfg(feature = "companion-dev")]
    pub(super) struct PreparedStm32wbStOtaTransfer {
        target: OpenStm32wbStOtaTarget,
        snapshot: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
        transfer_guard: BleCaptureGuard,
    }

    struct OpenOtaV2Target {
        control: GattCharacteristic,
        data: GattCharacteristic,
        status: GattCharacteristic,
        data_write_option: GattWriteOption,
        data_chunk_payload_bytes: usize,
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
        bluetooth_address: Option<u64>,
    }

    struct PreparedOtaV2Characteristics {
        control: GattCharacteristic,
        data: GattCharacteristic,
        status: GattCharacteristic,
        data_write_option: GattWriteOption,
        data_chunk_payload_bytes: usize,
        session: Option<GattSession>,
    }

    type OpenListenerOtaV2Target = OpenOtaV2Target;
    type PreparedListenerOtaV2Characteristics = PreparedOtaV2Characteristics;

    #[cfg(feature = "companion-dev")]
    pub(super) struct PreparedCompanionOtaV2Transfer {
        target: OpenOtaV2Target,
        snapshot: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
        transfer_guard: BleCaptureGuard,
    }

    pub(super) struct PreparedListenerOtaV2Transfer {
        target: OpenListenerOtaV2Target,
        snapshot: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
        transfer_guard: BleCaptureGuard,
    }

    impl PreparedFirmwareOtaTransfer {
        pub(super) fn snapshot(&self) -> &crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            &self.snapshot
        }

        pub(super) fn transfer(
            self,
            version: &str,
            firmware_sha256: &str,
            firmware_bytes: &[u8],
            manifest_chunk_bytes: usize,
            on_progress: Option<&dyn Fn(usize, usize)>,
        ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
            transfer_firmware_ota_to_target(
                &self.target,
                self.transfer_guard.session_id(),
                version,
                firmware_sha256,
                firmware_bytes,
                manifest_chunk_bytes,
                on_progress,
            )
        }
    }

    #[cfg(feature = "companion-dev")]
    impl PreparedStm32wbStOtaTransfer {
        pub(super) fn snapshot(&self) -> &crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            &self.snapshot
        }

        pub(super) fn transfer(
            self,
            firmware_bytes: &[u8],
            manifest_chunk_bytes: usize,
            on_progress: Option<&dyn Fn(usize, usize)>,
        ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
            transfer_stm32wb_st_ota_to_target(
                &self.target,
                self.transfer_guard.session_id(),
                firmware_bytes,
                manifest_chunk_bytes,
                on_progress,
            )
        }
    }

    #[cfg(feature = "companion-dev")]
    impl PreparedCompanionOtaV2Transfer {
        pub(super) fn snapshot(&self) -> &crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            &self.snapshot
        }

        pub(super) fn transfer(
            self,
            firmware_bytes: &[u8],
            manifest_chunk_bytes: usize,
            on_progress: Option<&dyn Fn(usize, usize)>,
        ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
            transfer_companion_ota_v2_to_target(
                &self.target,
                self.transfer_guard.session_id(),
                firmware_bytes,
                manifest_chunk_bytes,
                on_progress,
            )
        }
    }

    impl PreparedListenerOtaV2Transfer {
        pub(super) fn snapshot(&self) -> &crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            &self.snapshot
        }

        pub(super) fn transfer(
            self,
            firmware_bytes: &[u8],
            manifest_chunk_bytes: usize,
            on_progress: Option<&dyn Fn(usize, usize)>,
        ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
            transfer_listener_ota_v2_to_target(
                &self.target,
                self.transfer_guard.session_id(),
                firmware_bytes,
                manifest_chunk_bytes,
                on_progress,
            )
        }
    }

    pub(super) fn prepare_firmware_ota_transfer() -> Result<PreparedFirmwareOtaTransfer, String> {
        log::info!("[embedded-ble] OTA prepare: acquiring BLE capture guard");
        let transfer_guard = BleCaptureGuard::enter(None)?;
        log::info!("[embedded-ble] OTA prepare: discovering OTA service");
        let target = open_ota_target()?;
        log::info!("[embedded-ble] OTA prepare: reading device snapshot");
        let snapshot = firmware_ota_device_snapshot_from_target(&target);
        log::info!(
            "[embedded-ble] OTA prepare: ready (firmware={})",
            snapshot.firmware_version.as_deref().unwrap_or("unknown")
        );
        Ok(PreparedFirmwareOtaTransfer {
            target,
            snapshot,
            transfer_guard,
        })
    }

    #[cfg(feature = "companion-dev")]
    pub(super) fn prepare_stm32wb_st_ota_transfer() -> Result<PreparedStm32wbStOtaTransfer, String>
    {
        log::info!("[embedded-ble] STM32WB ST OTA prepare: acquiring BLE capture guard");
        let transfer_guard = BleCaptureGuard::enter(None)?;
        log::info!("[embedded-ble] STM32WB ST OTA prepare: discovering companion OTA service");
        let target = open_stm32wb_st_ota_target()?;
        let snapshot = stm32wb_st_ota_device_snapshot_from_target(&target);
        log::info!(
            "[embedded-ble] STM32WB ST OTA prepare: ready (detail={})",
            snapshot.detail.as_deref().unwrap_or("unknown")
        );
        Ok(PreparedStm32wbStOtaTransfer {
            target,
            snapshot,
            transfer_guard,
        })
    }

    #[cfg(feature = "companion-dev")]
    pub(super) fn prepare_companion_ota_v2_transfer(
    ) -> Result<PreparedCompanionOtaV2Transfer, String> {
        log::info!("[embedded-ble] Companion OTA v2 prepare: acquiring BLE capture guard");
        let transfer_guard = BleCaptureGuard::enter(None)?;
        log::info!("[embedded-ble] Companion OTA v2 prepare: discovering companion OTA v2 service");
        let target = open_companion_ota_v2_target()?;
        let snapshot = companion_ota_v2_device_snapshot_from_target(&target);
        log::info!(
            "[embedded-ble] Companion OTA v2 prepare: ready (detail={})",
            snapshot.detail.as_deref().unwrap_or("unknown")
        );
        Ok(PreparedCompanionOtaV2Transfer {
            target,
            snapshot,
            transfer_guard,
        })
    }

    pub(super) fn prepare_listener_ota_v2_transfer() -> Result<PreparedListenerOtaV2Transfer, String>
    {
        log::info!("[embedded-ble] Listener OTA v2 prepare: acquiring BLE capture guard");
        let transfer_guard = BleCaptureGuard::enter(None)?;
        log::info!("[embedded-ble] Listener OTA v2 prepare: discovering Listener OTA v2 service");
        let target = open_listener_ota_v2_target()?;
        let snapshot = listener_ota_v2_device_snapshot_from_target(&target);
        log::info!(
            "[embedded-ble] Listener OTA v2 prepare: ready (detail={})",
            snapshot.detail.as_deref().unwrap_or("unknown")
        );
        Ok(PreparedListenerOtaV2Transfer {
            target,
            snapshot,
            transfer_guard,
        })
    }

    pub fn transfer_firmware_ota(
        version: &str,
        firmware_sha256: &str,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let prepared = prepare_firmware_ota_transfer()?;
        prepared.transfer(
            version,
            firmware_sha256,
            firmware_bytes,
            manifest_chunk_bytes,
            on_progress,
        )
    }

    fn transfer_firmware_ota_to_target(
        target: &OpenOtaTarget,
        transfer_id: u64,
        version: &str,
        firmware_sha256: &str,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let data_chunk_bytes =
            ota_transfer_chunk_bytes(target.data_chunk_bytes, manifest_chunk_bytes)?;
        let inter_chunk_delay = ota_data_inter_chunk_delay()?;
        let total_chunks = firmware_bytes.len().div_ceil(data_chunk_bytes);
        log::info!(
            "[embedded-ble] ota #{transfer_id}: aborting previous OTA (if any) before begin"
        );
        let abort_cmd = "{\"op\":\"abort\"}\n".to_string();
        let _ = write_gatt_value_with_timeout(
            &target.control,
            abort_cmd.as_bytes(),
            GattWriteOption::WriteWithResponse,
            OTA_WRITE_TIMEOUT,
            "OTA control abort",
        );

        log::info!(
            "[embedded-ble] ota #{transfer_id}: writing begin version={version} size={} chunks={total_chunks}",
            firmware_bytes.len()
        );
        let begin = format!(
            "{{\"op\":\"begin\",\"version\":\"{}\",\"size\":{},\"sha256\":\"{}\"}}\n",
            json_escape(version),
            firmware_bytes.len(),
            json_escape(firmware_sha256)
        );
        write_gatt_value_with_timeout(
            &target.control,
            begin.as_bytes(),
            GattWriteOption::WriteWithResponse,
            OTA_WRITE_TIMEOUT,
            "OTA control begin",
        )?;

        let mut chunks_sent = 0usize;
        for chunk in firmware_bytes.chunks(data_chunk_bytes) {
            write_gatt_value_with_timeout(
                &target.data,
                chunk,
                target.data_write_option,
                OTA_WRITE_TIMEOUT,
                "OTA data",
            )?;
            if !inter_chunk_delay.is_zero() {
                std::thread::sleep(inter_chunk_delay);
            }
            chunks_sent += 1;
            let bytes_sent = (chunks_sent * data_chunk_bytes).min(firmware_bytes.len());
            if chunks_sent % 10 == 0 || bytes_sent == firmware_bytes.len() {
                log::info!(
                    "[embedded-ble] ota #{transfer_id}: progress {bytes_sent}/{} bytes ({chunks_sent}/{total_chunks} chunks)",
                    firmware_bytes.len()
                );
                if let Some(cb) = &on_progress {
                    cb(bytes_sent, firmware_bytes.len());
                }
            }
        }

        log::info!("[embedded-ble] ota #{transfer_id}: writing finish");
        let finish = format!(
            "{{\"op\":\"finish\",\"size\":{},\"sha256\":\"{}\"}}\n",
            firmware_bytes.len(),
            json_escape(firmware_sha256)
        );
        let finish_result = write_gatt_value_with_timeout(
            &target.control,
            finish.as_bytes(),
            GattWriteOption::WriteWithResponse,
            OTA_FINISH_WRITE_TIMEOUT,
            "OTA control finish",
        );
        if let Err(err) = finish_result {
            if is_ota_finish_reboot_handoff_error(&err) {
                log::warn!(
                    "[embedded-ble] ota #{transfer_id}: finish write reported reboot handoff after full payload; continuing to confirmation: {err}"
                );
            } else {
                return Err(err);
            }
        }
        log::info!(
            "[embedded-ble] ota #{transfer_id}: transferred {} bytes in {chunks_sent} chunks (chunk_bytes={data_chunk_bytes}, transport_limit={}, manifest_limit={})",
            firmware_bytes.len(),
            target.data_chunk_bytes,
            manifest_chunk_bytes
        );
        Ok(crate::embedded_ble::FirmwareOtaTransferStats {
            bytes_transferred: firmware_bytes.len(),
            chunks_sent,
            transport: "listener_ble_ota",
        })
    }

    pub(super) fn is_ota_finish_reboot_handoff_error(err: &str) -> bool {
        let lower = err.to_ascii_lowercase();
        lower.contains("0x800704c7")
            || lower.contains("0x800706ba")
            || lower.contains("transport_not_ready")
            || lower.contains("disconnected")
            || lower.contains("gattcommunicationstatus(3)")
            || lower.contains("service open async result failed")
            || (lower.contains("ota control finish") && lower.contains("timed out"))
    }

    #[cfg(feature = "companion-dev")]
    pub fn transfer_stm32wb_st_ota(
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let prepared = prepare_stm32wb_st_ota_transfer()?;
        prepared.transfer(firmware_bytes, manifest_chunk_bytes, on_progress)
    }

    #[cfg(feature = "companion-dev")]
    pub fn transfer_companion_ota_v2(
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let prepared = prepare_companion_ota_v2_transfer()?;
        prepared.transfer(firmware_bytes, manifest_chunk_bytes, on_progress)
    }

    pub fn transfer_listener_ota_v2(
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let prepared = prepare_listener_ota_v2_transfer()?;
        prepared.transfer(firmware_bytes, manifest_chunk_bytes, on_progress)
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct OtaV2Status {
        state: u8,
        last_error: u8,
        bytes_written: usize,
        expected_size: usize,
        chunk_payload_bytes: usize,
        window_chunks: usize,
        data_write_count: u32,
    }

    #[cfg(feature = "companion-dev")]
    fn transfer_companion_ota_v2_to_target(
        target: &OpenOtaV2Target,
        transfer_id: u64,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if manifest_chunk_bytes != OTA_V2_CHUNK_PAYLOAD_BYTES {
            return Err(format!(
                "Companion OTA v2 manifest chunk size must be {OTA_V2_CHUNK_PAYLOAD_BYTES} bytes, got {manifest_chunk_bytes}."
            ));
        }

        let configured_window = ota_v2_window_chunks()?;
        let begin_window = configured_window.min(u16::MAX as usize).max(1);
        let begin = ota_v2_control_command(
            OTA_V2_OP_BEGIN,
            firmware_bytes.len() as u32,
            manifest_chunk_bytes as u16,
            begin_window as u16,
        );

        log::info!(
            "[embedded-ble] Companion OTA v2 #{transfer_id}: aborting previous transfer (if any)"
        );
        let abort = ota_v2_control_command(OTA_V2_OP_ABORT, 0, 0, 0);
        let _ = write_gatt_value_with_timeout(
            &target.control,
            &abort,
            GattWriteOption::WriteWithResponse,
            OTA_WRITE_TIMEOUT,
            "Companion OTA v2 abort",
        );

        log::info!(
            "[embedded-ble] Companion OTA v2 #{transfer_id}: begin size={} chunk={} window={begin_window}",
            firmware_bytes.len(),
            manifest_chunk_bytes
        );
        write_gatt_value_with_timeout(
            &target.control,
            &begin,
            GattWriteOption::WriteWithResponse,
            OTA_WRITE_TIMEOUT,
            "Companion OTA v2 begin",
        )?;

        let mut status = wait_ota_v2_status(
            target,
            transfer_id,
            |status| {
                status.state == OTA_V2_STATE_RECEIVING
                    && status.expected_size == firmware_bytes.len()
            },
            "begin erase",
        )?;
        let mut window_chunks = status.window_chunks.max(1).min(configured_window.max(1));
        let chunk_payload_bytes = status
            .chunk_payload_bytes
            .max(1)
            .min(manifest_chunk_bytes)
            .min(target.data_chunk_payload_bytes);
        log::info!(
            "[embedded-ble] Companion OTA v2 #{transfer_id}: receiving offset={} device_chunk={} host_chunk={} window={window_chunks}",
            status.bytes_written,
            status.chunk_payload_bytes,
            chunk_payload_bytes
        );

        let mut chunks_sent = 0usize;
        let mut confirmed_offset = status.bytes_written.min(firmware_bytes.len());
        while confirmed_offset < firmware_bytes.len() {
            let mut sent_until = confirmed_offset;
            let mut chunks_this_window = 0usize;
            while chunks_this_window < window_chunks && sent_until < firmware_bytes.len() {
                let payload_len = chunk_payload_bytes.min(firmware_bytes.len() - sent_until);
                let mut packet = Vec::with_capacity(OTA_V2_PACKET_HEADER_BYTES + payload_len);
                packet.extend_from_slice(&(sent_until as u32).to_le_bytes());
                packet.extend_from_slice(&firmware_bytes[sent_until..sent_until + payload_len]);
                write_gatt_value_with_timeout(
                    &target.data,
                    &packet,
                    target.data_write_option,
                    OTA_WRITE_TIMEOUT,
                    "Companion OTA v2 data",
                )?;
                sent_until += payload_len;
                chunks_sent += 1;
                chunks_this_window += 1;
            }

            let sync = ota_v2_control_command(
                OTA_V2_OP_SYNC,
                firmware_bytes.len() as u32,
                manifest_chunk_bytes as u16,
                window_chunks as u16,
            );
            write_gatt_value_with_timeout(
                &target.control,
                &sync,
                GattWriteOption::WriteWithResponse,
                OTA_WRITE_TIMEOUT,
                "Companion OTA v2 sync",
            )?;
            status = read_ota_v2_status(target)?;
            ota_v2_status_result(&status)?;
            if status.expected_size != firmware_bytes.len() {
                return Err(format!(
                    "Companion OTA v2 status expected_size={} does not match package size {}.",
                    status.expected_size,
                    firmware_bytes.len()
                ));
            }
            if status.bytes_written > sent_until {
                return Err(format!(
                    "Companion OTA v2 status offset {} advanced beyond host sent offset {sent_until}.",
                    status.bytes_written
                ));
            }
            if status.bytes_written < sent_until {
                log::warn!(
                    "[embedded-ble] Companion OTA v2 #{transfer_id}: device accepted {}/{} bytes in window; retrying from offset {} last_error={}",
                    status.bytes_written.saturating_sub(confirmed_offset),
                    sent_until.saturating_sub(confirmed_offset),
                    status.bytes_written,
                    status.last_error
                );
                if status.last_error == COMPANION_OTA_V2_ERROR_OFFSET_MISMATCH {
                    window_chunks = window_chunks.saturating_div(2).max(1);
                }
            } else if window_chunks < configured_window {
                window_chunks += 1;
            }

            confirmed_offset = status.bytes_written.min(firmware_bytes.len());
            if chunks_sent % 10 == 0 || confirmed_offset == firmware_bytes.len() {
                log::info!(
                    "[embedded-ble] Companion OTA v2 #{transfer_id}: progress {confirmed_offset}/{} bytes (chunks_sent={chunks_sent}, window={window_chunks})",
                    firmware_bytes.len()
                );
                if let Some(cb) = &on_progress {
                    cb(confirmed_offset, firmware_bytes.len());
                }
            }
        }

        let finish = ota_v2_control_command(
            OTA_V2_OP_FINISH,
            firmware_bytes.len() as u32,
            manifest_chunk_bytes as u16,
            window_chunks as u16,
        );
        log::info!("[embedded-ble] Companion OTA v2 #{transfer_id}: writing finish");
        let finish_result = write_gatt_value_with_timeout(
            &target.control,
            &finish,
            GattWriteOption::WriteWithResponse,
            OTA_FINISH_WRITE_TIMEOUT,
            "Companion OTA v2 finish",
        );
        if let Err(err) = finish_result {
            if is_ota_finish_reboot_handoff_error(&err)
                || is_stm32wb_st_ota_finish_reboot_handoff_error(&err)
            {
                log::warn!(
                    "[embedded-ble] Companion OTA v2 #{transfer_id}: finish write reported reboot handoff after full payload; accepting completion: {err}"
                );
            } else {
                return Err(err);
            }
        }
        log::info!(
            "[embedded-ble] Companion OTA v2 #{transfer_id}: transferred {} bytes in {chunks_sent} chunks (chunk_bytes={chunk_payload_bytes}, window_limit={configured_window})",
            firmware_bytes.len()
        );
        Ok(crate::embedded_ble::FirmwareOtaTransferStats {
            bytes_transferred: firmware_bytes.len(),
            chunks_sent,
            transport: "companion_ota_v2",
        })
    }

    fn transfer_listener_ota_v2_to_target(
        target: &OpenListenerOtaV2Target,
        transfer_id: u64,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if manifest_chunk_bytes != LISTENER_OTA_V2_CHUNK_PAYLOAD_BYTES {
            return Err(format!(
                "Listener OTA v2 manifest chunk size must be {LISTENER_OTA_V2_CHUNK_PAYLOAD_BYTES} bytes, got {manifest_chunk_bytes}."
            ));
        }

        let configured_window = listener_ota_v2_window_chunks()?;
        let begin_window = configured_window.min(u16::MAX as usize).max(1);
        let begin = listener_ota_v2_control_command(
            OTA_V2_OP_BEGIN,
            firmware_bytes.len() as u32,
            manifest_chunk_bytes as u16,
            begin_window as u16,
        );
        let mut data_write_option = target.data_write_option;

        log::info!(
            "[embedded-ble] Listener OTA v2 #{transfer_id}: aborting previous transfer (if any)"
        );
        let abort = listener_ota_v2_control_command(OTA_V2_OP_ABORT, 0, 0, 0);
        if let Ok(option) = write_listener_ota_v2_value_with_fallback(
            &target.data,
            &abort,
            data_write_option,
            OTA_WRITE_TIMEOUT,
            "Listener OTA v2 abort",
        ) {
            data_write_option = option;
        }

        log::info!(
            "[embedded-ble] Listener OTA v2 #{transfer_id}: begin size={} chunk={} window={begin_window}",
            firmware_bytes.len(),
            manifest_chunk_bytes
        );
        data_write_option = write_listener_ota_v2_value_with_fallback(
            &target.data,
            &begin,
            data_write_option,
            OTA_WRITE_TIMEOUT,
            "Listener OTA v2 begin",
        )?;

        let mut status = wait_listener_ota_v2_status(
            target,
            transfer_id,
            |status| {
                status.state == OTA_V2_STATE_RECEIVING
                    && status.expected_size == firmware_bytes.len()
            },
            "begin",
        )?;
        let mut window_chunks = status.window_chunks.max(1).min(configured_window.max(1));
        let chunk_payload_bytes = status
            .chunk_payload_bytes
            .max(1)
            .min(manifest_chunk_bytes)
            .min(target.data_chunk_payload_bytes);
        log::info!(
            "[embedded-ble] Listener OTA v2 #{transfer_id}: receiving offset={} device_chunk={} host_chunk={} window={window_chunks}",
            status.bytes_written,
            status.chunk_payload_bytes,
            chunk_payload_bytes
        );

        let mut chunks_sent = 0usize;
        let mut confirmed_offset = status.bytes_written.min(firmware_bytes.len());
        while confirmed_offset < firmware_bytes.len() {
            let mut sent_until = confirmed_offset;
            let mut chunks_this_window = 0usize;
            while chunks_this_window < window_chunks && sent_until < firmware_bytes.len() {
                let payload_len = chunk_payload_bytes.min(firmware_bytes.len() - sent_until);
                let mut packet = Vec::with_capacity(OTA_V2_PACKET_HEADER_BYTES + payload_len);
                packet.extend_from_slice(&(sent_until as u32).to_le_bytes());
                packet.extend_from_slice(&firmware_bytes[sent_until..sent_until + payload_len]);
                data_write_option = write_listener_ota_v2_value_with_fallback(
                    &target.data,
                    &packet,
                    data_write_option,
                    OTA_WRITE_TIMEOUT,
                    "Listener OTA v2 data",
                )?;
                sent_until += payload_len;
                chunks_sent += 1;
                chunks_this_window += 1;
            }

            let sync = listener_ota_v2_control_command(
                OTA_V2_OP_SYNC,
                firmware_bytes.len() as u32,
                manifest_chunk_bytes as u16,
                window_chunks as u16,
            );
            data_write_option = write_listener_ota_v2_value_with_fallback(
                &target.data,
                &sync,
                data_write_option,
                OTA_WRITE_TIMEOUT,
                "Listener OTA v2 sync",
            )?;
            status = read_listener_ota_v2_status(target)?;
            ota_v2_status_result(&status)?;
            if status.expected_size != firmware_bytes.len() {
                return Err(format!(
                    "Listener OTA v2 status expected_size={} does not match package size {}.",
                    status.expected_size,
                    firmware_bytes.len()
                ));
            }
            if status.bytes_written > sent_until {
                return Err(format!(
                    "Listener OTA v2 status offset {} advanced beyond host sent offset {sent_until}.",
                    status.bytes_written
                ));
            }
            if status.bytes_written < sent_until {
                log::warn!(
                    "[embedded-ble] Listener OTA v2 #{transfer_id}: device accepted {}/{} bytes in window; retrying from offset {} last_error={}",
                    status.bytes_written.saturating_sub(confirmed_offset),
                    sent_until.saturating_sub(confirmed_offset),
                    status.bytes_written,
                    status.last_error
                );
                if status.last_error == OTA_V2_ERROR_OFFSET_MISMATCH {
                    window_chunks = window_chunks.saturating_div(2).max(1);
                }
            } else if window_chunks < configured_window {
                window_chunks += 1;
            }

            confirmed_offset = status.bytes_written.min(firmware_bytes.len());
            if chunks_sent % 10 == 0 || confirmed_offset == firmware_bytes.len() {
                log::info!(
                    "[embedded-ble] Listener OTA v2 #{transfer_id}: progress {confirmed_offset}/{} bytes (chunks_sent={chunks_sent}, window={window_chunks})",
                    firmware_bytes.len()
                );
                if let Some(cb) = &on_progress {
                    cb(confirmed_offset, firmware_bytes.len());
                }
            }
        }

        let finish = listener_ota_v2_control_command(
            OTA_V2_OP_FINISH,
            firmware_bytes.len() as u32,
            manifest_chunk_bytes as u16,
            window_chunks as u16,
        );
        log::info!("[embedded-ble] Listener OTA v2 #{transfer_id}: writing finish");
        let finish_result = write_listener_ota_v2_value_with_fallback(
            &target.data,
            &finish,
            data_write_option,
            OTA_FINISH_WRITE_TIMEOUT,
            "Listener OTA v2 finish",
        );
        if let Err(err) = finish_result {
            if is_ota_finish_reboot_handoff_error(&err) {
                log::warn!(
                    "[embedded-ble] Listener OTA v2 #{transfer_id}: finish write reported reboot handoff after full payload; accepting completion: {err}"
                );
            } else {
                return Err(err);
            }
        }
        log::info!(
            "[embedded-ble] Listener OTA v2 #{transfer_id}: transferred {} bytes in {chunks_sent} chunks (chunk_bytes={chunk_payload_bytes}, window_limit={configured_window})",
            firmware_bytes.len()
        );
        Ok(crate::embedded_ble::FirmwareOtaTransferStats {
            bytes_transferred: firmware_bytes.len(),
            chunks_sent,
            transport: "listener_ble_ota_v2",
        })
    }

    #[cfg(feature = "companion-dev")]
    fn ota_v2_control_command(
        op: u8,
        expected_size: u32,
        chunk_payload_bytes: u16,
        window_chunks: u16,
    ) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(OTA_V2_MAGIC);
        bytes[4] = op;
        bytes[5] = OTA_V2_PROTOCOL_VERSION;
        bytes[8..12].copy_from_slice(&expected_size.to_le_bytes());
        bytes[12..14].copy_from_slice(&chunk_payload_bytes.to_le_bytes());
        bytes[14..16].copy_from_slice(&window_chunks.to_le_bytes());
        bytes
    }

    fn listener_ota_v2_control_command(
        op: u8,
        expected_size: u32,
        chunk_payload_bytes: u16,
        window_chunks: u16,
    ) -> Vec<u8> {
        let op_name = match op {
            OTA_V2_OP_BEGIN => "begin_v2",
            OTA_V2_OP_SYNC => "sync_v2",
            OTA_V2_OP_FINISH => "finish_v2",
            OTA_V2_OP_ABORT => "abort_v2",
            _ => "abort_v2",
        };
        format!(
            "{{\"op\":\"{op_name}\",\"size\":{expected_size},\"chunk\":{chunk_payload_bytes},\"window\":{window_chunks}}}\n"
        )
        .into_bytes()
    }

    #[cfg(feature = "companion-dev")]
    fn ota_v2_window_chunks() -> Result<usize, String> {
        let configured = std::env::var(OTA_V2_WINDOW_ENV)
            .ok()
            .and_then(ota_env_value);
        match configured.as_deref() {
            None => Ok(OTA_V2_DEFAULT_WINDOW_CHUNKS),
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|window| (1..=64).contains(window))
                .ok_or_else(|| {
                    format!(
                        "Unsupported {OTA_V2_WINDOW_ENV}={value}; use a window from 1 to 64 chunks."
                    )
                }),
        }
    }

    fn listener_ota_v2_window_chunks() -> Result<usize, String> {
        let configured = std::env::var(LISTENER_OTA_V2_WINDOW_ENV)
            .ok()
            .and_then(ota_env_value);
        match configured.as_deref() {
            None => Ok(LISTENER_OTA_V2_DEFAULT_WINDOW_CHUNKS),
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|window| (1..=64).contains(window))
                .ok_or_else(|| {
                    format!(
                        "Unsupported {LISTENER_OTA_V2_WINDOW_ENV}={value}; use a window from 1 to 64 chunks."
                    )
                }),
        }
    }

    #[cfg(feature = "companion-dev")]
    fn wait_ota_v2_status(
        target: &OpenOtaV2Target,
        transfer_id: u64,
        ready: impl Fn(&OtaV2Status) -> bool,
        label: &str,
    ) -> Result<OtaV2Status, String> {
        let deadline = Instant::now() + OTA_V2_STATUS_POLL_TIMEOUT;
        loop {
            let status = read_companion_ota_v2_status(target)?;
            ota_v2_status_result(&status)?;
            if ready(&status) {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Companion OTA v2 #{transfer_id} timed out waiting for {label}; last_status={status:?}"
                ));
            }
            std::thread::sleep(OTA_V2_STATUS_POLL_INTERVAL);
        }
    }

    #[cfg(feature = "companion-dev")]
    fn read_companion_ota_v2_status(target: &OpenOtaV2Target) -> Result<OtaV2Status, String> {
        read_ota_v2_status(target)
    }

    #[cfg(feature = "companion-dev")]
    fn read_ota_v2_status(target: &OpenOtaV2Target) -> Result<OtaV2Status, String> {
        let bytes = read_characteristic_bytes(
            &target.status,
            BluetoothCacheMode::Uncached,
            "Companion OTA v2 status",
        )?;
        parse_ota_v2_status(&bytes)
    }

    fn wait_listener_ota_v2_status(
        target: &OpenListenerOtaV2Target,
        transfer_id: u64,
        ready: impl Fn(&OtaV2Status) -> bool,
        label: &str,
    ) -> Result<OtaV2Status, String> {
        let deadline = Instant::now() + OTA_V2_STATUS_POLL_TIMEOUT;
        loop {
            let status = read_listener_ota_v2_status(target)?;
            ota_v2_status_result(&status)?;
            if ready(&status) {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Listener OTA v2 #{transfer_id} timed out waiting for {label}; last_status={status:?}"
                ));
            }
            std::thread::sleep(OTA_V2_STATUS_POLL_INTERVAL);
        }
    }

    fn read_listener_ota_v2_status(
        target: &OpenListenerOtaV2Target,
    ) -> Result<OtaV2Status, String> {
        let bytes = read_characteristic_bytes(
            &target.status,
            BluetoothCacheMode::Uncached,
            "Listener OTA v2 status",
        )?;
        parse_listener_ota_v2_status(&bytes)
    }

    #[cfg(feature = "companion-dev")]
    fn parse_ota_v2_status(bytes: &[u8]) -> Result<OtaV2Status, String> {
        if bytes.len() < OTA_V2_STATUS_BYTES {
            return Err(format!(
                "Companion OTA v2 status is {} bytes; expected {OTA_V2_STATUS_BYTES}.",
                bytes.len()
            ));
        }
        if &bytes[0..4] != OTA_V2_MAGIC {
            return Err("Companion OTA v2 status magic mismatch.".to_string());
        }
        if bytes[4] != OTA_V2_PROTOCOL_VERSION {
            return Err(format!(
                "Companion OTA v2 status protocol version {} is unsupported.",
                bytes[4]
            ));
        }
        Ok(OtaV2Status {
            state: bytes[5],
            last_error: bytes[6],
            bytes_written: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            expected_size: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]])
                as usize,
            chunk_payload_bytes: u16::from_le_bytes([bytes[16], bytes[17]]) as usize,
            window_chunks: u16::from_le_bytes([bytes[18], bytes[19]]) as usize,
            data_write_count: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        })
    }

    fn parse_listener_ota_v2_status(bytes: &[u8]) -> Result<OtaV2Status, String> {
        if bytes.len() < OTA_V2_STATUS_BYTES {
            return Err(format!(
                "Listener OTA v2 status is {} bytes; expected {OTA_V2_STATUS_BYTES}.",
                bytes.len()
            ));
        }
        if &bytes[0..4] != LISTENER_OTA_V2_MAGIC {
            return Err("Listener OTA v2 status magic mismatch.".to_string());
        }
        if bytes[4] != OTA_V2_PROTOCOL_VERSION {
            return Err(format!(
                "Listener OTA v2 status version {} is unsupported.",
                bytes[4]
            ));
        }
        Ok(OtaV2Status {
            state: bytes[5],
            last_error: bytes[6],
            bytes_written: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            expected_size: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]])
                as usize,
            chunk_payload_bytes: u16::from_le_bytes([bytes[16], bytes[17]]) as usize,
            window_chunks: u16::from_le_bytes([bytes[18], bytes[19]]) as usize,
            data_write_count: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        })
    }

    fn ota_v2_status_result(status: &OtaV2Status) -> Result<(), String> {
        if status.state == OTA_V2_STATE_ERROR {
            return Err(format!(
                "OTA v2 device entered error state last_error={} offset={} expected_size={} data_writes={}.",
                status.last_error,
                status.bytes_written,
                status.expected_size,
                status.data_write_count
            ));
        }
        if status.last_error != OTA_V2_ERROR_NONE
            && status.last_error != OTA_V2_ERROR_OFFSET_MISMATCH
        {
            return Err(format!(
                "OTA v2 device reported fatal last_error={} state={} offset={}.",
                status.last_error, status.state, status.bytes_written
            ));
        }
        Ok(())
    }

    #[cfg(feature = "companion-dev")]
    fn transfer_stm32wb_st_ota_to_target(
        target: &OpenStm32wbStOtaTarget,
        transfer_id: u64,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        let data_chunk_bytes =
            stm32wb_st_ota_transfer_chunk_bytes(target.raw_chunk_bytes, manifest_chunk_bytes)?;
        let total_chunks = firmware_bytes.len().div_ceil(data_chunk_bytes);
        let (confirm_tx, confirm_rx) = mpsc::channel::<Vec<u8>>();
        let mut cleanup = Stm32wbStOtaConfirmCleanup {
            characteristic: target.confirm.clone(),
            token: None,
            cccd_enabled: false,
        };
        let use_confirm = stm32wb_st_ota_use_confirm_indication();
        let confirm_enabled = if use_confirm {
            let handler = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
                move |_sender, args| {
                    if let Some(args) = args {
                        match args
                            .CharacteristicValue()
                            .map_err(|err| err.to_string())
                            .and_then(|buffer| {
                                buffer_to_vec(&buffer).map_err(|err| err.to_string())
                            }) {
                            Ok(bytes) => {
                                let _ = confirm_tx.send(bytes);
                            }
                            Err(err) => {
                                log::warn!(
                                    "[embedded-ble] STM32WB ST OTA confirm indication read failed: {err}"
                                );
                            }
                        }
                    }
                    Ok(())
                },
            );
            let token = target.confirm.ValueChanged(&handler).map_err(|err| {
                format!("STM32WB ST OTA confirm ValueChanged registration failed: {err}")
            })?;
            cleanup.token = Some(token);
            let confirm_cccd_result = write_cccd_indicate_with_retry(
                transfer_id,
                "STM32WB ST OTA confirm",
                &target.confirm,
                CCCD_ENABLE_TIMEOUT,
            )
            .and_then(|status| {
                if status == GattCommunicationStatus::Success {
                    Ok(status)
                } else {
                    Err(format!(
                        "STM32WB ST OTA confirm indication CCCD returned status={status:?}"
                    ))
                }
            });
            match confirm_cccd_result {
                Ok(_) => {
                    cleanup.cccd_enabled = true;
                    true
                }
                Err(err) => {
                    return Err(format!(
                        "STM32WB ST OTA confirmation indication is required before transfer; CCCD enable failed: {err}. Set COMPANION_STM32WB_ST_OTA_CONFIRM=0 only when testing the legacy no-confirm loader."
                    ));
                }
            }
        } else {
            log::info!(
                "[embedded-ble] STM32WB ST OTA #{transfer_id}: using explicit no-confirm reboot handoff debug path because COMPANION_STM32WB_ST_OTA_CONFIRM=0"
            );
            drop(confirm_tx);
            false
        };

        if confirm_enabled {
            log::info!(
                "[embedded-ble] STM32WB ST OTA #{transfer_id}: ST reboot confirmation indication enabled"
            );
        } else {
            log::info!(
                "[embedded-ble] STM32WB ST OTA #{transfer_id}: transfer will verify completion by reboot handoff"
            );
        }

        let base_address = STM32WB_ST_OTA_APP_BASE_ADDRESS;
        let begin = stm32wb_st_ota_base_address_command(base_address);
        log::info!(
            "[embedded-ble] STM32WB ST OTA #{transfer_id}: begin base=0x{base_address:08x} size={} chunks={total_chunks}",
            firmware_bytes.len()
        );
        write_stm32wb_st_ota_value_with_fallback(
            &target.base,
            &begin,
            target.base_write_option,
            OTA_WRITE_TIMEOUT,
            "STM32WB ST OTA base address",
        )?;

        let mut chunks_sent = 0usize;
        let mut raw_write_option = target.raw_write_option;
        for chunk in firmware_bytes.chunks(data_chunk_bytes) {
            raw_write_option = write_stm32wb_st_ota_value_with_fallback(
                &target.raw,
                chunk,
                raw_write_option,
                OTA_WRITE_TIMEOUT,
                "STM32WB ST OTA raw data",
            )?;
            if raw_write_option == GattWriteOption::WriteWithoutResponse {
                std::thread::sleep(STM32WB_ST_OTA_NO_RESPONSE_CHUNK_DELAY);
            }
            chunks_sent += 1;
            let bytes_sent = (chunks_sent * data_chunk_bytes).min(firmware_bytes.len());
            if chunks_sent % 10 == 0 || bytes_sent == firmware_bytes.len() {
                log::info!(
                    "[embedded-ble] STM32WB ST OTA #{transfer_id}: progress {bytes_sent}/{} bytes ({chunks_sent}/{total_chunks} chunks)",
                    firmware_bytes.len()
                );
                if let Some(cb) = &on_progress {
                    cb(bytes_sent, firmware_bytes.len());
                }
            }
        }

        let (handoff_tx, handoff_rx) = mpsc::channel::<String>();
        let mut handoff_cleanup =
            Stm32wbStOtaHandoffCleanup::install(transfer_id, target, handoff_tx);

        let finish = [STM32WB_ST_OTA_UPLOAD_FINISHED, 0x00, 0x00, 0x00];
        log::info!("[embedded-ble] STM32WB ST OTA #{transfer_id}: writing finish");
        let finish_result = write_stm32wb_st_ota_value_with_fallback(
            &target.base,
            &finish,
            target.base_write_option,
            OTA_WRITE_TIMEOUT,
            "STM32WB ST OTA finish",
        );
        if let Err(err) = finish_result {
            if is_stm32wb_st_ota_finish_reboot_handoff_error(&err) {
                log::warn!(
                    "[embedded-ble] STM32WB ST OTA #{transfer_id}: finish write reported reboot handoff after full payload; continuing to confirmation/handoff: {err}"
                );
            } else {
                return Err(err);
            }
        }
        wait_stm32wb_st_ota_completion(
            transfer_id,
            target,
            confirm_enabled,
            &confirm_rx,
            &handoff_rx,
        )?;
        handoff_cleanup.finish();
        cleanup.finish();
        log::info!(
            "[embedded-ble] STM32WB ST OTA #{transfer_id}: transferred {} bytes in {chunks_sent} chunks (chunk_bytes={data_chunk_bytes})",
            firmware_bytes.len()
        );
        Ok(crate::embedded_ble::FirmwareOtaTransferStats {
            bytes_transferred: firmware_bytes.len(),
            chunks_sent,
            transport: "stm32wb_st_ble_ota",
        })
    }

    #[cfg(feature = "companion-dev")]
    fn write_stm32wb_st_ota_value_with_fallback(
        characteristic: &GattCharacteristic,
        bytes: &[u8],
        primary: GattWriteOption,
        timeout: Duration,
        label: &str,
    ) -> Result<GattWriteOption, String> {
        match write_gatt_value_with_timeout(characteristic, bytes, primary, timeout, label) {
            Ok(_) => Ok(primary),
            Err(primary_err) => {
                let fallback = match primary {
                    GattWriteOption::WriteWithoutResponse => GattWriteOption::WriteWithResponse,
                    GattWriteOption::WriteWithResponse => GattWriteOption::WriteWithoutResponse,
                    _ => return Err(primary_err),
                };
                log::warn!(
                    "[embedded-ble] {label} failed with {primary:?}: {primary_err}; retrying with {fallback:?}"
                );
                write_gatt_value_with_timeout(characteristic, bytes, fallback, timeout, label)
                    .map(|_| fallback)
                    .map_err(|fallback_err| {
                        format!(
                            "{primary_err}; fallback {fallback:?} for {label} also failed: {fallback_err}"
                        )
                    })
            }
        }
    }

    fn write_listener_ota_v2_value_with_fallback(
        characteristic: &GattCharacteristic,
        bytes: &[u8],
        primary: GattWriteOption,
        timeout: Duration,
        label: &str,
    ) -> Result<GattWriteOption, String> {
        match write_gatt_value_with_timeout(characteristic, bytes, primary, timeout, label) {
            Ok(_) => Ok(primary),
            Err(primary_err) => {
                let fallback = match primary {
                    GattWriteOption::WriteWithoutResponse => GattWriteOption::WriteWithResponse,
                    GattWriteOption::WriteWithResponse => GattWriteOption::WriteWithoutResponse,
                    _ => return Err(primary_err),
                };
                log::warn!(
                    "[embedded-ble] {label} failed with {primary:?}: {primary_err}; retrying with {fallback:?}"
                );
                write_gatt_value_with_timeout(characteristic, bytes, fallback, timeout, label)
                    .map(|_| fallback)
                    .map_err(|fallback_err| {
                        format!(
                            "{primary_err}; fallback {fallback:?} for {label} also failed: {fallback_err}"
                        )
                    })
            }
        }
    }

    #[cfg(feature = "companion-dev")]
    fn wait_stm32wb_st_ota_completion(
        transfer_id: u64,
        target: &OpenStm32wbStOtaTarget,
        confirm_enabled: bool,
        confirm_rx: &mpsc::Receiver<Vec<u8>>,
        handoff_rx: &mpsc::Receiver<String>,
    ) -> Result<(), String> {
        if confirm_enabled {
            let deadline = Instant::now() + STM32WB_ST_OTA_CONFIRM_TIMEOUT;
            loop {
                match confirm_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(confirm)
                        if confirm.first().copied() == Some(STM32WB_ST_OTA_REBOOT_CONFIRMED) =>
                    {
                        log::info!(
                            "[embedded-ble] STM32WB ST OTA #{transfer_id}: reboot confirmation indication received"
                        );
                        return Ok(());
                    }
                    Ok(confirm) => {
                        return Err(format!(
                            "STM32WB ST OTA returned unexpected confirmation payload: {:02x?}",
                            confirm
                        ));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Ok(reason) = handoff_rx.try_recv() {
                            log::info!(
                                "[embedded-ble] STM32WB ST OTA #{transfer_id}: reboot handoff observed before confirm: {reason}"
                            );
                            return Ok(());
                        }
                        if stm32wb_st_ota_target_rebooted(target) {
                            log::info!(
                                "[embedded-ble] STM32WB ST OTA #{transfer_id}: reboot handoff observed by polling"
                            );
                            return Ok(());
                        }
                        if Instant::now() >= deadline {
                            return Err(
                                "Timed out waiting for STM32WB ST OTA reboot confirmation indication or reboot handoff".to_string(),
                            );
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(
                            "STM32WB ST OTA confirm indication channel disconnected".to_string()
                        );
                    }
                }
            }
        }

        let deadline = Instant::now() + STM32WB_ST_OTA_REBOOT_HANDOFF_TIMEOUT;
        loop {
            match handoff_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(reason) => {
                    log::info!(
                        "[embedded-ble] STM32WB ST OTA #{transfer_id}: no-confirm reboot handoff observed: {reason}"
                    );
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if stm32wb_st_ota_target_rebooted(target) {
                        log::info!(
                            "[embedded-ble] STM32WB ST OTA #{transfer_id}: no-confirm reboot handoff observed by polling"
                        );
                        return Ok(());
                    }
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "Timed out waiting {} ms for STM32WB ST OTA no-confirm reboot handoff after finish",
                            STM32WB_ST_OTA_REBOOT_HANDOFF_TIMEOUT.as_millis()
                        ));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("STM32WB ST OTA reboot handoff channel disconnected".to_string());
                }
            }
        }
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_target_rebooted(target: &OpenStm32wbStOtaTarget) -> bool {
        if let Some(session) = target.session.as_ref() {
            match session.SessionStatus() {
                Ok(status) if status != GattSessionStatus::Active => {
                    return true;
                }
                Err(err) => {
                    log::info!(
                        "[embedded-ble] STM32WB ST OTA session status read failed after finish; treating as reboot handoff: {err}"
                    );
                    return true;
                }
                _ => {}
            }
        }
        if let Some(device) = target.device.as_ref() {
            match device.ConnectionStatus() {
                Ok(BluetoothConnectionStatus::Disconnected) => {
                    return true;
                }
                Err(err) => {
                    log::info!(
                        "[embedded-ble] STM32WB ST OTA device status read failed after finish; treating as reboot handoff: {err}"
                    );
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    #[cfg(feature = "companion-dev")]
    pub(super) fn is_stm32wb_st_ota_finish_reboot_handoff_error(err: &str) -> bool {
        let lower = err.to_ascii_lowercase();
        is_ota_finish_reboot_handoff_error(err)
            || lower.contains("gattcommunicationstatus(1)")
            || lower.contains("0x80000013")
            || (lower.contains("stm32wb st ota finish") && lower.contains("timed out"))
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_use_confirm_indication() -> bool {
        std::env::var("COMPANION_STM32WB_ST_OTA_CONFIRM")
            .ok()
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true)
    }

    pub fn firmware_ota_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let _fresh_guard = match BleFreshGattGuard::enter("firmware OTA snapshot") {
            Ok(guard) => guard,
            Err(err) => {
                return crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                    connected: false,
                    hardware_revision: None,
                    firmware_version: None,
                    capabilities: Vec::new(),
                    battery_percent: None,
                    usb_powered: None,
                    detail: Some(format!("Listener OTA unavailable: {err}")),
                };
            }
        };
        match open_ota_target() {
            Ok(target) => firmware_ota_device_snapshot_from_target(&target),
            Err(listener_err) => crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                connected: false,
                hardware_revision: None,
                firmware_version: None,
                capabilities: Vec::new(),
                battery_percent: None,
                usb_powered: None,
                detail: Some(format!("Listener OTA unavailable: {listener_err}")),
            },
        }
    }

    #[cfg(feature = "companion-dev")]
    pub fn stm32wb_st_ota_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        match open_stm32wb_st_ota_target() {
            Ok(target) => stm32wb_st_ota_device_snapshot_from_target(&target),
            Err(err) => crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                connected: false,
                hardware_revision: None,
                firmware_version: None,
                capabilities: Vec::new(),
                battery_percent: None,
                usb_powered: None,
                detail: Some(err),
            },
        }
    }

    #[cfg(feature = "companion-dev")]
    pub fn companion_ota_v2_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        match open_companion_ota_v2_target() {
            Ok(target) => companion_ota_v2_device_snapshot_from_target(&target),
            Err(err) => crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                connected: false,
                hardware_revision: None,
                firmware_version: None,
                capabilities: Vec::new(),
                battery_percent: None,
                usb_powered: None,
                detail: Some(err),
            },
        }
    }

    pub fn listener_ota_v2_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let _fresh_guard = match BleFreshGattGuard::enter("Listener OTA v2 snapshot") {
            Ok(guard) => guard,
            Err(err) => {
                return crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                    connected: false,
                    hardware_revision: None,
                    firmware_version: None,
                    capabilities: Vec::new(),
                    battery_percent: None,
                    usb_powered: None,
                    detail: Some(err),
                };
            }
        };
        match open_listener_ota_v2_target() {
            Ok(target) => listener_ota_v2_device_snapshot_from_target(&target),
            Err(err) => crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                connected: false,
                hardware_revision: None,
                firmware_version: None,
                capabilities: Vec::new(),
                battery_percent: None,
                usb_powered: None,
                detail: Some(err),
            },
        }
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_device_snapshot_from_target(
        target: &OpenStm32wbStOtaTarget,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: Some("NUCLEO-WB55RG".to_string()),
            firmware_version: Some("companion OTA loader".to_string()),
            capabilities: vec!["stm32wb_st_ble_ota_v1".to_string()],
            battery_percent: None,
            usb_powered: None,
            detail: target.bluetooth_address.map(|address| {
                format!(
                    "companion ST BLE_Ota loader connected at {}",
                    crate::embedded_ble::format_bluetooth_address(address)
                )
            }),
        }
    }

    #[cfg(feature = "companion-dev")]
    fn companion_ota_v2_device_snapshot_from_target(
        target: &OpenOtaV2Target,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: Some("NUCLEO-WB55RG".to_string()),
            firmware_version: Some("companion OTA v2 loader".to_string()),
            capabilities: vec!["companion_ota_v2".to_string()],
            battery_percent: None,
            usb_powered: None,
            detail: target.bluetooth_address.map(|address| {
                format!(
                    "companion OTA v2 loader connected at {}",
                    crate::embedded_ble::format_bluetooth_address(address)
                )
            }),
        }
    }

    fn listener_ota_v2_device_snapshot_from_target(
        target: &OpenListenerOtaV2Target,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let mut snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: None,
            firmware_version: None,
            capabilities: vec!["firmware_ota_v2".to_string()],
            battery_percent: None,
            usb_powered: None,
            detail: None,
        };
        if let Some(service) = target.service.as_ref() {
            if let Some(readiness) = read_optional_string_characteristic_from_service(
                service,
                OTA_READINESS_UUID,
                BluetoothCacheMode::Uncached,
            )
            .or_else(|| {
                read_optional_string_characteristic_from_service(
                    service,
                    OTA_CONTROL_UUID,
                    BluetoothCacheMode::Uncached,
                )
            }) {
                snapshot.hardware_revision =
                    readiness_hardware_revision(&readiness).or(snapshot.hardware_revision);
                snapshot.firmware_version =
                    readiness_field(&readiness, "fw_version").or(snapshot.firmware_version);
                snapshot.usb_powered = readiness_bool(&readiness, "external_power_present")
                    .or_else(|| readiness_bool(&readiness, "usb_power_present"))
                    .or_else(|| readiness_bool(&readiness, "charging"))
                    .or(snapshot.usb_powered);
                if snapshot.battery_percent.is_none()
                    && readiness_bool(&readiness, "battery_valid") != Some(false)
                {
                    snapshot.battery_percent = readiness_u8(&readiness, "battery_level");
                }
            } else {
                log::info!(
                    "[embedded-ble] Listener OTA v2 readiness identity not readable from current OTA service"
                );
            }

            if let Some(capabilities) = read_optional_string_characteristic_from_service(
                service,
                OTA_CAPABILITIES_UUID,
                BluetoothCacheMode::Uncached,
            )
            .or_else(|| {
                read_optional_string_characteristic_from_service(
                    service,
                    OTA_DATA_UUID,
                    BluetoothCacheMode::Uncached,
                )
            }) {
                let parsed = split_capability_tokens(&capabilities);
                if parsed.iter().any(|item| item == "firmware_ota_v2") {
                    snapshot.capabilities = parsed;
                }
            } else {
                log::info!(
                    "[embedded-ble] Listener OTA v2 capabilities not readable from current OTA service"
                );
            }
        }
        if !snapshot
            .capabilities
            .iter()
            .any(|item| item == "firmware_ota_v2")
        {
            snapshot.capabilities.push("firmware_ota_v2".to_string());
        }
        if snapshot.hardware_revision.is_none() && snapshot.firmware_version.is_none() {
            snapshot.detail = Some(
                "Listener OTA v2 service is reachable, but readiness identity was not exposed in this BLE session."
                    .to_string(),
            );
        }
        log::info!(
            "[embedded-ble] Listener OTA v2 snapshot connected={} hardware={:?} firmware={:?} battery={:?} usb_powered={:?} detail={:?}",
            snapshot.connected,
            snapshot.hardware_revision,
            snapshot.firmware_version,
            snapshot.battery_percent,
            snapshot.usb_powered,
            snapshot.detail
        );
        snapshot
    }

    fn firmware_ota_device_snapshot_from_target(
        target: &OpenOtaTarget,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let mut snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: None,
            firmware_version: None,
            capabilities: vec!["firmware_ota_v1".to_string()],
            battery_percent: None,
            usb_powered: None,
            detail: None,
        };
        let mut ota_readiness_snapshot: Option<String> = None;
        if let Some(service) = target.service.as_ref() {
            let ota_readiness = read_optional_string_characteristic_from_service(
                service,
                OTA_READINESS_UUID,
                BluetoothCacheMode::Uncached,
            )
            .or_else(|| {
                read_optional_string_characteristic_from_discovered_service(
                    OTA_SERVICE_UUID,
                    OTA_READINESS_UUID,
                    target.bluetooth_address,
                )
            })
            .or_else(|| {
                read_optional_string_characteristic_from_service(
                    service,
                    OTA_CONTROL_UUID,
                    BluetoothCacheMode::Uncached,
                )
            });
            if let Some(readiness) = ota_readiness {
                snapshot.hardware_revision =
                    readiness_hardware_revision(&readiness).or(snapshot.hardware_revision);
                snapshot.firmware_version =
                    readiness_field(&readiness, "fw_version").or(snapshot.firmware_version);
                snapshot.usb_powered = readiness_bool(&readiness, "external_power_present")
                    .or_else(|| readiness_bool(&readiness, "usb_power_present"))
                    .or_else(|| readiness_bool(&readiness, "charging"))
                    .or(snapshot.usb_powered);
                if snapshot.battery_percent.is_none()
                    && readiness_bool(&readiness, "battery_valid") != Some(false)
                {
                    snapshot.battery_percent = readiness_u8(&readiness, "battery_level");
                }
                ota_readiness_snapshot = Some(readiness);
            } else {
                log::info!(
                    "[embedded-ble] OTA readiness identity not readable from current OTA service"
                );
            }
            let ota_capabilities = read_optional_string_characteristic_from_service(
                service,
                OTA_CAPABILITIES_UUID,
                BluetoothCacheMode::Uncached,
            )
            .or_else(|| {
                read_optional_string_characteristic_from_discovered_service(
                    OTA_SERVICE_UUID,
                    OTA_CAPABILITIES_UUID,
                    target.bluetooth_address,
                )
            })
            .or_else(|| {
                read_optional_string_characteristic_from_service(
                    service,
                    OTA_DATA_UUID,
                    BluetoothCacheMode::Uncached,
                )
            });
            if let Some(capabilities) = ota_capabilities {
                let parsed = split_capability_tokens(&capabilities);
                if parsed.iter().any(|item| item == "firmware_ota_v1") {
                    snapshot.capabilities = parsed;
                }
            } else {
                log::info!("[embedded-ble] OTA capabilities not readable from current OTA service");
            }
        }
        let (dis_model, dis_hardware, dis_firmware, dis_battery) =
            read_dis_metadata_from_discovered_services(target.bluetooth_address);
        if snapshot.hardware_revision.is_none() {
            snapshot.hardware_revision =
                normalize_optional_hardware_revision(dis_hardware).or(dis_model);
        }
        if snapshot.firmware_version.is_none() {
            snapshot.firmware_version = dis_firmware;
        }
        let readiness_charging = ota_readiness_snapshot
            .as_deref()
            .and_then(|readiness| readiness_bool(readiness, "charging"));
        let readiness_charge_full = ota_readiness_snapshot
            .as_deref()
            .and_then(|readiness| readiness_bool(readiness, "charge_full"));
        if snapshot.battery_percent.is_none() {
            snapshot.battery_percent = dis_battery;
        }
        if snapshot.battery_percent == Some(100)
            && readiness_charging == Some(true)
            && readiness_charge_full != Some(true)
        {
            snapshot.battery_percent = Some(99);
        }

        if let Some(device) = target.device.as_ref() {
            if snapshot.hardware_revision.is_none() {
                let model = read_optional_string_characteristic(
                    device,
                    DIS_SERVICE_UUID,
                    DIS_MODEL_NUMBER_UUID,
                );
                let hardware = read_optional_string_characteristic(
                    device,
                    DIS_SERVICE_UUID,
                    DIS_HARDWARE_REVISION_UUID,
                );
                snapshot.hardware_revision =
                    normalize_optional_hardware_revision(hardware).or(model);
            }
            if snapshot.firmware_version.is_none() {
                snapshot.firmware_version = read_optional_string_characteristic(
                    device,
                    DIS_SERVICE_UUID,
                    DIS_FIRMWARE_REVISION_UUID,
                );
            }
            if snapshot.battery_percent.is_none() {
                snapshot.battery_percent = read_optional_u8_characteristic(
                    device,
                    BATTERY_SERVICE_UUID,
                    BATTERY_LEVEL_UUID,
                );
            }
        }
        if snapshot.hardware_revision.is_none()
            && snapshot.firmware_version.is_none()
            && snapshot.battery_percent.is_none()
        {
            snapshot.detail = Some(
                "OTA service is reachable, but Windows did not expose DIS metadata for this BLE session."
                    .to_string(),
            );
        }
        log::info!(
            "[embedded-ble] OTA snapshot connected={} hardware={:?} firmware={:?} battery={:?} usb_powered={:?} detail={:?}",
            snapshot.connected,
            snapshot.hardware_revision,
            snapshot.firmware_version,
            snapshot.battery_percent,
            snapshot.usb_powered,
            snapshot.detail
        );
        snapshot
    }

    fn read_dis_metadata_from_discovered_services(
        bluetooth_address: Option<u64>,
    ) -> (Option<String>, Option<String>, Option<String>, Option<u8>) {
        let model = read_optional_string_characteristic_from_discovered_service(
            DIS_SERVICE_UUID,
            DIS_MODEL_NUMBER_UUID,
            bluetooth_address,
        );
        let hardware = read_optional_string_characteristic_from_discovered_service(
            DIS_SERVICE_UUID,
            DIS_HARDWARE_REVISION_UUID,
            bluetooth_address,
        );
        let firmware = read_optional_string_characteristic_from_discovered_service(
            DIS_SERVICE_UUID,
            DIS_FIRMWARE_REVISION_UUID,
            bluetooth_address,
        );
        let battery = read_optional_u8_characteristic_from_discovered_service(
            BATTERY_SERVICE_UUID,
            BATTERY_LEVEL_UUID,
            bluetooth_address,
        );
        (model, hardware, firmware, battery)
    }

    fn open_notify_target() -> Result<OpenNotifyTarget, String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(SERVICE_UUID)
            .map_err(|err| format!("BLE service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE service discovery failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "service discovery"))?;
        let count = devices
            .Size()
            .map_err(|err| format!("BLE service collection size failed: {err}"))?;

        let mut last_error = if count == 0 {
            Some(format!(
                "Embedded audio BLE service {SERVICE_UUID:?} not found by Windows service selector"
            ))
        } else {
            None
        };
        if count > 0 {
            for index in 0..count {
                let info = match devices.GetAt(index) {
                    Ok(info) => info,
                    Err(err) => {
                        last_error = Some(format!("read BLE service info failed: {err}"));
                        continue;
                    }
                };
                let name = info
                    .Name()
                    .map(|value| value.to_string_lossy())
                    .unwrap_or_default();
                let id = match info.Id() {
                    Ok(id) => id,
                    Err(err) => {
                        last_error = Some(format!("read BLE service id failed: {err}"));
                        continue;
                    }
                };
                let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
                if !ble_candidate_allowed("audio notify", index, &name, address) {
                    continue;
                }

                let mut candidate_error = None;
                if let Some(address) = address {
                    match open_notify_target_for_device(address) {
                        Ok(target) => {
                            log::info!(
                                "[embedded-ble] selected device path index={index} name={name} address={address:012X}"
                            );
                            return Ok(target);
                        }
                        Err(err) => {
                            candidate_error = Some(format!(
                                "{name}: BLE device path {address:012X} failed: {err}"
                            ));
                        }
                    }
                }

                match open_notify_target_for_service(&id) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected service-id fallback index={index} name={name}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        last_error = Some(match candidate_error {
                            Some(previous) => {
                                format!("{previous}; service-id fallback failed: {err}")
                            }
                            None => format!("{name}: {err}"),
                        });
                    }
                }
            }
        }

        match open_notify_target_from_advertisement() {
            Ok(target) => Ok(target),
            Err(advertisement_error) => {
                let service_error = last_error.unwrap_or_else(|| {
                    "No subscribable embedded audio BLE notify characteristic found by service selector".to_string()
                });
                Err(format!(
                    "{service_error}; advertisement fallback failed: {advertisement_error}"
                ))
            }
        }
    }

    fn open_notify_target_with_retry(capture_id: u64) -> Result<OpenNotifyTarget, String> {
        let mut last_error = None;
        for attempt in 1..=NOTIFY_TARGET_OPEN_RETRY_DELAYS.len() + 1 {
            match open_notify_target() {
                Ok(target) => {
                    if attempt > 1 {
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: notify target open recovered on attempt {attempt}"
                        );
                    }
                    return Ok(target);
                }
                Err(err) => {
                    if attempt > NOTIFY_TARGET_OPEN_RETRY_DELAYS.len()
                        || !is_transient_notify_target_open_error(&err)
                    {
                        return Err(err);
                    }
                    let delay = NOTIFY_TARGET_OPEN_RETRY_DELAYS[attempt - 1];
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: notify target open attempt {attempt} failed: {err}; retrying in {} ms",
                        delay.as_millis()
                    );
                    last_error = Some(err);
                    std::thread::sleep(delay);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "No subscribable embedded audio BLE notify characteristic found".to_string()
        }))
    }

    fn read_embedded_audio_status_from_service(
        service: &GattDeviceService,
    ) -> crate::embedded_ble::EmbeddedAudioBleStatus {
        let readiness = read_optional_string_characteristic_from_service(
            service,
            OTA_READINESS_UUID,
            BluetoothCacheMode::Uncached,
        );
        let capabilities = read_optional_string_characteristic_from_service(
            service,
            OTA_CAPABILITIES_UUID,
            BluetoothCacheMode::Uncached,
        );

        crate::embedded_ble::EmbeddedAudioBleStatus {
            connected: true,
            readiness,
            capabilities,
            detail: Some("embedded audio BLE service reachable".to_string()),
        }
    }

    pub fn read_embedded_audio_status(
        _timeout: Duration,
    ) -> Result<crate::embedded_ble::EmbeddedAudioBleStatus, String> {
        let _fresh_guard = BleFreshGattGuard::enter("embedded audio status")?;
        let target = open_notify_target()?;
        let service = target
            .service
            .as_ref()
            .ok_or_else(|| "embedded audio BLE service was not retained".to_string())?;

        Ok(read_embedded_audio_status_from_service(service))
    }

    pub(super) fn is_transient_notify_target_open_error(err: &str) -> bool {
        err.contains("GattCommunicationStatus(3)")
            || err.contains("Unreachable")
            || err.contains("unreachable")
            || err.contains("HRESULT(0x800706BA)")
            || err.contains("BLE characteristic discovery returned status")
            || err.contains("BLE service open wait failed")
            || err.contains("BLE service discovery wait failed")
            || err.contains("GATT session did not become active")
            || err.contains("device open by address")
            || err.contains("device open by id")
    }

    fn is_transient_audio_control_write_error(err: &str) -> bool {
        err.contains("GattCommunicationStatus(2)")
            || err.contains("GattCommunicationStatus(3)")
            || err.contains("ProtocolError")
            || err.contains("protocol_error")
            || err.contains("Unreachable")
            || err.contains("unreachable")
            || err.contains("disconnected")
            || err.contains("timed out")
            || err.contains("timeout")
            || err.contains("stale")
            || err.contains("GATT session did not become active")
    }

    fn open_audio_control_target() -> Result<OpenAudioControlTarget, String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(SERVICE_UUID)
            .map_err(|err| format!("BLE audio control service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE audio control service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "audio control service discovery")
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("BLE audio control service collection size failed: {err}"))?;

        let mut last_error = if count == 0 {
            Some(format!(
                "Embedded audio BLE service {SERVICE_UUID:?} not found for recording control by Windows service selector"
            ))
        } else {
            None
        };
        if count > 0 {
            for index in 0..count {
                let info = match devices.GetAt(index) {
                    Ok(info) => info,
                    Err(err) => {
                        last_error =
                            Some(format!("read BLE audio control service info failed: {err}"));
                        continue;
                    }
                };
                let name = info
                    .Name()
                    .map(|value| value.to_string_lossy())
                    .unwrap_or_default();
                let id = match info.Id() {
                    Ok(id) => id,
                    Err(err) => {
                        last_error =
                            Some(format!("read BLE audio control service id failed: {err}"));
                        continue;
                    }
                };
                let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
                if !ble_candidate_allowed("audio control", index, &name, address) {
                    continue;
                }

                let mut candidate_error = None;
                if let Some(address) = address {
                    match open_audio_control_target_for_device(address) {
                        Ok(target) => {
                            log::info!(
                                "[embedded-ble] selected audio control device path index={index} name={name} address={address:012X}"
                            );
                            return Ok(target);
                        }
                        Err(err) => {
                            candidate_error = Some(format!(
                                "{name}: BLE audio control device path {address:012X} failed: {err}"
                            ));
                        }
                    }
                }

                match open_audio_control_target_for_service(&id) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected audio control service-id fallback index={index} name={name}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        last_error = Some(match candidate_error {
                            Some(previous) => {
                                format!(
                                    "{previous}; audio control service-id fallback failed: {err}"
                                )
                            }
                            None => format!("{name}: {err}"),
                        });
                    }
                }
            }
        }

        match open_audio_control_target_from_advertisement() {
            Ok(target) => Ok(target),
            Err(advertisement_error) => {
                let service_error = last_error.unwrap_or_else(|| {
                    "No writable Listener BLE audio control characteristic found by service selector"
                        .to_string()
                });
                Err(format!(
                    "{service_error}; advertisement fallback failed: {advertisement_error}"
                ))
            }
        }
    }

    fn open_audio_control_target_with_retry(label: &str) -> Result<OpenAudioControlTarget, String> {
        let mut last_error = None;
        for attempt in 1..=NOTIFY_TARGET_OPEN_RETRY_DELAYS.len() + 1 {
            match open_audio_control_target() {
                Ok(target) => {
                    if attempt > 1 {
                        log::info!(
                            "[embedded-ble] {label}: audio control target recovered on attempt {attempt}"
                        );
                    }
                    return Ok(target);
                }
                Err(err) => {
                    if attempt > NOTIFY_TARGET_OPEN_RETRY_DELAYS.len()
                        || !is_transient_notify_target_open_error(&err)
                    {
                        return Err(err);
                    }
                    let delay = NOTIFY_TARGET_OPEN_RETRY_DELAYS[attempt - 1];
                    log::warn!(
                        "[embedded-ble] {label}: audio control target open attempt {attempt} failed: {err}; retrying in {} ms",
                        delay.as_millis()
                    );
                    last_error = Some(err);
                    std::thread::sleep(delay);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "No writable Listener BLE audio control characteristic found".to_string()
        }))
    }

    fn open_notify_target_from_advertisement() -> Result<OpenNotifyTarget, String> {
        let addresses = audio_target_advertisement_addresses("audio notify")?;
        let mut last_error = None;
        for address in addresses {
            match open_notify_target_for_device(address) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected audio notify advertisement address={address:012X}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(format!(
                        "advertised audio notify address {address:012X} failed: {err}"
                    ));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "audio notify advertisement scan returned no usable addresses".to_string()
        }))
    }

    fn open_audio_control_target_from_advertisement() -> Result<OpenAudioControlTarget, String> {
        let addresses = audio_target_advertisement_addresses("audio control")?;
        let mut last_error = None;
        for address in addresses {
            match open_audio_control_target_for_device(address) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected audio control advertisement address={address:012X}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(format!(
                        "advertised audio control address {address:012X} failed: {err}"
                    ));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "audio control advertisement scan returned no usable addresses".to_string()
        }))
    }

    fn audio_target_advertisement_addresses(kind: &str) -> Result<Vec<u64>, String> {
        if let Some(address) = configured_bluetooth_address() {
            log::info!(
                "[embedded-ble] using configured {kind} BLE address {}",
                crate::embedded_ble::format_bluetooth_address(address)
            );
            return Ok(vec![address]);
        }

        if let Some(expected_name) = configured_bluetooth_target_name() {
            return scan_ble_advertisements_by_name(
                kind,
                &expected_name,
                AUDIO_ADVERTISEMENT_SCAN_TIMEOUT,
            );
        }

        scan_listener_audio_advertisements(kind, AUDIO_ADVERTISEMENT_SCAN_TIMEOUT)
    }

    fn read_optional_string_characteristic(
        device: &BluetoothLEDevice,
        service_uuid: GUID,
        characteristic_uuid: GUID,
    ) -> Option<String> {
        read_optional_characteristic_bytes(device, service_uuid, characteristic_uuid)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|value| value.trim_matches(char::from(0)).trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn read_optional_string_characteristic_from_service(
        service: &GattDeviceService,
        characteristic_uuid: GUID,
        cache_mode: BluetoothCacheMode,
    ) -> Option<String> {
        read_optional_characteristic_from_service(service, characteristic_uuid, cache_mode)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|value| value.trim_matches(char::from(0)).trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn readiness_field(readiness: &str, key: &str) -> Option<String> {
        let prefix = format!("{key}=");
        readiness
            .split(';')
            .find_map(|token| token.strip_prefix(&prefix))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn readiness_hardware_revision(readiness: &str) -> Option<String> {
        for key in ["hardware_revision", "board", "model"] {
            if let Some(value) = readiness_field(readiness, key) {
                return Some(normalize_listener_hardware_revision(&value).unwrap_or(value));
            }
        }
        None
    }

    fn normalize_optional_hardware_revision(value: Option<String>) -> Option<String> {
        value.map(|raw| normalize_listener_hardware_revision(&raw).unwrap_or(raw))
    }

    fn normalize_listener_hardware_revision(value: &str) -> Option<String> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "keyboard-v2-n16r8" | "voice-keyboard-v2-n16r8" | "esp32s3-wroom-1-n16r8" => {
                Some("keyboard-v2-n16r8".to_string())
            }
            _ => None,
        }
    }

    fn readiness_bool(readiness: &str, key: &str) -> Option<bool> {
        readiness_field(readiness, key).and_then(|value| match value.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
    }

    fn readiness_u8(readiness: &str, key: &str) -> Option<u8> {
        readiness_field(readiness, key)
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|value| *value <= 100)
    }

    fn split_capability_tokens(capabilities: &str) -> Vec<String> {
        capabilities
            .split(';')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }

    fn read_optional_u8_characteristic(
        device: &BluetoothLEDevice,
        service_uuid: GUID,
        characteristic_uuid: GUID,
    ) -> Option<u8> {
        read_optional_characteristic_bytes(device, service_uuid, characteristic_uuid)
            .and_then(|bytes| bytes.first().copied())
    }

    fn read_optional_string_characteristic_from_discovered_service(
        service_uuid: GUID,
        characteristic_uuid: GUID,
        bluetooth_address: Option<u64>,
    ) -> Option<String> {
        read_optional_characteristic_bytes_from_discovered_service(
            service_uuid,
            characteristic_uuid,
            bluetooth_address,
        )
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|value| value.trim_matches(char::from(0)).trim().to_string())
        .filter(|value| !value.is_empty())
    }

    fn read_optional_u8_characteristic_from_discovered_service(
        service_uuid: GUID,
        characteristic_uuid: GUID,
        bluetooth_address: Option<u64>,
    ) -> Option<u8> {
        read_optional_characteristic_bytes_from_discovered_service(
            service_uuid,
            characteristic_uuid,
            bluetooth_address,
        )
        .and_then(|bytes| bytes.first().copied())
    }

    fn read_optional_characteristic_bytes_from_discovered_service(
        service_uuid: GUID,
        characteristic_uuid: GUID,
        bluetooth_address: Option<u64>,
    ) -> Option<Vec<u8>> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(service_uuid).ok()?;
        let services = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .ok()?
            .get()
            .ok()?;
        let expected_name = effective_bluetooth_target_name(None);
        for index in 0..services.Size().ok()? {
            let info = services.GetAt(index).ok()?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = info.Id().ok()?;
            let address_matches = bluetooth_address.is_some_and(|expected_address| {
                parse_bluetooth_address_from_device_id(&id.to_string_lossy())
                    .is_some_and(|address| address == expected_address)
            });
            let name_matches = bluetooth_name_matches_expected(&name, &expected_name);
            if !address_matches && !name_matches {
                continue;
            }
            if let Some(expected_address) = bluetooth_address {
                match parse_bluetooth_address_from_device_id(&id.to_string_lossy()) {
                    Some(address) if address == expected_address => {}
                    Some(address) => {
                        log::debug!(
                            "[embedded-ble] skipping DIS service for address={address:012X}; target={expected_address:012X}"
                        );
                        continue;
                    }
                    None => {
                        log::debug!(
                            "[embedded-ble] skipping DIS service without parseable address for target={expected_address:012X}"
                        );
                        continue;
                    }
                }
            }
            let service = GattDeviceService::FromIdAsync(&id).ok()?.get().ok()?;
            let result = read_optional_characteristic_from_service(
                &service,
                characteristic_uuid,
                BluetoothCacheMode::Uncached,
            );
            let _ = service.Close();
            if result.is_some() {
                return result;
            }
        }
        None
    }

    fn read_optional_characteristic_bytes(
        device: &BluetoothLEDevice,
        service_uuid: GUID,
        characteristic_uuid: GUID,
    ) -> Option<Vec<u8>> {
        for cache_mode in [BluetoothCacheMode::Uncached] {
            let services_result = device
                .GetGattServicesForUuidWithCacheModeAsync(service_uuid, cache_mode)
                .ok()?
                .get()
                .ok()?;
            if services_result.Status().ok()? != GattCommunicationStatus::Success {
                continue;
            }
            let services = services_result.Services().ok()?;
            for index in 0..services.Size().ok()? {
                let service = services.GetAt(index).ok()?;
                let read_result = read_optional_characteristic_from_service(
                    &service,
                    characteristic_uuid,
                    cache_mode,
                );
                let _ = service.Close();
                if read_result.is_some() {
                    return read_result;
                }
            }
        }
        None
    }

    fn read_optional_characteristic_from_service(
        service: &GattDeviceService,
        characteristic_uuid: GUID,
        cache_mode: BluetoothCacheMode,
    ) -> Option<Vec<u8>> {
        let result = match service
            .GetCharacteristicsForUuidWithCacheModeAsync(characteristic_uuid, cache_mode)
            .ok()?
            .get()
        {
            Ok(result) => result,
            Err(err) => {
                log::debug!(
                    "[embedded-ble] optional characteristic {characteristic_uuid:?} discovery wait failed via {cache_mode:?}: {err}"
                );
                return None;
            }
        };
        let status = result.Status().ok()?;
        if status != GattCommunicationStatus::Success {
            log::debug!(
                "[embedded-ble] optional characteristic {characteristic_uuid:?} discovery returned status={status:?} via {cache_mode:?}"
            );
            return None;
        }
        let characteristics = result.Characteristics().ok()?;
        if characteristics.Size().ok()? == 0 {
            log::debug!(
                "[embedded-ble] optional characteristic {characteristic_uuid:?} not found via {cache_mode:?}"
            );
            return None;
        }
        let characteristic = characteristics.GetAt(0).ok()?;
        let read = match characteristic
            .ReadValueWithCacheModeAsync(cache_mode)
            .ok()?
            .get()
        {
            Ok(read) => read,
            Err(err) => {
                log::debug!(
                    "[embedded-ble] optional characteristic {characteristic_uuid:?} read wait failed via {cache_mode:?}: {err}"
                );
                return None;
            }
        };
        let status = read.Status().ok()?;
        if status != GattCommunicationStatus::Success {
            log::debug!(
                "[embedded-ble] optional characteristic {characteristic_uuid:?} read returned status={status:?} via {cache_mode:?}"
            );
            return None;
        }
        buffer_to_vec(&read.Value().ok()?).ok()
    }

    fn open_ota_target() -> Result<OpenOtaTarget, String> {
        let mut last_error = None;
        if let Some(address) = configured_bluetooth_address() {
            match open_ota_target_for_device(address) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected configured OTA device address={address:012X}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    let message = format!("configured OTA address {address:012X} failed: {err}");
                    log::warn!("[embedded-ble] {message}");
                    last_error = Some(message);
                }
            }
        }

        let selector = GattDeviceService::GetDeviceSelectorFromUuid(OTA_SERVICE_UUID)
            .map_err(|err| format!("BLE OTA service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE OTA service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "OTA service discovery")
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("BLE OTA service collection size failed: {err}"))?;
        if count == 0 {
            return Err(format!(
                "Listener BLE OTA service {OTA_SERVICE_UUID:?} not found; ensure firmware advertises firmware_ota_v1 and the device is paired and online"
            ));
        }

        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read BLE OTA service info failed: {err}"));
                    continue;
                }
            };
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = match info.Id() {
                Ok(id) => id,
                Err(err) => {
                    last_error = Some(format!("read BLE OTA service id failed: {err}"));
                    continue;
                }
            };
            let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
            if !ble_candidate_allowed("OTA", index, &name, address) {
                continue;
            }

            let mut candidate_error = None;
            if let Some(address) = address {
                match open_ota_target_for_device(address) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected OTA device index={index} name={name} address={address:012X}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        candidate_error = Some(format!(
                            "{name}: BLE OTA device path {address:012X} failed: {err}"
                        ));
                    }
                }
            }

            match open_ota_target_for_service(&id) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected OTA service-id fallback index={index} name={name}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(match candidate_error {
                        Some(previous) => {
                            format!("{previous}; OTA service-id fallback failed: {err}")
                        }
                        None => format!("{name}: {err}"),
                    });
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "No writable Listener BLE OTA service found".to_string()))
    }

    #[cfg(feature = "companion-dev")]
    fn open_stm32wb_st_ota_target() -> Result<OpenStm32wbStOtaTarget, String> {
        let mut errors = Vec::new();
        for uuid_set in STM32WB_ST_OTA_UUID_SETS.iter() {
            match open_stm32wb_st_ota_target_for_uuid_set(uuid_set) {
                Ok(target) => return Ok(target),
                Err(err) => errors.push(format!("{}: {err}", uuid_set.label)),
            }
        }
        Err(format!(
            "No writable STM32WB ST OTA service found ({})",
            errors.join("; ")
        ))
    }

    #[cfg(feature = "companion-dev")]
    fn open_companion_ota_v2_target() -> Result<OpenOtaV2Target, String> {
        match open_companion_ota_v2_target_from_advertisement() {
            Ok(target) => return Ok(target),
            Err(err) => {
                log::warn!("[embedded-ble] Companion OTA v2 advertisement fallback failed: {err}");
            }
        }

        let selector = GattDeviceService::GetDeviceSelectorFromUuid(OTA_V2_SERVICE_UUID)
            .map_err(|err| format!("Companion OTA v2 service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("Companion OTA v2 service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    BLE_DISCOVERY_TIMEOUT,
                    "Companion OTA v2 service discovery",
                )
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("Companion OTA v2 service collection size failed: {err}"))?;
        if count == 0 {
            return Err(format!(
                "Companion OTA v2 service {OTA_V2_SERVICE_UUID:?} not found; flash the new OTA loader or reset companion into OTA loader"
            ));
        }

        let mut last_error = None;
        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read Companion OTA v2 service info failed: {err}"));
                    continue;
                }
            };
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = match info.Id() {
                Ok(id) => id,
                Err(err) => {
                    last_error = Some(format!("read Companion OTA v2 service id failed: {err}"));
                    continue;
                }
            };
            let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
            if !ble_candidate_allowed("Companion OTA v2", index, &name, address) {
                continue;
            }

            let mut candidate_error = None;
            if let Some(address) = address {
                match open_companion_ota_v2_target_for_device(address) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected Companion OTA v2 device index={index} name={name} address={address:012X}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        candidate_error = Some(format!(
                            "{name}: Companion OTA v2 device path {address:012X} failed: {err}"
                        ));
                    }
                }
            }

            match open_companion_ota_v2_target_for_service(&id) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected Companion OTA v2 service-id fallback index={index} name={name}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(match candidate_error {
                        Some(previous) => {
                            format!(
                                "{previous}; Companion OTA v2 service-id fallback failed: {err}"
                            )
                        }
                        None => format!("{name}: {err}"),
                    });
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "No writable Companion OTA v2 service found".to_string()))
    }

    fn open_listener_ota_v2_target() -> Result<OpenListenerOtaV2Target, String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(LISTENER_OTA_V2_SERVICE_UUID)
            .map_err(|err| format!("Listener OTA v2 service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("Listener OTA v2 service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    BLE_DISCOVERY_TIMEOUT,
                    "Listener OTA v2 service discovery",
                )
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("Listener OTA v2 service collection size failed: {err}"))?;
        if count == 0 {
            return open_listener_ota_v2_target_from_stable_ota(format!(
                "Listener OTA v2 service {LISTENER_OTA_V2_SERVICE_UUID:?} not found in Windows service index"
            ));
        }

        let mut last_error = None;
        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read Listener OTA v2 service info failed: {err}"));
                    continue;
                }
            };
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = match info.Id() {
                Ok(id) => id,
                Err(err) => {
                    last_error = Some(format!("read Listener OTA v2 service id failed: {err}"));
                    continue;
                }
            };
            let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
            if !ble_candidate_allowed("Listener OTA v2", index, &name, address) {
                continue;
            }

            let mut candidate_error = None;
            if let Some(address) = address {
                match open_listener_ota_v2_target_for_device(address) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected Listener OTA v2 device index={index} name={name} address={address:012X}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        candidate_error = Some(format!(
                            "{name}: Listener OTA v2 device path {address:012X} failed: {err}"
                        ));
                    }
                }
            }

            match open_listener_ota_v2_target_for_service(&id) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected Listener OTA v2 service-id fallback index={index} name={name}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(match candidate_error {
                        Some(previous) => {
                            format!("{previous}; Listener OTA v2 service-id fallback failed: {err}")
                        }
                        None => format!("{name}: {err}"),
                    });
                }
            }
        }

        match open_listener_ota_v2_target_from_stable_ota(
            last_error.unwrap_or_else(|| "No writable Listener OTA v2 service found".to_string()),
        ) {
            Ok(target) => Ok(target),
            Err(err) => Err(err),
        }
    }

    fn open_listener_ota_v2_target_from_stable_ota(
        previous_error: String,
    ) -> Result<OpenListenerOtaV2Target, String> {
        log::warn!(
            "[embedded-ble] Listener OTA v2 direct discovery failed; trying stable OTA service anchor: {previous_error}"
        );
        let stable = open_ota_target()
            .map_err(|err| format!("{previous_error}; stable Listener OTA anchor failed: {err}"))?;
        let address = stable.bluetooth_address.ok_or_else(|| {
            format!(
                "{previous_error}; stable Listener OTA anchor did not expose a Bluetooth address"
            )
        })?;
        open_listener_ota_v2_target_for_device(address).map_err(|err| {
            format!(
                "{previous_error}; Listener OTA v2 uncached device discovery via stable OTA anchor {} failed: {err}",
                crate::embedded_ble::format_bluetooth_address(address)
            )
        })
    }

    #[cfg(feature = "companion-dev")]
    fn open_companion_ota_v2_target_from_advertisement() -> Result<OpenOtaV2Target, String> {
        let addresses = scan_ble_advertisements_by_name(
            "Companion OTA v2",
            STM32WB_ST_OTA_ADVERTISEMENT_NAME,
            STM32WB_ST_OTA_ADVERTISEMENT_SCAN_TIMEOUT,
        )?;
        let mut last_error = None;
        for address in addresses {
            match open_companion_ota_v2_target_for_device(address) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected Companion OTA v2 advertisement address={address:012X}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(format!(
                        "advertised companion address {address:012X} failed: {err}"
                    ));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "companion OTA v2 advertisement scan returned no usable addresses".to_string()
        }))
    }

    #[cfg(feature = "companion-dev")]
    fn open_stm32wb_st_ota_target_for_uuid_set(
        uuid_set: &Stm32wbStOtaUuidSet,
    ) -> Result<OpenStm32wbStOtaTarget, String> {
        if uuid_set.label == "companion" {
            match open_stm32wb_st_ota_target_from_advertisement(uuid_set) {
                Ok(target) => return Ok(target),
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] STM32WB ST OTA companion advertisement fallback failed: {err}"
                    );
                }
            }
        }

        let selector = GattDeviceService::GetDeviceSelectorFromUuid(uuid_set.service)
            .map_err(|err| format!("STM32WB ST OTA service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("STM32WB ST OTA service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    BLE_DISCOVERY_TIMEOUT,
                    "STM32WB ST OTA service discovery",
                )
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("STM32WB ST OTA service collection size failed: {err}"))?;
        if count == 0 {
            return Err(format!(
                "STM32WB ST OTA service {:?} ({}) not found; flash or reboot into companion OTA loader, then connect companion in Windows Bluetooth if Windows has not cached it yet",
                uuid_set.service, uuid_set.label
            ));
        }

        let mut last_error = None;
        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read STM32WB ST OTA service info failed: {err}"));
                    continue;
                }
            };
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = match info.Id() {
                Ok(id) => id,
                Err(err) => {
                    last_error = Some(format!("read STM32WB ST OTA service id failed: {err}"));
                    continue;
                }
            };
            let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
            if !ble_candidate_allowed("STM32WB ST OTA", index, &name, address) {
                continue;
            }

            let mut candidate_error = None;
            if let Some(address) = address {
                match open_stm32wb_st_ota_target_for_device(address, uuid_set) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected STM32WB ST OTA {} device index={index} name={name} address={address:012X}",
                            uuid_set.label
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        candidate_error = Some(format!(
                            "{name}: STM32WB ST OTA device path {address:012X} failed: {err}"
                        ));
                    }
                }
            }

            match open_stm32wb_st_ota_target_for_service(&id, uuid_set) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected STM32WB ST OTA {} service-id fallback index={index} name={name}",
                        uuid_set.label
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(match candidate_error {
                        Some(previous) => {
                            format!("{previous}; STM32WB ST OTA service-id fallback failed: {err}")
                        }
                        None => format!("{name}: {err}"),
                    });
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "No writable STM32WB ST OTA service found".to_string()))
    }

    #[cfg(feature = "companion-dev")]
    fn open_stm32wb_st_ota_target_from_advertisement(
        uuid_set: &Stm32wbStOtaUuidSet,
    ) -> Result<OpenStm32wbStOtaTarget, String> {
        let addresses = scan_stm32wb_st_ota_advertisements()?;
        let mut last_error = None;
        for address in addresses {
            match open_stm32wb_st_ota_target_for_device(address, uuid_set) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected STM32WB ST OTA {} advertisement address={address:012X}",
                        uuid_set.label
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(format!(
                        "advertised companion address {address:012X} failed: {err}"
                    ));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "companion advertisement scan returned no usable addresses".to_string()
        }))
    }

    #[cfg(feature = "companion-dev")]
    fn scan_stm32wb_st_ota_advertisements() -> Result<Vec<u64>, String> {
        scan_ble_advertisements_by_name(
            "STM32WB ST OTA",
            STM32WB_ST_OTA_ADVERTISEMENT_NAME,
            STM32WB_ST_OTA_ADVERTISEMENT_SCAN_TIMEOUT,
        )
    }

    fn scan_listener_pairing_advertisements(
        expected_name: Option<&str>,
    ) -> Result<Vec<(u64, BluetoothAddressType, String)>, String> {
        scan_listener_advertisements(
            "Listener pairing",
            expected_name,
            AUDIO_ADVERTISEMENT_SCAN_TIMEOUT,
        )
    }

    fn scan_listener_audio_advertisements(
        context: &str,
        timeout: Duration,
    ) -> Result<Vec<u64>, String> {
        let expected_name = effective_bluetooth_target_name(None);
        let candidates = scan_listener_advertisements(context, Some(&expected_name), timeout)?;
        Ok(candidates
            .into_iter()
            .map(|(address, _address_type, _name)| address)
            .collect())
    }

    fn scan_listener_advertisements(
        context: &str,
        expected_name: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<(u64, BluetoothAddressType, String)>, String> {
        let watcher = BluetoothLEAdvertisementWatcher::new()
            .map_err(|err| format!("{context} advertisement watcher create failed: {err}"))?;
        watcher
            .SetScanningMode(BluetoothLEScanningMode::Active)
            .map_err(|err| format!("{context} advertisement active scan failed: {err}"))?;

        let (tx, rx) = mpsc::channel::<(u64, BluetoothAddressType, String, i16)>();
        let expected_name_for_handler = expected_name.map(ToOwned::to_owned);
        let handler = TypedEventHandler::<
            BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementReceivedEventArgs,
        >::new(move |_watcher, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let Ok(advertisement) = args.Advertisement() else {
                return Ok(());
            };
            let name = advertisement
                .LocalName()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if !listener_pairing_name_matches(&name, expected_name_for_handler.as_deref()) {
                return Ok(());
            }
            let address = args.BluetoothAddress().unwrap_or_default();
            if address == 0 {
                return Ok(());
            }
            let address_type = args
                .BluetoothAddressType()
                .unwrap_or(BluetoothAddressType::Unspecified);
            let rssi = args.RawSignalStrengthInDBm().unwrap_or_default();
            let _ = tx.send((address, address_type, name, rssi));
            Ok(())
        });

        let token = watcher
            .Received(&handler)
            .map_err(|err| format!("{context} advertisement handler failed: {err}"))?;
        watcher
            .Start()
            .map_err(|err| format!("{context} advertisement scan start failed: {err}"))?;

        let deadline = Instant::now() + timeout;
        let mut addresses = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = remaining.min(Duration::from_millis(500));
            match rx.recv_timeout(timeout) {
                Ok((address, address_type, name, rssi)) => {
                    if addresses.iter().any(|(seen, _, _)| *seen == address) {
                        continue;
                    }
                    log::info!(
                        "[embedded-ble] {context} advertisement candidate name={name} address={address:012X} address_type={address_type:?} rssi={rssi}"
                    );
                    addresses.push((address, address_type, name));
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let _ = watcher.Stop();
        let _ = watcher.RemoveReceived(token);

        if addresses.is_empty() {
            return Err(format!(
                "no Listener advertisement seen for {context} in {} ms",
                timeout.as_millis()
            ));
        }
        Ok(addresses)
    }

    fn scan_ble_advertisements_by_name(
        context: &str,
        expected_name: &str,
        timeout: Duration,
    ) -> Result<Vec<u64>, String> {
        let watcher = BluetoothLEAdvertisementWatcher::new()
            .map_err(|err| format!("{context} advertisement watcher create failed: {err}"))?;
        watcher
            .SetScanningMode(BluetoothLEScanningMode::Active)
            .map_err(|err| format!("{context} advertisement active scan failed: {err}"))?;

        let (tx, rx) = mpsc::channel::<(u64, String, i16)>();
        let expected_name_for_handler = expected_name.to_string();
        let handler = TypedEventHandler::<
            BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementReceivedEventArgs,
        >::new(move |_watcher, args| {
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            let Ok(advertisement) = args.Advertisement() else {
                return Ok(());
            };
            let name = advertisement
                .LocalName()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if !ble_advertisement_name_matches(&name, &expected_name_for_handler) {
                return Ok(());
            }
            let address = args.BluetoothAddress().unwrap_or_default();
            if address == 0 {
                return Ok(());
            }
            let rssi = args.RawSignalStrengthInDBm().unwrap_or_default();
            let _ = tx.send((address, name, rssi));
            Ok(())
        });

        let token = watcher
            .Received(&handler)
            .map_err(|err| format!("{context} advertisement handler failed: {err}"))?;
        watcher
            .Start()
            .map_err(|err| format!("{context} advertisement scan start failed: {err}"))?;

        let deadline = Instant::now() + timeout;
        let mut addresses = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = remaining.min(Duration::from_millis(500));
            match rx.recv_timeout(timeout) {
                Ok((address, name, rssi)) => {
                    if addresses.contains(&address) {
                        continue;
                    }
                    log::info!(
                        "[embedded-ble] {context} advertisement candidate name={name} address={address:012X} rssi={rssi}"
                    );
                    addresses.push(address);
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let _ = watcher.Stop();
        let _ = watcher.RemoveReceived(token);

        if addresses.is_empty() {
            return Err(format!(
                "no Bluetooth advertisement named {expected_name:?} seen for {context} in {} ms",
                timeout.as_millis()
            ));
        }
        Ok(addresses)
    }

    fn ble_advertisement_name_matches(name: &str, expected_name: &str) -> bool {
        name.trim().eq_ignore_ascii_case(expected_name.trim())
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_advertisement_name_matches(name: &str) -> bool {
        ble_advertisement_name_matches(name, STM32WB_ST_OTA_ADVERTISEMENT_NAME)
    }

    fn diagnostic_target_candidates() -> Result<Vec<DiagnosticTargetCandidate>, String> {
        let mut candidates = Vec::new();
        let mut seen_addresses = Vec::new();
        let mut seen_service_ids = Vec::new();

        if let Some(address) = configured_bluetooth_address() {
            push_diagnostic_device_candidate(
                &mut candidates,
                &mut seen_addresses,
                address,
                format!(
                    "configured address {}",
                    crate::embedded_ble::format_bluetooth_address(address)
                ),
            );
        }

        let selector = GattDeviceService::GetDeviceSelectorFromUuid(DIAGNOSTIC_SERVICE_UUID)
            .map_err(|err| format!("BLE diagnostic service selector failed: {err}"))?;
        let devices = match DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE diagnostic service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "diagnostic service discovery")
            }) {
            Ok(devices) => devices,
            Err(err) => {
                if candidates.is_empty() {
                    return Err(err);
                }
                log::warn!(
                    "[embedded-ble] diagnostic service discovery failed; trying configured target only: {err}"
                );
                return Ok(candidates);
            }
        };
        let count = devices
            .Size()
            .map_err(|err| format!("BLE diagnostic service collection size failed: {err}"))?;
        if count == 0 {
            if candidates.is_empty() {
                return Err(format!(
                    "Listener BLE diagnostic service {DIAGNOSTIC_SERVICE_UUID:?} not found; ensure firmware exposes diag_export_v1 and the device is paired and online"
                ));
            }
            log::warn!(
                "[embedded-ble] diagnostic service discovery returned no entries; trying configured target only"
            );
            return Ok(candidates);
        }

        let mut last_error = None;
        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read BLE diagnostic service info failed: {err}"));
                    continue;
                }
            };
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = match info.Id() {
                Ok(id) => id,
                Err(err) => {
                    last_error = Some(format!("read BLE diagnostic service id failed: {err}"));
                    continue;
                }
            };

            let id_text = id.to_string_lossy();
            if let Some(address) = parse_bluetooth_address_from_device_id(&id_text) {
                push_diagnostic_device_candidate(
                    &mut candidates,
                    &mut seen_addresses,
                    address,
                    format!("discovered service index={index} name={name} address={address:012X}"),
                );
            }

            if !seen_service_ids.iter().any(|seen| seen == &id_text) {
                seen_service_ids.push(id_text);
                candidates.push(DiagnosticTargetCandidate::Service {
                    label: format!("service-id fallback index={index} name={name}"),
                    id,
                });
            }
        }

        if candidates.is_empty() {
            return Err(last_error
                .unwrap_or_else(|| "No usable Listener BLE diagnostic service found".to_string()));
        }
        Ok(candidates)
    }

    fn push_diagnostic_device_candidate(
        candidates: &mut Vec<DiagnosticTargetCandidate>,
        seen_addresses: &mut Vec<u64>,
        address: u64,
        label: String,
    ) {
        if seen_addresses.iter().any(|seen| *seen == address) {
            return;
        }
        seen_addresses.push(address);
        candidates.push(DiagnosticTargetCandidate::Device { label, address });
    }

    fn open_ota_target_for_device(address: u64) -> Result<OpenOtaTarget, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "OTA device access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!("BLE OTA device access denied status={access:?}"));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(OTA_SERVICE_UUID, cache_mode)
                .map_err(|err| format!("BLE OTA {cache_mode:?} service discovery failed: {err}"))
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("OTA {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!("BLE OTA {cache_mode:?} service discovery wait failed: {err}")
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result.Status().map_err(|err| {
                format!("BLE OTA {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "BLE OTA {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result
                .Services()
                .map_err(|err| format!("BLE OTA {cache_mode:?} service list read failed: {err}"))?;
            let count = services
                .Size()
                .map_err(|err| format!("BLE OTA {cache_mode:?} service list size failed: {err}"))?;
            if count == 0 {
                last_error = Some(format!(
                    "OTA service {OTA_SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error =
                            Some(format!("read BLE OTA {cache_mode:?} service failed: {err}"));
                        continue;
                    }
                };
                match open_ota_characteristics_from_service(&service, cache_mode) {
                    Ok(prepared) => {
                        return Ok(OpenOtaTarget {
                            control: prepared.control,
                            data: prepared.data,
                            data_write_option: prepared.data_write_option,
                            data_chunk_bytes: prepared.data_chunk_bytes,
                            service: Some(service),
                            session: prepared.session,
                            device: Some(device),
                            bluetooth_address: Some(address),
                        });
                    }
                    Err(err) => {
                        last_error = Some(format!("{cache_mode:?}: {err}"));
                        let _ = service.Close();
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No writable Listener BLE OTA characteristics found on device".to_string()
        }))
    }

    #[cfg(feature = "companion-dev")]
    fn open_stm32wb_st_ota_target_for_device(
        address: u64,
        uuid_set: &Stm32wbStOtaUuidSet,
    ) -> Result<OpenStm32wbStOtaTarget, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "STM32WB ST OTA device access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "STM32WB ST OTA device access denied status={access:?}"
                ));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(uuid_set.service, cache_mode)
                .map_err(|err| {
                    format!("STM32WB ST OTA {cache_mode:?} service discovery failed: {err}")
                })
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("STM32WB ST OTA {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!(
                            "STM32WB ST OTA {cache_mode:?} service discovery wait failed: {err}"
                        )
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result.Status().map_err(|err| {
                format!("STM32WB ST OTA {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "STM32WB ST OTA {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("STM32WB ST OTA {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("STM32WB ST OTA {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "STM32WB ST OTA service {:?} ({}) not found from BLE device via {cache_mode:?}",
                    uuid_set.service, uuid_set.label
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read STM32WB ST OTA {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                match open_stm32wb_st_ota_characteristics_from_service(
                    &service, cache_mode, uuid_set,
                ) {
                    Ok(prepared) => {
                        return Ok(OpenStm32wbStOtaTarget {
                            base: prepared.base,
                            raw: prepared.raw,
                            confirm: prepared.confirm,
                            base_write_option: prepared.base_write_option,
                            raw_write_option: prepared.raw_write_option,
                            raw_chunk_bytes: prepared.raw_chunk_bytes,
                            service: Some(service),
                            session: prepared.session,
                            device: Some(device),
                            bluetooth_address: Some(address),
                        });
                    }
                    Err(err) => {
                        last_error = Some(format!("{cache_mode:?}: {err}"));
                        let _ = service.Close();
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No writable STM32WB ST OTA characteristics found on device".to_string()
        }))
    }

    #[cfg(feature = "companion-dev")]
    fn open_companion_ota_v2_target_for_device(address: u64) -> Result<OpenOtaV2Target, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Companion OTA v2 device access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Companion OTA v2 device access denied status={access:?}"
                ));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(OTA_V2_SERVICE_UUID, cache_mode)
                .map_err(|err| {
                    format!("Companion OTA v2 {cache_mode:?} service discovery failed: {err}")
                })
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("Companion OTA v2 {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!(
                            "Companion OTA v2 {cache_mode:?} service discovery wait failed: {err}"
                        )
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result.Status().map_err(|err| {
                format!("Companion OTA v2 {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "Companion OTA v2 {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("Companion OTA v2 {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("Companion OTA v2 {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "Companion OTA v2 service {OTA_V2_SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read Companion OTA v2 {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                match open_companion_ota_v2_characteristics_from_service(&service, cache_mode) {
                    Ok(prepared) => {
                        return Ok(OpenOtaV2Target {
                            control: prepared.control,
                            data: prepared.data,
                            status: prepared.status,
                            data_write_option: prepared.data_write_option,
                            data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                            service: Some(service),
                            session: prepared.session,
                            device: Some(device),
                            bluetooth_address: Some(address),
                        });
                    }
                    Err(err) => {
                        last_error = Some(format!("{cache_mode:?}: {err}"));
                        let _ = service.Close();
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No writable Companion OTA v2 characteristics found on device".to_string()
        }))
    }

    fn open_listener_ota_v2_target_for_device(
        address: u64,
    ) -> Result<OpenListenerOtaV2Target, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Listener OTA v2 device access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Listener OTA v2 device access denied status={access:?}"
                ));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(LISTENER_OTA_V2_SERVICE_UUID, cache_mode)
                .map_err(|err| {
                    format!("Listener OTA v2 {cache_mode:?} service discovery failed: {err}")
                })
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("Listener OTA v2 {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!(
                            "Listener OTA v2 {cache_mode:?} service discovery wait failed: {err}"
                        )
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result.Status().map_err(|err| {
                format!("Listener OTA v2 {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "Listener OTA v2 {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("Listener OTA v2 {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("Listener OTA v2 {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "Listener OTA v2 service {LISTENER_OTA_V2_SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read Listener OTA v2 {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                match open_listener_ota_v2_characteristics_from_service(&service, cache_mode) {
                    Ok(prepared) => {
                        return Ok(OpenListenerOtaV2Target {
                            control: prepared.control,
                            data: prepared.data,
                            status: prepared.status,
                            data_write_option: prepared.data_write_option,
                            data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                            service: Some(service),
                            session: prepared.session,
                            device: Some(device),
                            bluetooth_address: Some(address),
                        });
                    }
                    Err(err) => {
                        last_error = Some(format!("{cache_mode:?}: {err}"));
                        let _ = service.Close();
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No writable Listener OTA v2 characteristics found on device".to_string()
        }))
    }

    fn open_diagnostic_target_for_device(address: u64) -> Result<OpenDiagnosticTarget, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "diagnostic device access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "BLE diagnostic device access denied status={access:?}"
                ));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(DIAGNOSTIC_SERVICE_UUID, cache_mode)
                .map_err(|err| {
                    format!("BLE diagnostic {cache_mode:?} service discovery failed: {err}")
                })
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("diagnostic {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!(
                            "BLE diagnostic {cache_mode:?} service discovery wait failed: {err}"
                        )
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result.Status().map_err(|err| {
                format!("BLE diagnostic {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "BLE diagnostic {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("BLE diagnostic {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("BLE diagnostic {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "diagnostic service {DIAGNOSTIC_SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read BLE diagnostic {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                match open_diagnostic_characteristics_from_service(&service, cache_mode) {
                    Ok(prepared) => {
                        return Ok(OpenDiagnosticTarget {
                            control: prepared.control,
                            data: prepared.data,
                            count: prepared.count,
                            service: Some(service),
                            session: prepared.session,
                            device: Some(device),
                        });
                    }
                    Err(err) => {
                        last_error = Some(format!("{cache_mode:?}: {err}"));
                        let _ = service.Close();
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No usable Listener BLE diagnostic characteristics found on device".to_string()
        }))
    }

    fn open_notify_target_for_device(address: u64) -> Result<OpenNotifyTarget, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device
            .RequestAccessAsync()
            .ok()
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "device access").ok())
        {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!("BLE device access denied status={access:?}"));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(SERVICE_UUID, cache_mode)
                .map_err(|err| format!("BLE {cache_mode:?} service discovery failed: {err}"))
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("{cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!("BLE {cache_mode:?} service discovery wait failed: {err}")
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result
                .Status()
                .map_err(|err| format!("BLE {cache_mode:?} service status read failed: {err}"))?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "BLE {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result
                .Services()
                .map_err(|err| format!("BLE {cache_mode:?} service list read failed: {err}"))?;
            let count = services
                .Size()
                .map_err(|err| format!("BLE {cache_mode:?} service list size failed: {err}"))?;
            if count == 0 {
                last_error = Some(format!(
                    "service {SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!("read BLE {cache_mode:?} service failed: {err}"));
                        continue;
                    }
                };
                match open_notify_characteristic_from_service(&service, cache_mode) {
                    Ok(prepared) => {
                        return Ok(OpenNotifyTarget {
                            characteristic: prepared.characteristic,
                            control: prepared.control,
                            service: Some(service),
                            session: prepared.session,
                            device: Some(device),
                        });
                    }
                    Err(err) => {
                        last_error = Some(format!("{cache_mode:?}: {err}"));
                        let _ = service.Close();
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No subscribable embedded audio BLE notify characteristic found on device".to_string()
        }))
    }

    fn open_audio_control_target_for_device(
        address: u64,
    ) -> Result<OpenAudioControlTarget, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device
            .RequestAccessAsync()
            .ok()
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "device access").ok())
        {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!("BLE device access denied status={access:?}"));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(SERVICE_UUID, cache_mode)
                .map_err(|err| {
                    format!("BLE audio control {cache_mode:?} service discovery failed: {err}")
                })
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("audio control {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!(
                            "BLE audio control {cache_mode:?} service discovery wait failed: {err}"
                        )
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result.Status().map_err(|err| {
                format!("BLE audio control {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "BLE audio control {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("BLE audio control {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("BLE audio control {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "service {SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read BLE audio control {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                match open_audio_control_characteristic_from_service(&service, cache_mode) {
                    Ok(prepared) => {
                        return Ok(OpenAudioControlTarget {
                            control: prepared.control,
                            service: Some(service),
                            session: prepared.session,
                            device: Some(device),
                        });
                    }
                    Err(err) => {
                        last_error = Some(format!("{cache_mode:?}: {err}"));
                        let _ = service.Close();
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No writable Listener BLE audio control characteristic found on device".to_string()
        }))
    }

    fn open_ble_device(address: u64) -> Result<BluetoothLEDevice, String> {
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(address)
            .map_err(|err| format!("BLE device open by address failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "device open by address")
            })?;

        let device_id = device
            .DeviceId()
            .map(|id| id.to_string_lossy())
            .unwrap_or_default();
        if device_id.is_empty() {
            return Ok(device);
        }

        match BluetoothLEDevice::FromIdAsync(&HSTRING::from(device_id.as_str()))
            .ok()
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "device open by id").ok()
            }) {
            Some(device_by_id) => {
                let _ = device.Close();
                Ok(device_by_id)
            }
            None => {
                log::warn!("[embedded-ble] BLE device reopen by id failed, using address handle");
                Ok(device)
            }
        }
    }

    fn open_ota_target_for_service(service_id: &HSTRING) -> Result<OpenOtaTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE OTA service open failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "OTA service open"))?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "OTA service device").ok()
                })
        });

        let prepared =
            open_ota_characteristics_from_service(&service, BluetoothCacheMode::Uncached)?;
        Ok(OpenOtaTarget {
            control: prepared.control,
            data: prepared.data,
            data_write_option: prepared.data_write_option,
            data_chunk_bytes: prepared.data_chunk_bytes,
            service: Some(service),
            session: prepared.session,
            device,
            bluetooth_address: parse_bluetooth_address_from_device_id(
                &service_id.to_string_lossy(),
            ),
        })
    }

    #[cfg(feature = "companion-dev")]
    fn open_stm32wb_st_ota_target_for_service(
        service_id: &HSTRING,
        uuid_set: &Stm32wbStOtaUuidSet,
    ) -> Result<OpenStm32wbStOtaTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("STM32WB ST OTA service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "STM32WB ST OTA service open")
            })?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "STM32WB ST OTA service device")
                        .ok()
                })
        });

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            match open_stm32wb_st_ota_characteristics_from_service(&service, cache_mode, uuid_set) {
                Ok(prepared) => {
                    return Ok(OpenStm32wbStOtaTarget {
                        base: prepared.base,
                        raw: prepared.raw,
                        confirm: prepared.confirm,
                        base_write_option: prepared.base_write_option,
                        raw_write_option: prepared.raw_write_option,
                        raw_chunk_bytes: prepared.raw_chunk_bytes,
                        service: Some(service),
                        session: prepared.session,
                        device,
                        bluetooth_address: parse_bluetooth_address_from_device_id(
                            &service_id.to_string_lossy(),
                        ),
                    });
                }
                Err(err) => {
                    last_error = Some(format!("{cache_mode:?}: {err}"));
                }
            }
        }
        return Err(last_error.unwrap_or_else(|| {
            "No writable STM32WB ST OTA characteristics found from service id".to_string()
        }));
    }

    #[cfg(feature = "companion-dev")]
    fn open_companion_ota_v2_target_for_service(
        service_id: &HSTRING,
    ) -> Result<OpenOtaV2Target, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("Companion OTA v2 service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Companion OTA v2 service open")
            })?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        "Companion OTA v2 service device",
                    )
                    .ok()
                })
        });

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            match open_companion_ota_v2_characteristics_from_service(&service, cache_mode) {
                Ok(prepared) => {
                    return Ok(OpenOtaV2Target {
                        control: prepared.control,
                        data: prepared.data,
                        status: prepared.status,
                        data_write_option: prepared.data_write_option,
                        data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                        service: Some(service),
                        session: prepared.session,
                        device,
                        bluetooth_address: parse_bluetooth_address_from_device_id(
                            &service_id.to_string_lossy(),
                        ),
                    });
                }
                Err(err) => {
                    last_error = Some(format!("{cache_mode:?}: {err}"));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "No writable Companion OTA v2 characteristics found from service id".to_string()
        }))
    }

    fn open_listener_ota_v2_target_for_service(
        service_id: &HSTRING,
    ) -> Result<OpenListenerOtaV2Target, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("Listener OTA v2 service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Listener OTA v2 service open")
            })?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        "Listener OTA v2 service device",
                    )
                    .ok()
                })
        });

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            match open_listener_ota_v2_characteristics_from_service(&service, cache_mode) {
                Ok(prepared) => {
                    return Ok(OpenListenerOtaV2Target {
                        control: prepared.control,
                        data: prepared.data,
                        status: prepared.status,
                        data_write_option: prepared.data_write_option,
                        data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                        service: Some(service),
                        session: prepared.session,
                        device,
                        bluetooth_address: parse_bluetooth_address_from_device_id(
                            &service_id.to_string_lossy(),
                        ),
                    });
                }
                Err(err) => {
                    last_error = Some(format!("{cache_mode:?}: {err}"));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "No writable Listener OTA v2 characteristics found from service id".to_string()
        }))
    }

    fn open_diagnostic_target_for_service(
        service_id: &HSTRING,
    ) -> Result<OpenDiagnosticTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE diagnostic service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "diagnostic service open")
            })?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "diagnostic service device")
                        .ok()
                })
        });

        let prepared =
            open_diagnostic_characteristics_from_service(&service, BluetoothCacheMode::Uncached)?;
        Ok(OpenDiagnosticTarget {
            control: prepared.control,
            data: prepared.data,
            count: prepared.count,
            service: Some(service),
            session: prepared.session,
            device,
        })
    }

    fn open_notify_target_for_service(service_id: &HSTRING) -> Result<OpenNotifyTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE service open failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "service open"))?;

        let prepared =
            open_notify_characteristic_from_service(&service, BluetoothCacheMode::Uncached)?;
        Ok(OpenNotifyTarget {
            characteristic: prepared.characteristic,
            control: prepared.control,
            service: Some(service),
            session: prepared.session,
            device: None,
        })
    }

    fn open_audio_control_target_for_service(
        service_id: &HSTRING,
    ) -> Result<OpenAudioControlTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE audio control service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "audio control service open")
            })?;

        let prepared =
            open_audio_control_characteristic_from_service(&service, BluetoothCacheMode::Uncached)?;
        Ok(OpenAudioControlTarget {
            control: prepared.control,
            service: Some(service),
            session: prepared.session,
            device: None,
        })
    }

    fn open_ota_characteristics_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedOtaCharacteristics, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "OTA service access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!("BLE OTA service access denied status={access:?}"));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let control = open_write_characteristic_from_service(
            service,
            OTA_CONTROL_UUID,
            "OTA control",
            cache_mode,
        )?;
        let data =
            open_write_characteristic_from_service(service, OTA_DATA_UUID, "OTA data", cache_mode)?;
        let data_properties = data
            .CharacteristicProperties()
            .map_err(|err| format!("BLE OTA data characteristic properties read failed: {err}"))?;
        let data_write_option = ota_data_write_option(data_properties)?;
        let data_chunk_bytes = ota_data_chunk_bytes(session.as_ref(), data_write_option);
        log::info!(
            "[embedded-ble] OTA data write option={data_write_option:?} chunk_bytes={data_chunk_bytes}"
        );
        Ok(PreparedOtaCharacteristics {
            control,
            data,
            data_write_option,
            data_chunk_bytes,
            session,
        })
    }

    #[cfg(feature = "companion-dev")]
    fn open_stm32wb_st_ota_characteristics_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
        uuid_set: &Stm32wbStOtaUuidSet,
    ) -> Result<PreparedStm32wbStOtaCharacteristics, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "STM32WB ST OTA service access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "STM32WB ST OTA service access denied status={access:?}"
                ));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let base = open_write_characteristic_from_service(
            service,
            uuid_set.base,
            "STM32WB ST OTA base",
            cache_mode,
        )?;
        let raw = open_write_characteristic_from_service(
            service,
            uuid_set.raw,
            "STM32WB ST OTA raw",
            cache_mode,
        )?;
        let confirm = open_indicate_characteristic_from_service(
            service,
            uuid_set.confirm,
            "STM32WB ST OTA confirm",
            cache_mode,
        )?;
        let base_properties = base.CharacteristicProperties().map_err(|err| {
            format!("STM32WB ST OTA base characteristic properties read failed: {err}")
        })?;
        let base_write_option = stm32wb_st_ota_write_option(base_properties)?;
        let raw_properties = raw.CharacteristicProperties().map_err(|err| {
            format!("STM32WB ST OTA raw characteristic properties read failed: {err}")
        })?;
        let raw_write_option = stm32wb_st_ota_write_option(raw_properties)?;
        let raw_chunk_bytes = stm32wb_st_ota_raw_chunk_bytes(session.as_ref(), raw_write_option);
        log::info!(
            "[embedded-ble] STM32WB ST OTA {} base write option={base_write_option:?} raw write option={raw_write_option:?} chunk_bytes={raw_chunk_bytes}",
            uuid_set.label
        );
        Ok(PreparedStm32wbStOtaCharacteristics {
            base,
            raw,
            confirm,
            base_write_option,
            raw_write_option,
            raw_chunk_bytes,
            session,
        })
    }

    #[cfg(feature = "companion-dev")]
    fn open_companion_ota_v2_characteristics_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedOtaV2Characteristics, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Companion OTA v2 service access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Companion OTA v2 service access denied status={access:?}"
                ));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let control = open_write_characteristic_from_service(
            service,
            OTA_V2_CONTROL_UUID,
            "Companion OTA v2 control",
            cache_mode,
        )?;
        let data = open_write_characteristic_from_service(
            service,
            OTA_V2_DATA_UUID,
            "Companion OTA v2 data",
            cache_mode,
        )?;
        let status = open_read_characteristic_from_service(
            service,
            OTA_V2_STATUS_UUID,
            "Companion OTA v2 status",
            cache_mode,
        )?;
        let data_properties = data.CharacteristicProperties().map_err(|err| {
            format!("Companion OTA v2 data characteristic properties read failed: {err}")
        })?;
        let data_write_option = ota_v2_data_write_option(data_properties)?;
        let payload_bytes = ota_v2_data_chunk_payload_bytes(session.as_ref());
        log::info!(
            "[embedded-ble] Companion OTA v2 data write option={data_write_option:?} chunk_payload_bytes={payload_bytes}"
        );
        Ok(PreparedOtaV2Characteristics {
            control,
            data,
            status,
            data_write_option,
            data_chunk_payload_bytes: payload_bytes,
            session,
        })
    }

    fn open_listener_ota_v2_characteristics_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedListenerOtaV2Characteristics, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Listener OTA v2 service access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Listener OTA v2 service access denied status={access:?}"
                ));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let control = open_write_characteristic_from_service(
            service,
            LISTENER_OTA_V2_CONTROL_UUID,
            "Listener OTA v2 control",
            cache_mode,
        )?;
        let data = open_write_characteristic_from_service(
            service,
            LISTENER_OTA_V2_DATA_UUID,
            "Listener OTA v2 data",
            cache_mode,
        )?;
        let status = if LISTENER_OTA_V2_STATUS_UUID == LISTENER_OTA_V2_CONTROL_UUID {
            control.clone()
        } else {
            open_read_characteristic_from_service(
                service,
                LISTENER_OTA_V2_STATUS_UUID,
                "Listener OTA v2 status",
                cache_mode,
            )?
        };
        let data_properties = data.CharacteristicProperties().map_err(|err| {
            format!("Listener OTA v2 data characteristic properties read failed: {err}")
        })?;
        let data_write_option = listener_ota_v2_data_write_option(data_properties)?;
        let payload_bytes =
            listener_ota_v2_data_chunk_payload_bytes(session.as_ref(), data_write_option);
        log::info!(
            "[embedded-ble] Listener OTA v2 data write option={data_write_option:?} chunk_payload_bytes={payload_bytes}"
        );
        Ok(PreparedListenerOtaV2Characteristics {
            control,
            data,
            status,
            data_write_option,
            data_chunk_payload_bytes: payload_bytes,
            session,
        })
    }

    fn open_diagnostic_characteristics_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedDiagnosticCharacteristics, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "diagnostic service access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "BLE diagnostic service access denied status={access:?}"
                ));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let control = open_write_characteristic_from_service(
            service,
            DIAGNOSTIC_CONTROL_UUID,
            "diagnostic control",
            cache_mode,
        )?;
        let data = open_notify_characteristic_by_uuid_from_service(
            service,
            DIAGNOSTIC_DATA_UUID,
            "diagnostic data",
            cache_mode,
        )?;
        let count = open_read_characteristic_from_service(
            service,
            DIAGNOSTIC_COUNT_UUID,
            "diagnostic count",
            cache_mode,
        )?;
        Ok(PreparedDiagnosticCharacteristics {
            control,
            data,
            count,
            session,
        })
    }

    fn open_audio_control_characteristic_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedAudioControlCharacteristic, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "audio control service access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "BLE audio control service access denied status={access:?}"
                ));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let control = open_write_characteristic_from_service(
            service,
            AUDIO_CONTROL_UUID,
            "audio control",
            cache_mode,
        )?;
        Ok(PreparedAudioControlCharacteristic { control, session })
    }

    fn ota_data_write_option(
        data_properties: GattCharacteristicProperties,
    ) -> Result<GattWriteOption, String> {
        let requested = std::env::var(OTA_DATA_WRITE_OPTION_ENV)
            .ok()
            .and_then(ota_env_value);
        ota_data_write_option_from_request(data_properties, requested.as_deref())
    }

    fn ota_data_write_option_from_request(
        data_properties: GattCharacteristicProperties,
        requested: Option<&str>,
    ) -> Result<GattWriteOption, String> {
        let supports_write = data_properties.contains(GattCharacteristicProperties::Write);
        let supports_without_response =
            data_properties.contains(GattCharacteristicProperties::WriteWithoutResponse);
        let requested = requested
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "auto".to_string());
        let option = match requested.as_str() {
            "auto" => {
                if supports_write {
                    GattWriteOption::WriteWithResponse
                } else if supports_without_response {
                    GattWriteOption::WriteWithoutResponse
                } else {
                    return Err(
                        "BLE OTA data characteristic must support Write or WriteWithoutResponse."
                            .to_string(),
                    );
                }
            }
            "with-response" | "with_response" | "response" => {
                if supports_write {
                    GattWriteOption::WriteWithResponse
                } else {
                    return Err(format!(
                        "{OTA_DATA_WRITE_OPTION_ENV}=with-response requested, but BLE OTA data characteristic does not support Write."
                    ));
                }
            }
            "without-response" | "without_response" | "no-response" | "no_response" => {
                if supports_without_response {
                    GattWriteOption::WriteWithoutResponse
                } else {
                    return Err(format!(
                        "{OTA_DATA_WRITE_OPTION_ENV}=without-response requested, but BLE OTA data characteristic does not support WriteWithoutResponse."
                    ));
                }
            }
            other => {
                return Err(format!(
                    "Unsupported {OTA_DATA_WRITE_OPTION_ENV}={other}; use auto, with-response, or without-response."
                ));
            }
        };
        log::info!("[embedded-ble] OTA data write option request={requested} selected={option:?}");
        Ok(option)
    }

    fn ota_data_chunk_bytes(session: Option<&GattSession>, write_option: GattWriteOption) -> usize {
        let payload_bytes = session
            .and_then(|session| session.MaxPduSize().ok())
            .map(|max_pdu_size| usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES))
            .filter(|payload_bytes| *payload_bytes > 0)
            .unwrap_or(ATT_DEFAULT_PAYLOAD_BYTES);
        if write_option == GattWriteOption::WriteWithoutResponse {
            return payload_bytes.min(ATT_DEFAULT_PAYLOAD_BYTES).max(1);
        }
        payload_bytes.max(1)
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_write_option(
        properties: GattCharacteristicProperties,
    ) -> Result<GattWriteOption, String> {
        // The Companion loader exposes Write on top of ST's original
        // Write-Without-Response path so Windows waits for the WB55 write
        // permit response instead of silently dropping queued raw data.
        if properties.contains(GattCharacteristicProperties::Write) {
            Ok(GattWriteOption::WriteWithResponse)
        } else if properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
            Ok(GattWriteOption::WriteWithoutResponse)
        } else {
            Err("STM32WB ST OTA raw characteristic is not writable".to_string())
        }
    }

    #[cfg(feature = "companion-dev")]
    fn ota_v2_data_write_option(
        properties: GattCharacteristicProperties,
    ) -> Result<GattWriteOption, String> {
        if properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
            Ok(GattWriteOption::WriteWithoutResponse)
        } else if properties.contains(GattCharacteristicProperties::Write) {
            Ok(GattWriteOption::WriteWithResponse)
        } else {
            Err("Companion OTA v2 data characteristic is not writable".to_string())
        }
    }

    fn listener_ota_v2_data_write_option(
        properties: GattCharacteristicProperties,
    ) -> Result<GattWriteOption, String> {
        if properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
            Ok(GattWriteOption::WriteWithoutResponse)
        } else if properties.contains(GattCharacteristicProperties::Write) {
            Ok(GattWriteOption::WriteWithResponse)
        } else {
            Err("Listener OTA v2 data characteristic is not writable".to_string())
        }
    }

    #[cfg(feature = "companion-dev")]
    fn ota_v2_data_chunk_payload_bytes(session: Option<&GattSession>) -> usize {
        let payload_bytes = session
            .and_then(|session| session.MaxPduSize().ok())
            .map(|max_pdu_size| usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES))
            .filter(|payload_bytes| *payload_bytes > OTA_V2_PACKET_HEADER_BYTES)
            .unwrap_or(ATT_DEFAULT_PAYLOAD_BYTES);
        payload_bytes
            .saturating_sub(OTA_V2_PACKET_HEADER_BYTES)
            .min(OTA_V2_CHUNK_PAYLOAD_BYTES)
            .max(1)
    }

    fn listener_ota_v2_data_chunk_payload_bytes(
        session: Option<&GattSession>,
        write_option: GattWriteOption,
    ) -> usize {
        let desired_payload = LISTENER_OTA_V2_CHUNK_PAYLOAD_BYTES + OTA_V2_PACKET_HEADER_BYTES;
        let mut payload_bytes = session
            .and_then(|session| session.MaxPduSize().ok())
            .map(|max_pdu_size| usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES))
            .filter(|payload_bytes| *payload_bytes > OTA_V2_PACKET_HEADER_BYTES)
            .unwrap_or(ATT_DEFAULT_PAYLOAD_BYTES);

        if write_option == GattWriteOption::WriteWithoutResponse && payload_bytes < desired_payload
        {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && payload_bytes < desired_payload {
                std::thread::sleep(Duration::from_millis(100));
                if let Some(next_payload) = session
                    .and_then(|session| session.MaxPduSize().ok())
                    .map(|max_pdu_size| {
                        usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES)
                    })
                    .filter(|payload_bytes| *payload_bytes > OTA_V2_PACKET_HEADER_BYTES)
                {
                    payload_bytes = payload_bytes.max(next_payload);
                }
            }
            if payload_bytes < desired_payload {
                log::warn!(
                    "[embedded-ble] Listener OTA v2 MaxPduSize stayed at payload_bytes={payload_bytes}; using {LISTENER_OTA_V2_CHUNK_PAYLOAD_BYTES} byte payload for WriteWithoutResponse and relying on WinRT write status"
                );
                return LISTENER_OTA_V2_CHUNK_PAYLOAD_BYTES;
            }
        }

        payload_bytes
            .saturating_sub(OTA_V2_PACKET_HEADER_BYTES)
            .min(LISTENER_OTA_V2_CHUNK_PAYLOAD_BYTES)
            .max(1)
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_base_address_command(base_address: u32) -> [u8; 4] {
        [
            STM32WB_ST_OTA_APPLICATION_UPLOAD,
            ((base_address >> 16) & 0xff) as u8,
            ((base_address >> 8) & 0xff) as u8,
            (base_address & 0xff) as u8,
        ]
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_raw_chunk_bytes(
        session: Option<&GattSession>,
        write_option: GattWriteOption,
    ) -> usize {
        let payload_bytes = session
            .and_then(|session| session.MaxPduSize().ok())
            .map(|max_pdu_size| usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES))
            .filter(|payload_bytes| *payload_bytes > 0)
            .unwrap_or(ATT_DEFAULT_PAYLOAD_BYTES);
        stm32wb_st_ota_raw_chunk_bytes_from_payload(payload_bytes, write_option)
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_raw_chunk_bytes_from_payload(
        payload_bytes: usize,
        write_option: GattWriteOption,
    ) -> usize {
        let payload_bytes = payload_bytes.max(1);
        let transport_limit = if write_option == GattWriteOption::WriteWithoutResponse {
            payload_bytes.min(ATT_DEFAULT_PAYLOAD_BYTES)
        } else {
            payload_bytes
        };
        transport_limit.min(STM32WB_ST_OTA_RAW_DATA_SIZE).max(1)
    }

    #[cfg(feature = "companion-dev")]
    fn stm32wb_st_ota_transfer_chunk_bytes(
        transport_limit_bytes: usize,
        manifest_chunk_bytes: usize,
    ) -> Result<usize, String> {
        if manifest_chunk_bytes != STM32WB_ST_OTA_RAW_DATA_SIZE {
            return Err(format!(
                "STM32WB ST OTA manifest chunk size must be {STM32WB_ST_OTA_RAW_DATA_SIZE} bytes, got {manifest_chunk_bytes}."
            ));
        }
        if transport_limit_bytes == 0 {
            return Err(
                "STM32WB ST OTA transport payload limit is zero; cannot transfer firmware."
                    .to_string(),
            );
        }
        Ok(transport_limit_bytes.min(STM32WB_ST_OTA_RAW_DATA_SIZE))
    }

    pub(super) fn ota_transfer_chunk_bytes(
        transport_limit_bytes: usize,
        manifest_chunk_bytes: usize,
    ) -> Result<usize, String> {
        if manifest_chunk_bytes != OTA_REQUIRED_DATA_CHUNK_BYTES {
            return Err(format!(
                "OTA manifest chunk size must be {OTA_REQUIRED_DATA_CHUNK_BYTES} bytes, got {manifest_chunk_bytes}."
            ));
        }
        let test_chunk_bytes = ota_data_test_chunk_bytes_override()?;
        ota_transfer_chunk_bytes_with_override(
            transport_limit_bytes,
            manifest_chunk_bytes,
            test_chunk_bytes,
        )
    }

    fn ota_transfer_chunk_bytes_with_override(
        transport_limit_bytes: usize,
        manifest_chunk_bytes: usize,
        test_chunk_bytes: Option<usize>,
    ) -> Result<usize, String> {
        if transport_limit_bytes == 0 {
            return Err(format!(
                "BLE transport payload limit is {transport_limit_bytes} bytes; OTA requires a positive data chunk size."
            ));
        }
        let mut chunk_bytes = manifest_chunk_bytes.min(transport_limit_bytes);
        if let Some(test_chunk_bytes) = test_chunk_bytes {
            chunk_bytes = chunk_bytes.min(test_chunk_bytes);
        }
        if chunk_bytes == 0 {
            return Err("BLE OTA data chunk size resolved to 0 bytes.".to_string());
        }
        Ok(chunk_bytes)
    }

    fn ota_data_test_chunk_bytes_override() -> Result<Option<usize>, String> {
        let value = std::env::var(OTA_DATA_CHUNK_BYTES_ENV)
            .ok()
            .and_then(ota_env_value);
        ota_data_test_chunk_bytes_override_from(value.as_deref())
    }

    fn ota_data_test_chunk_bytes_override_from(
        value: Option<&str>,
    ) -> Result<Option<usize>, String> {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            None => Ok(None),
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|chunk_bytes| *chunk_bytes > 0)
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "Unsupported {OTA_DATA_CHUNK_BYTES_ENV}={value}; use a positive byte count."
                    )
                }),
        }
    }

    fn ota_data_inter_chunk_delay() -> Result<Duration, String> {
        let value = std::env::var(OTA_DATA_INTER_CHUNK_DELAY_MS_ENV)
            .ok()
            .and_then(ota_env_value);
        ota_data_inter_chunk_delay_from(value.as_deref())
    }

    fn ota_data_inter_chunk_delay_from(value: Option<&str>) -> Result<Duration, String> {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            None => Ok(Duration::ZERO),
            Some(value) => value.parse::<u64>().map(Duration::from_millis).map_err(|_| {
                format!(
                    "Unsupported {OTA_DATA_INTER_CHUNK_DELAY_MS_ENV}={value}; use a non-negative millisecond count."
                )
            }),
        }
    }

    fn ota_env_value(value: String) -> Option<String> {
        let value = value.trim().to_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }

    fn type_ready_command_bytes() -> Vec<u8> {
        let command = std::env::var(TYPE_READY_COMMAND_ENV)
            .ok()
            .and_then(ota_env_value)
            .unwrap_or_else(|| "TYPE:READY".to_string());
        let command = if command.starts_with("TYPE:READY") {
            command
        } else {
            log::warn!(
                "[embedded-ble] ignoring unsupported {TYPE_READY_COMMAND_ENV}={command:?}; using TYPE:READY"
            );
            "TYPE:READY".to_string()
        };
        let mut bytes = command.into_bytes();
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes
    }

    fn open_write_characteristic_from_service(
        service: &GattDeviceService,
        uuid: GUID,
        label: &str,
        cache_mode: BluetoothCacheMode,
    ) -> Result<GattCharacteristic, String> {
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(uuid, cache_mode)
            .map_err(|err| format!("BLE {label} characteristic discovery failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE {label} characteristic discovery wait failed: {err}"))?;
        let status = result
            .Status()
            .map_err(|err| format!("BLE {label} characteristic status read failed: {err}"))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!(
                "BLE {label} characteristic discovery returned status={status:?}"
            ));
        }
        let characteristics = result
            .Characteristics()
            .map_err(|err| format!("BLE {label} characteristic list read failed: {err}"))?;
        if characteristics
            .Size()
            .map_err(|err| format!("BLE {label} characteristic list size failed: {err}"))?
            == 0
        {
            return Err(format!("{label} characteristic {uuid:?} not found"));
        }

        let characteristic = characteristics
            .GetAt(0)
            .map_err(|err| format!("BLE {label} characteristic read failed: {err}"))?;
        let properties = characteristic
            .CharacteristicProperties()
            .map_err(|err| format!("BLE {label} characteristic properties read failed: {err}"))?;
        if !properties.contains(GattCharacteristicProperties::Write)
            && !properties.contains(GattCharacteristicProperties::WriteWithoutResponse)
        {
            return Err(format!("{label} characteristic is not writable"));
        }
        Ok(characteristic)
    }

    fn open_indicate_characteristic_from_service(
        service: &GattDeviceService,
        uuid: GUID,
        label: &str,
        cache_mode: BluetoothCacheMode,
    ) -> Result<GattCharacteristic, String> {
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(uuid, cache_mode)
            .map_err(|err| format!("BLE {label} characteristic discovery failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE {label} characteristic discovery wait failed: {err}"))?;
        let status = result
            .Status()
            .map_err(|err| format!("BLE {label} characteristic status read failed: {err}"))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!(
                "BLE {label} characteristic discovery returned status={status:?}"
            ));
        }
        let characteristics = result
            .Characteristics()
            .map_err(|err| format!("BLE {label} characteristic list read failed: {err}"))?;
        if characteristics
            .Size()
            .map_err(|err| format!("BLE {label} characteristic list size failed: {err}"))?
            == 0
        {
            return Err(format!("{label} characteristic {uuid:?} not found"));
        }

        let characteristic = characteristics
            .GetAt(0)
            .map_err(|err| format!("BLE {label} characteristic read failed: {err}"))?;
        let properties = characteristic
            .CharacteristicProperties()
            .map_err(|err| format!("BLE {label} characteristic properties read failed: {err}"))?;
        if !properties.contains(GattCharacteristicProperties::Indicate) {
            return Err(format!(
                "{label} characteristic does not advertise INDICATE"
            ));
        }
        Ok(characteristic)
    }

    fn open_read_characteristic_from_service(
        service: &GattDeviceService,
        uuid: GUID,
        label: &str,
        cache_mode: BluetoothCacheMode,
    ) -> Result<GattCharacteristic, String> {
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(uuid, cache_mode)
            .map_err(|err| format!("BLE {label} characteristic discovery failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE {label} characteristic discovery wait failed: {err}"))?;
        let status = result
            .Status()
            .map_err(|err| format!("BLE {label} characteristic status read failed: {err}"))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!(
                "BLE {label} characteristic discovery returned status={status:?}"
            ));
        }
        let characteristics = result
            .Characteristics()
            .map_err(|err| format!("BLE {label} characteristic list read failed: {err}"))?;
        if characteristics
            .Size()
            .map_err(|err| format!("BLE {label} characteristic list size failed: {err}"))?
            == 0
        {
            return Err(format!("{label} characteristic {uuid:?} not found"));
        }

        let characteristic = characteristics
            .GetAt(0)
            .map_err(|err| format!("BLE {label} characteristic read failed: {err}"))?;
        let properties = characteristic
            .CharacteristicProperties()
            .map_err(|err| format!("BLE {label} characteristic properties read failed: {err}"))?;
        if !properties.contains(GattCharacteristicProperties::Read) {
            return Err(format!("{label} characteristic is not readable"));
        }
        Ok(characteristic)
    }

    fn open_notify_characteristic_by_uuid_from_service(
        service: &GattDeviceService,
        uuid: GUID,
        label: &str,
        cache_mode: BluetoothCacheMode,
    ) -> Result<GattCharacteristic, String> {
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(uuid, cache_mode)
            .map_err(|err| format!("BLE {label} characteristic discovery failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE {label} characteristic discovery wait failed: {err}"))?;
        let status = result
            .Status()
            .map_err(|err| format!("BLE {label} characteristic status read failed: {err}"))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!(
                "BLE {label} characteristic discovery returned status={status:?}"
            ));
        }
        let characteristics = result
            .Characteristics()
            .map_err(|err| format!("BLE {label} characteristic list read failed: {err}"))?;
        if characteristics
            .Size()
            .map_err(|err| format!("BLE {label} characteristic list size failed: {err}"))?
            == 0
        {
            return Err(format!("{label} characteristic {uuid:?} not found"));
        }

        let characteristic = characteristics
            .GetAt(0)
            .map_err(|err| format!("BLE {label} characteristic read failed: {err}"))?;
        let properties = characteristic
            .CharacteristicProperties()
            .map_err(|err| format!("BLE {label} characteristic properties read failed: {err}"))?;
        if !properties.contains(GattCharacteristicProperties::Notify) {
            return Err(format!("{label} characteristic does not advertise NOTIFY"));
        }
        Ok(characteristic)
    }

    fn open_notify_characteristic_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedNotifyCharacteristic, String> {
        if let Some(access) = service
            .RequestAccessAsync()
            .ok()
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "service access").ok())
        {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!("BLE service access denied status={access:?}"));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let control = open_optional_audio_control_for_notify_setup(service, cache_mode);

        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(NOTIFY_UUID, cache_mode)
            .map_err(|err| format!("BLE characteristic discovery failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE characteristic discovery wait failed: {err}"))?;
        let status = result
            .Status()
            .map_err(|err| format!("BLE characteristic status read failed: {err}"))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!(
                "BLE characteristic discovery returned status={status:?}"
            ));
        }
        let characteristics = result
            .Characteristics()
            .map_err(|err| format!("BLE characteristic list read failed: {err}"))?;
        if characteristics
            .Size()
            .map_err(|err| format!("BLE characteristic list size failed: {err}"))?
            == 0
        {
            return Err(format!("notify characteristic {NOTIFY_UUID:?} not found"));
        }

        let characteristic = characteristics
            .GetAt(0)
            .map_err(|err| format!("BLE notify characteristic read failed: {err}"))?;
        let properties = characteristic
            .CharacteristicProperties()
            .map_err(|err| format!("BLE notify characteristic properties read failed: {err}"))?;
        if !properties.contains(GattCharacteristicProperties::Notify) {
            return Err("BLE notify characteristic does not advertise NOTIFY".to_string());
        }
        Ok(PreparedNotifyCharacteristic {
            characteristic,
            control,
            session,
        })
    }

    fn open_optional_audio_control_for_notify_setup(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Option<GattCharacteristic> {
        let mut last_error = None;
        for attempt in 1..=AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS.len() + 1 {
            match open_write_characteristic_from_service(
                service,
                AUDIO_CONTROL_UUID,
                "audio control",
                cache_mode,
            ) {
                Ok(control) => {
                    if attempt > 1 {
                        log::info!(
                            "[embedded-ble] audio control characteristic recovered during notify setup via {cache_mode:?} on attempt {attempt}"
                        );
                    }
                    return Some(control);
                }
                Err(err) => {
                    let transient = is_transient_audio_control_write_error(&err);
                    if attempt > AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS.len() || !transient {
                        log::warn!(
                            "[embedded-ble] audio control characteristic unavailable during notify setup via {cache_mode:?}: {err}"
                        );
                        return None;
                    }
                    let delay = AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS[attempt - 1];
                    log::info!(
                        "[embedded-ble] audio control characteristic discovery attempt {attempt} via {cache_mode:?} returned transient error: {err}; retrying in {} ms",
                        delay.as_millis()
                    );
                    last_error = Some(err);
                    std::thread::sleep(delay);
                }
            }
        }
        log::warn!(
            "[embedded-ble] audio control characteristic unavailable during notify setup via {cache_mode:?}: {}",
            last_error.unwrap_or_else(|| "no attempts completed".to_string())
        );
        None
    }

    fn prepare_gatt_session(
        service: &GattDeviceService,
        timeout: Duration,
    ) -> Result<Option<GattSession>, String> {
        let session = match service.Session() {
            Ok(session) => session,
            Err(err) => {
                log::warn!("[embedded-ble] GATT session unavailable: {err}");
                return Ok(None);
            }
        };
        match session.CanMaintainConnection() {
            Ok(true) => {
                if let Err(err) = session.SetMaintainConnection(true) {
                    log::warn!("[embedded-ble] GATT maintain connection failed: {err}");
                }
            }
            Ok(false) => {}
            Err(err) => log::warn!("[embedded-ble] GATT maintain capability read failed: {err}"),
        }
        let initial_status = session.SessionStatus().ok();
        if wait_gatt_session_ready(&session, timeout) {
            log::info!(
                "[embedded-ble] GATT session ready initial={:?} current={:?}",
                initial_status,
                session.SessionStatus().ok()
            );
        } else {
            let current_status = session.SessionStatus().ok();
            log::warn!(
                "[embedded-ble] GATT session still not active after {} ms initial={:?} current={:?}; failing before GATT write",
                timeout.as_millis(),
                initial_status,
                current_status
            );
            let _ = session.Close();
            return Err(format!(
                "BLE GATT session did not become active after {} ms initial={:?} current={:?}; stale GATT/cache or paired device disconnected",
                timeout.as_millis(),
                initial_status,
                current_status
            ));
        }
        Ok(Some(session))
    }

    fn wait_gatt_session_ready(session: &GattSession, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if session
                .SessionStatus()
                .is_ok_and(|status| status == GattSessionStatus::Active)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(GATT_READY_POLL_INTERVAL);
        }
    }

    pub(super) fn parse_bluetooth_address_from_device_id(device_id: &str) -> Option<u64> {
        let upper = device_id.to_ascii_uppercase();
        for marker in ["DEV_", "_"] {
            let Some((_, suffix)) = upper.rsplit_once(marker) else {
                continue;
            };
            let hex: String = suffix
                .chars()
                .take_while(|ch| ch.is_ascii_hexdigit())
                .collect();
            if hex.len() == 12 {
                return u64::from_str_radix(&hex, 16).ok();
            }
        }
        for segment in upper.rsplit(|ch: char| matches!(ch, '\\' | '/' | '#' | '_' | '-')) {
            if let Some(address) = parse_bluetooth_address_hex_exact(segment) {
                return Some(address);
            }
        }
        None
    }

    fn configured_bluetooth_address() -> Option<u64> {
        for key in [
            "LISTENER_TYPE_BLE_ADDRESS",
            "LISTENER_TYPE_BLUETOOTH_ADDRESS",
        ] {
            let Ok(value) = std::env::var(key) else {
                continue;
            };
            if let Some(address) = parse_bluetooth_address_hex(&value) {
                return Some(address);
            }
            log::warn!("[embedded-ble] ignoring invalid {key}={value}");
        }
        None
    }

    pub fn set_configured_bluetooth_target_name(name: &str) {
        let trimmed = name.trim();
        let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_NAME
            .get_or_init(|| Mutex::new(None))
            .lock()
        else {
            log::warn!("[embedded-ble] target Bluetooth name slot is poisoned");
            return;
        };
        if trimmed.is_empty() {
            *slot = None;
        } else {
            *slot = Some(trimmed.to_string());
        }
    }

    fn configured_bluetooth_target_name() -> Option<String> {
        for key in [
            "LISTENER_TYPE_BLE_TARGET_NAME",
            "LISTENER_TYPE_BLUETOOTH_TARGET_NAME",
        ] {
            let Ok(value) = std::env::var(key) else {
                continue;
            };
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        RUNTIME_BLUETOOTH_TARGET_NAME
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    fn effective_bluetooth_target_name(expected_name: Option<&str>) -> String {
        expected_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(configured_bluetooth_target_name)
            .unwrap_or_else(|| DEFAULT_BLUETOOTH_TARGET_NAME.to_string())
    }

    fn push_unique_target_name(names: &mut Vec<String>, name: &str) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        if names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            return;
        }
        names.push(trimmed.to_string());
    }

    fn listener_target_names(extra_names: &[String]) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(name) = configured_bluetooth_target_name() {
            push_unique_target_name(&mut names, &name);
        }
        for name in extra_names {
            push_unique_target_name(&mut names, name);
        }
        if names.is_empty() {
            names.push(DEFAULT_BLUETOOTH_TARGET_NAME.to_string());
        }
        names
    }

    fn bluetooth_name_matches_expected(name: &str, expected_name: &str) -> bool {
        let trimmed = name.trim();
        !trimmed.is_empty() && trimmed.eq_ignore_ascii_case(expected_name.trim())
    }

    fn bluetooth_name_matches_any(name: &str, target_names: &[String]) -> bool {
        target_names
            .iter()
            .any(|target| bluetooth_name_matches_expected(name, target))
    }

    fn ble_candidate_allowed(kind: &str, index: u32, name: &str, address: Option<u64>) -> bool {
        if let Some(expected_address) = configured_bluetooth_address() {
            if address != Some(expected_address) {
                log::info!(
                    "[embedded-ble] skipping {kind} candidate index={index} name={name} address={}: configured address is {}",
                    address
                        .map(crate::embedded_ble::format_bluetooth_address)
                        .unwrap_or_else(|| "-".to_string()),
                    crate::embedded_ble::format_bluetooth_address(expected_address)
                );
                return false;
            }
        }

        if let Some(expected_name) = configured_bluetooth_target_name() {
            if !name.trim().eq_ignore_ascii_case(&expected_name) {
                log::info!(
                    "[embedded-ble] skipping {kind} candidate index={index} name={name:?} address={}: target name is {expected_name:?}",
                    address
                        .map(crate::embedded_ble::format_bluetooth_address)
                        .unwrap_or_else(|| "-".to_string())
                );
                return false;
            }
        }

        true
    }

    pub(super) fn parse_bluetooth_address_hex(value: &str) -> Option<u64> {
        parse_bluetooth_address_hex_exact(value)
    }

    fn parse_bluetooth_address_hex_exact(value: &str) -> Option<u64> {
        let hex: String = value.chars().filter(|ch| ch.is_ascii_hexdigit()).collect();
        if hex.len() != 12 {
            return None;
        }
        u64::from_str_radix(&hex, 16).ok()
    }

    fn buffer_to_vec(buffer: &IBuffer) -> windows::core::Result<Vec<u8>> {
        let length = buffer.Length()? as usize;
        let reader = DataReader::FromBuffer(buffer)?;
        let mut bytes = vec![0u8; length];
        reader.ReadBytes(&mut bytes)?;
        Ok(bytes)
    }

    fn read_characteristic_bytes(
        characteristic: &GattCharacteristic,
        cache_mode: BluetoothCacheMode,
        label: &str,
    ) -> Result<Vec<u8>, String> {
        let read = characteristic
            .ReadValueWithCacheModeAsync(cache_mode)
            .map_err(|err| format!("BLE {label} read failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE {label} read wait failed: {err}"))?;
        let status = read
            .Status()
            .map_err(|err| format!("BLE {label} read status failed: {err}"))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("BLE {label} read returned status={status:?}"));
        }
        buffer_to_vec(
            &read
                .Value()
                .map_err(|err| format!("BLE {label} read value failed: {err}"))?,
        )
        .map_err(|err| format!("BLE {label} read buffer failed: {err}"))
    }

    fn read_diagnostic_count(characteristic: &GattCharacteristic) -> Result<u32, String> {
        let bytes = read_characteristic_bytes(
            characteristic,
            BluetoothCacheMode::Uncached,
            "diagnostic count",
        )?;
        let text = String::from_utf8(bytes)
            .map_err(|err| format!("BLE diagnostic count was not UTF-8 JSON: {err}"))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|err| format!("BLE diagnostic count JSON parse failed: {err}"))?;
        let count = value
            .get("count")
            .and_then(|count| count.as_u64())
            .ok_or_else(|| "BLE diagnostic count JSON missing numeric count".to_string())?;
        u32::try_from(count).map_err(|_| format!("BLE diagnostic count too large: {count}"))
    }

    fn write_diagnostic_control(
        characteristic: &GattCharacteristic,
        payload: &str,
        timeout: Duration,
        op: &str,
    ) -> Result<(), String> {
        write_gatt_value_with_timeout(
            characteristic,
            payload.as_bytes(),
            GattWriteOption::WriteWithResponse,
            timeout,
            &format!("diagnostic control {op}"),
        )?;
        Ok(())
    }

    fn parse_diagnostic_chunk(
        packet: &[u8],
        host_offset: usize,
    ) -> Result<(u16, u16, u32, &[u8]), String> {
        if packet.len() < crate::embedded_ble::DIAGNOSTIC_CHUNK_HEADER_BYTES {
            return Err(format!(
                "BLE diagnostic notification too short at offset {host_offset}: {} bytes",
                packet.len()
            ));
        }
        let event_count = u16::from_le_bytes([packet[0], packet[1]]);
        let global_offset = u16::from_le_bytes([packet[2], packet[3]]);
        let firmware_crc = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
        if event_count == 0 {
            return Err(format!(
                "BLE diagnostic empty chunk at offset {host_offset}"
            ));
        }
        let payload = &packet[crate::embedded_ble::DIAGNOSTIC_CHUNK_HEADER_BYTES..];
        let expected_payload_len =
            usize::from(event_count) * crate::embedded_ble::DIAGNOSTIC_EVENT_BYTES;
        if payload.len() != expected_payload_len {
            return Err(format!(
                "BLE diagnostic chunk payload length mismatch: offset={host_offset} count={event_count} bytes={} expected={expected_payload_len}",
                payload.len()
            ));
        }
        Ok((event_count, global_offset, firmware_crc, payload))
    }

    fn write_cccd_with_timeout(
        characteristic: &GattCharacteristic,
        value: GattClientCharacteristicConfigurationDescriptorValue,
        timeout: Duration,
    ) -> Result<GattCommunicationStatus, String> {
        let operation = characteristic
            .WriteClientCharacteristicConfigurationDescriptorWithResultAsync(value)
            .map_err(|err| format!("BLE CCCD write failed: {err}"))?;
        let result = wait_gatt_write_result(operation, timeout, "CCCD")?;
        let status = result
            .Status()
            .map_err(|err| format!("BLE CCCD write status read failed: {err}"))?;
        let protocol_error = result
            .ProtocolError()
            .ok()
            .and_then(|value| value.Value().ok());
        if let Some(protocol_error) = protocol_error {
            log::warn!("[embedded-ble] CCCD write protocol_error={protocol_error}");
        }
        Ok(status)
    }

    fn write_cccd_notify_with_retry(
        capture_id: u64,
        label: &str,
        characteristic: &GattCharacteristic,
        timeout: Duration,
    ) -> Result<GattCommunicationStatus, String> {
        let mut last_error: Option<String> = None;
        let mut last_status: Option<GattCommunicationStatus> = None;
        for attempt in 1..=CCCD_ENABLE_RETRY_DELAYS.len() + 1 {
            match write_cccd_with_timeout(
                characteristic,
                GattClientCharacteristicConfigurationDescriptorValue::Notify,
                timeout,
            ) {
                Ok(GattCommunicationStatus::Success) => {
                    return Ok(GattCommunicationStatus::Success);
                }
                Ok(status) => {
                    if attempt > CCCD_ENABLE_RETRY_DELAYS.len() {
                        return Ok(status);
                    }
                    last_status = Some(status);
                    let delay = cccd_enable_retry_delay(attempt);
                    log::warn!(
                        "[embedded-ble] {label} #{capture_id}: notify CCCD enable attempt {attempt} returned status={status:?}; retrying in {} ms",
                        delay.as_millis()
                    );
                    std::thread::sleep(delay);
                }
                Err(err) => {
                    if attempt > CCCD_ENABLE_RETRY_DELAYS.len() {
                        return Err(err);
                    }
                    let delay = cccd_enable_retry_delay(attempt);
                    log::warn!(
                        "[embedded-ble] {label} #{capture_id}: notify CCCD enable attempt {attempt} failed: {err}; retrying in {} ms",
                        delay.as_millis()
                    );
                    last_error = Some(err);
                    std::thread::sleep(delay);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            format!(
                "BLE CCCD notify write returned status={:?}",
                last_status.unwrap_or(GattCommunicationStatus::Unreachable)
            )
        }))
    }

    fn write_cccd_indicate_with_retry(
        transfer_id: u64,
        label: &str,
        characteristic: &GattCharacteristic,
        timeout: Duration,
    ) -> Result<GattCommunicationStatus, String> {
        let mut last_error: Option<String> = None;
        let mut last_status: Option<GattCommunicationStatus> = None;
        for attempt in 1..=CCCD_ENABLE_RETRY_DELAYS.len() + 1 {
            match write_cccd_with_timeout(
                characteristic,
                GattClientCharacteristicConfigurationDescriptorValue::Indicate,
                timeout,
            ) {
                Ok(GattCommunicationStatus::Success) => {
                    return Ok(GattCommunicationStatus::Success);
                }
                Ok(status) => {
                    if attempt > CCCD_ENABLE_RETRY_DELAYS.len() {
                        return Ok(status);
                    }
                    last_status = Some(status);
                    let delay = cccd_enable_retry_delay(attempt);
                    log::warn!(
                        "[embedded-ble] {label} #{transfer_id}: indicate CCCD enable attempt {attempt} returned status={status:?}; retrying in {} ms",
                        delay.as_millis()
                    );
                    std::thread::sleep(delay);
                }
                Err(err) => {
                    if attempt > CCCD_ENABLE_RETRY_DELAYS.len() {
                        return Err(err);
                    }
                    let delay = cccd_enable_retry_delay(attempt);
                    log::warn!(
                        "[embedded-ble] {label} #{transfer_id}: indicate CCCD enable attempt {attempt} failed: {err}; retrying in {} ms",
                        delay.as_millis()
                    );
                    last_error = Some(err);
                    std::thread::sleep(delay);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            format!(
                "BLE CCCD indicate write returned status={:?}",
                last_status.unwrap_or(GattCommunicationStatus::Unreachable)
            )
        }))
    }

    fn cccd_enable_retry_delay(attempt: usize) -> Duration {
        CCCD_ENABLE_RETRY_DELAYS
            .get(attempt.saturating_sub(1))
            .copied()
            .unwrap_or_else(|| *CCCD_ENABLE_RETRY_DELAYS.last().expect("retry delays"))
    }

    fn write_gatt_value_with_timeout(
        characteristic: &GattCharacteristic,
        bytes: &[u8],
        write_option: GattWriteOption,
        timeout: Duration,
        label: &str,
    ) -> Result<GattCommunicationStatus, String> {
        let buffer = bytes_to_buffer(bytes)?;
        if write_option == GattWriteOption::WriteWithoutResponse {
            let operation = characteristic
                .WriteValueWithOptionAsync(&buffer, write_option)
                .map_err(|err| format!("BLE {label} write failed: {err}"))?;
            let status = wait_gatt_communication_status(operation, timeout, label)?;
            if status != GattCommunicationStatus::Success {
                return Err(format!("BLE {label} write returned status={status:?}"));
            }
            return Ok(status);
        }

        let operation = characteristic
            .WriteValueWithResultAndOptionAsync(&buffer, write_option)
            .map_err(|err| format!("BLE {label} write failed: {err}"))?;
        let result = wait_gatt_write_result(operation, timeout, label)?;
        let status = result
            .Status()
            .map_err(|err| format!("BLE {label} write status read failed: {err}"))?;
        let protocol_error = result
            .ProtocolError()
            .ok()
            .and_then(|value| value.Value().ok());
        if let Some(protocol_error) = protocol_error {
            log::warn!("[embedded-ble] {label} write protocol_error={protocol_error}");
        }
        if status != GattCommunicationStatus::Success {
            let protocol_suffix = protocol_error
                .map(|value| format!(" protocol_error={value}"))
                .unwrap_or_default();
            return Err(format!(
                "BLE {label} write returned status={status:?}{protocol_suffix}"
            ));
        }
        Ok(status)
    }

    fn write_gatt_value_status_with_timeout(
        characteristic: &GattCharacteristic,
        bytes: &[u8],
        write_option: GattWriteOption,
        timeout: Duration,
        label: &str,
    ) -> Result<GattCommunicationStatus, String> {
        let buffer = bytes_to_buffer(bytes)?;
        let operation = characteristic
            .WriteValueWithOptionAsync(&buffer, write_option)
            .map_err(|err| format!("BLE {label} write failed: {err}"))?;
        let status = wait_gatt_communication_status(operation, timeout, label)?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("BLE {label} write returned status={status:?}"));
        }
        Ok(status)
    }

    fn bytes_to_buffer(bytes: &[u8]) -> Result<IBuffer, String> {
        let writer = DataWriter::new().map_err(|err| format!("BLE buffer writer failed: {err}"))?;
        writer
            .WriteBytes(bytes)
            .map_err(|err| format!("BLE buffer write failed: {err}"))?;
        writer
            .DetachBuffer()
            .map_err(|err| format!("BLE buffer detach failed: {err}"))
    }

    fn wait_async_operation<T: windows::core::RuntimeType>(
        operation: IAsyncOperation<T>,
        timeout: Duration,
        label: &str,
    ) -> Result<T, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match operation
                .Status()
                .map_err(|err| format!("BLE {label} async status failed: {err}"))?
            {
                AsyncStatus::Completed => {
                    return operation
                        .GetResults()
                        .map_err(|err| format!("BLE {label} async result failed: {err}"));
                }
                AsyncStatus::Error => {
                    let code = operation.ErrorCode().ok();
                    let _ = operation.Close();
                    return Err(format!("BLE {label} async error: {code:?}"));
                }
                AsyncStatus::Canceled => {
                    let _ = operation.Close();
                    return Err(format!("BLE {label} async canceled"));
                }
                AsyncStatus::Started => {
                    if Instant::now() >= deadline {
                        let _ = operation.Cancel();
                        let _ = operation.Close();
                        return Err(format!(
                            "BLE {label} timed out after {} ms",
                            timeout.as_millis()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                status => {
                    let _ = operation.Close();
                    return Err(format!("BLE {label} unknown async status={status:?}"));
                }
            }
        }
    }

    fn wait_gatt_write_result(
        operation: IAsyncOperation<GattWriteResult>,
        timeout: Duration,
        label: &str,
    ) -> Result<GattWriteResult, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match operation
                .Status()
                .map_err(|err| format!("BLE {label} write async status failed: {err}"))?
            {
                AsyncStatus::Completed => {
                    return operation
                        .GetResults()
                        .map_err(|err| format!("BLE {label} write result failed: {err}"));
                }
                AsyncStatus::Error => {
                    let code = operation.ErrorCode().ok();
                    let _ = operation.Close();
                    return Err(format!("BLE {label} write async error: {code:?}"));
                }
                AsyncStatus::Canceled => {
                    let _ = operation.Close();
                    return Err(format!("BLE {label} write async canceled"));
                }
                AsyncStatus::Started => {
                    if Instant::now() >= deadline {
                        let _ = operation.Cancel();
                        let _ = operation.Close();
                        return Err(format!(
                            "BLE {label} write timed out after {} ms",
                            timeout.as_millis()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                status => {
                    let _ = operation.Close();
                    return Err(format!("BLE {label} write unknown async status={status:?}"));
                }
            }
        }
    }

    fn wait_gatt_communication_status(
        operation: IAsyncOperation<GattCommunicationStatus>,
        timeout: Duration,
        label: &str,
    ) -> Result<GattCommunicationStatus, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match operation
                .Status()
                .map_err(|err| format!("BLE {label} write async status failed: {err}"))?
            {
                AsyncStatus::Completed => {
                    return operation
                        .GetResults()
                        .map_err(|err| format!("BLE {label} write result failed: {err}"));
                }
                AsyncStatus::Error => {
                    let code = operation.ErrorCode().ok();
                    let _ = operation.Close();
                    return Err(format!("BLE {label} write async error: {code:?}"));
                }
                AsyncStatus::Canceled => {
                    let _ = operation.Close();
                    return Err(format!("BLE {label} write async canceled"));
                }
                AsyncStatus::Started => {
                    if Instant::now() >= deadline {
                        let _ = operation.Cancel();
                        let _ = operation.Close();
                        return Err(format!(
                            "BLE {label} write timed out after {} ms",
                            timeout.as_millis()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                status => {
                    let _ = operation.Close();
                    return Err(format!("BLE {label} write unknown async status={status:?}"));
                }
            }
        }
    }

    fn json_escape(value: &str) -> String {
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    }

    #[derive(Default)]
    struct BleCaptureGateState {
        next_session_id: u64,
        last_closed_at: Option<Instant>,
    }

    fn capture_gate() -> &'static Mutex<BleCaptureGateState> {
        static GATE: OnceLock<Mutex<BleCaptureGateState>> = OnceLock::new();
        GATE.get_or_init(|| Mutex::new(BleCaptureGateState::default()))
    }

    struct BleCaptureGuard {
        guard: MutexGuard<'static, BleCaptureGateState>,
        session_id: u64,
        started_at: Instant,
    }

    impl BleCaptureGuard {
        fn enter(idle_timeout: Option<Duration>) -> Result<Self, String> {
            let mut guard = capture_gate()
                .lock()
                .map_err(|_| "BLE capture gate is poisoned".to_string())?;
            if let Some(last_closed_at) = guard.last_closed_at {
                let elapsed = last_closed_at.elapsed();
                if elapsed < RECONNECT_COOLDOWN {
                    let delay = RECONNECT_COOLDOWN - elapsed;
                    log::info!(
                        "[embedded-ble] waiting {} ms before reconnect after previous capture cleanup",
                        delay.as_millis()
                    );
                    std::thread::sleep(delay);
                }
            }
            guard.next_session_id = guard.next_session_id.wrapping_add(1);
            if guard.next_session_id == 0 {
                guard.next_session_id = 1;
            }
            let session_id = guard.next_session_id;
            match idle_timeout {
                Some(timeout) => log::info!(
                    "[embedded-ble] capture #{session_id}: opening serialized BLE notify session timeout_ms={}",
                    timeout.as_millis()
                ),
                None => log::info!(
                    "[embedded-ble] capture #{session_id}: opening serialized BLE notify session timeout_ms=none"
                ),
            }
            Ok(Self {
                guard,
                session_id,
                started_at: Instant::now(),
            })
        }

        fn session_id(&self) -> u64 {
            self.session_id
        }
    }

    impl Drop for BleCaptureGuard {
        fn drop(&mut self) {
            let elapsed_ms = self.started_at.elapsed().as_millis();
            self.guard.last_closed_at = Some(Instant::now());
            log::info!(
                "[embedded-ble] capture #{}: released BLE notify session after {} ms",
                self.session_id,
                elapsed_ms
            );
        }
    }

    struct OpenNotifyTarget {
        characteristic: GattCharacteristic,
        control: Option<GattCharacteristic>,
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
    }

    struct OpenAudioControlTarget {
        control: GattCharacteristic,
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
    }

    struct PreparedNotifyCharacteristic {
        characteristic: GattCharacteristic,
        control: Option<GattCharacteristic>,
        session: Option<GattSession>,
    }

    struct PreparedAudioControlCharacteristic {
        control: GattCharacteristic,
        session: Option<GattSession>,
    }

    struct OpenOtaTarget {
        control: GattCharacteristic,
        data: GattCharacteristic,
        data_write_option: GattWriteOption,
        data_chunk_bytes: usize,
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
        bluetooth_address: Option<u64>,
    }

    #[cfg(feature = "companion-dev")]
    struct OpenStm32wbStOtaTarget {
        base: GattCharacteristic,
        raw: GattCharacteristic,
        confirm: GattCharacteristic,
        base_write_option: GattWriteOption,
        raw_write_option: GattWriteOption,
        raw_chunk_bytes: usize,
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
        bluetooth_address: Option<u64>,
    }

    struct OpenDiagnosticTarget {
        control: GattCharacteristic,
        data: GattCharacteristic,
        count: GattCharacteristic,
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
    }

    enum DiagnosticTargetCandidate {
        Device { label: String, address: u64 },
        Service { label: String, id: HSTRING },
    }

    impl DiagnosticTargetCandidate {
        fn label(&self) -> &str {
            match self {
                DiagnosticTargetCandidate::Device { label, .. }
                | DiagnosticTargetCandidate::Service { label, .. } => label,
            }
        }

        fn open(&self) -> Result<OpenDiagnosticTarget, String> {
            match self {
                DiagnosticTargetCandidate::Device { address, .. } => {
                    open_diagnostic_target_for_device(*address)
                }
                DiagnosticTargetCandidate::Service { id, .. } => {
                    open_diagnostic_target_for_service(id)
                }
            }
        }
    }

    struct PreparedOtaCharacteristics {
        control: GattCharacteristic,
        data: GattCharacteristic,
        data_write_option: GattWriteOption,
        data_chunk_bytes: usize,
        session: Option<GattSession>,
    }

    #[cfg(feature = "companion-dev")]
    struct PreparedStm32wbStOtaCharacteristics {
        base: GattCharacteristic,
        raw: GattCharacteristic,
        confirm: GattCharacteristic,
        base_write_option: GattWriteOption,
        raw_write_option: GattWriteOption,
        raw_chunk_bytes: usize,
        session: Option<GattSession>,
    }

    struct PreparedDiagnosticCharacteristics {
        control: GattCharacteristic,
        data: GattCharacteristic,
        count: GattCharacteristic,
        session: Option<GattSession>,
    }

    impl Drop for OpenOtaTarget {
        fn drop(&mut self) {
            if let Some(session) = self.session.take() {
                let _ = session.Close();
            }
            if let Some(service) = self.service.take() {
                let _ = service.Close();
            }
            if let Some(device) = self.device.take() {
                let _ = device.Close();
            }
        }
    }

    #[cfg(feature = "companion-dev")]
    impl Drop for OpenStm32wbStOtaTarget {
        fn drop(&mut self) {
            if let Some(session) = self.session.take() {
                let _ = session.Close();
            }
            if let Some(service) = self.service.take() {
                let _ = service.Close();
            }
            if let Some(device) = self.device.take() {
                let _ = device.Close();
            }
        }
    }

    impl Drop for OpenOtaV2Target {
        fn drop(&mut self) {
            if let Some(session) = self.session.take() {
                let _ = session.Close();
            }
            if let Some(service) = self.service.take() {
                let _ = service.Close();
            }
            if let Some(device) = self.device.take() {
                let _ = device.Close();
            }
        }
    }

    #[cfg(feature = "companion-dev")]
    struct Stm32wbStOtaHandoffCleanup {
        transfer_id: u64,
        device: Option<BluetoothLEDevice>,
        session: Option<GattSession>,
        connection_status_token: Option<EventRegistrationToken>,
        session_status_token: Option<EventRegistrationToken>,
    }

    #[cfg(feature = "companion-dev")]
    impl Stm32wbStOtaHandoffCleanup {
        fn install(
            transfer_id: u64,
            target: &OpenStm32wbStOtaTarget,
            tx: mpsc::Sender<String>,
        ) -> Self {
            let mut cleanup = Self {
                transfer_id,
                device: None,
                session: None,
                connection_status_token: None,
                session_status_token: None,
            };

            if let Some(device) = target.device.as_ref() {
                if let Ok(status) = device.ConnectionStatus() {
                    log::info!(
                        "[embedded-ble] STM32WB ST OTA #{transfer_id}: device connection status before finish={status:?}"
                    );
                    if status == BluetoothConnectionStatus::Disconnected {
                        let _ = tx.send("device already disconnected before finish".to_string());
                    }
                }
                let handler_tx = tx.clone();
                let handler = TypedEventHandler::<BluetoothLEDevice, IInspectable>::new(
                    move |sender, _| {
                        if let Some(device) = sender {
                            match device.ConnectionStatus() {
                                Ok(status) => {
                                    log::info!(
                                        "[embedded-ble] STM32WB ST OTA #{transfer_id}: device connection status changed to {status:?}"
                                    );
                                    if status == BluetoothConnectionStatus::Disconnected {
                                        let _ = handler_tx.send(
                                            "device connection changed to Disconnected".to_string(),
                                        );
                                    }
                                }
                                Err(err) => {
                                    let _ = handler_tx.send(format!(
                                        "device connection status read failed after finish: {err}"
                                    ));
                                }
                            }
                        }
                        Ok(())
                    },
                );
                match device.ConnectionStatusChanged(&handler) {
                    Ok(token) => {
                        cleanup.device = Some(device.clone());
                        cleanup.connection_status_token = Some(token);
                    }
                    Err(err) => log::warn!(
                        "[embedded-ble] STM32WB ST OTA #{transfer_id}: device connection status handler registration failed: {err}"
                    ),
                }
            }

            if let Some(session) = target.session.as_ref() {
                if let Ok(status) = session.SessionStatus() {
                    log::info!(
                        "[embedded-ble] STM32WB ST OTA #{transfer_id}: GATT session status before finish={status:?}"
                    );
                    if status != GattSessionStatus::Active {
                        let _ = tx.send(format!(
                            "GATT session already non-active before finish: {status:?}"
                        ));
                    }
                }
                let handler_tx = tx;
                let handler =
                    TypedEventHandler::<GattSession, GattSessionStatusChangedEventArgs>::new(
                        move |_sender, args| {
                            if let Some(args) = args {
                                let status = args.Status().ok();
                                let error = args.Error().ok();
                                log::info!(
                                    "[embedded-ble] STM32WB ST OTA #{transfer_id}: GATT session status changed status={status:?} error={error:?}"
                                );
                                if status.is_some_and(|status| status != GattSessionStatus::Active)
                                {
                                    let _ = handler_tx.send(format!(
                                        "GATT session changed to {status:?} error={error:?}"
                                    ));
                                }
                            }
                            Ok(())
                        },
                    );
                match session.SessionStatusChanged(&handler) {
                    Ok(token) => {
                        cleanup.session = Some(session.clone());
                        cleanup.session_status_token = Some(token);
                    }
                    Err(err) => log::warn!(
                        "[embedded-ble] STM32WB ST OTA #{transfer_id}: GATT session status handler registration failed: {err}"
                    ),
                }
            }

            cleanup
        }

        fn finish(&mut self) {
            if let Some(token) = self.connection_status_token.take() {
                if let Some(device) = self.device.as_ref() {
                    if let Err(err) = device.RemoveConnectionStatusChanged(token) {
                        log::warn!(
                            "[embedded-ble] STM32WB ST OTA #{}: device connection status handler remove failed: {err}",
                            self.transfer_id
                        );
                    }
                }
            }
            if let Some(token) = self.session_status_token.take() {
                if let Some(session) = self.session.as_ref() {
                    if let Err(err) = session.RemoveSessionStatusChanged(token) {
                        log::warn!(
                            "[embedded-ble] STM32WB ST OTA #{}: GATT session status handler remove failed: {err}",
                            self.transfer_id
                        );
                    }
                }
            }
        }
    }

    #[cfg(feature = "companion-dev")]
    impl Drop for Stm32wbStOtaHandoffCleanup {
        fn drop(&mut self) {
            self.finish();
        }
    }

    #[cfg(feature = "companion-dev")]
    struct Stm32wbStOtaConfirmCleanup {
        characteristic: GattCharacteristic,
        token: Option<EventRegistrationToken>,
        cccd_enabled: bool,
    }

    #[cfg(feature = "companion-dev")]
    impl Stm32wbStOtaConfirmCleanup {
        fn finish(&mut self) {
            if let Some(token) = self.token.take() {
                if let Err(err) = self.characteristic.RemoveValueChanged(token) {
                    log::warn!(
                        "[embedded-ble] STM32WB ST OTA confirm ValueChanged remove failed: {err}"
                    );
                }
            }
            if self.cccd_enabled {
                match self
                    .characteristic
                    .WriteClientCharacteristicConfigurationDescriptorWithResultAsync(
                        GattClientCharacteristicConfigurationDescriptorValue::None,
                    ) {
                    Ok(operation) => {
                        if let Err(err) = wait_gatt_write_result(
                            operation,
                            Duration::from_secs(2),
                            "STM32WB ST OTA confirm CCCD",
                        ) {
                            log::warn!(
                                "[embedded-ble] STM32WB ST OTA confirm CCCD disable failed: {err}"
                            );
                        }
                    }
                    Err(err) => {
                        log::warn!(
                            "[embedded-ble] STM32WB ST OTA confirm CCCD disable launch failed: {err}"
                        );
                    }
                }
                self.cccd_enabled = false;
            }
        }
    }

    #[cfg(feature = "companion-dev")]
    impl Drop for Stm32wbStOtaConfirmCleanup {
        fn drop(&mut self) {
            self.finish();
        }
    }

    impl Drop for OpenAudioControlTarget {
        fn drop(&mut self) {
            if let Some(session) = self.session.take() {
                let _ = session.Close();
            }
            if let Some(service) = self.service.take() {
                let _ = service.Close();
            }
            if let Some(device) = self.device.take() {
                let _ = device.Close();
            }
        }
    }

    impl Drop for OpenDiagnosticTarget {
        fn drop(&mut self) {
            if let Some(session) = self.session.take() {
                let _ = session.Close();
            }
            if let Some(service) = self.service.take() {
                let _ = service.Close();
            }
            if let Some(device) = self.device.take() {
                let _ = device.Close();
            }
        }
    }

    struct DiagnosticNotifyCleanup {
        target: OpenDiagnosticTarget,
        token: Option<EventRegistrationToken>,
        notify_disabled: bool,
    }

    impl DiagnosticNotifyCleanup {
        fn new(target: OpenDiagnosticTarget) -> Self {
            Self {
                target,
                token: None,
                notify_disabled: false,
            }
        }

        fn set_token(&mut self, token: EventRegistrationToken) {
            self.token = Some(token);
        }

        fn disable_notify(&mut self) {
            if self.notify_disabled {
                return;
            }
            if let Some(token) = self.token.take() {
                if let Err(err) = self.target.data.RemoveValueChanged(token) {
                    log::warn!(
                        "[embedded-ble] diagnostic log ValueChanged handler remove failed: {err}"
                    );
                }
            }
            match self
                .target
                .data
                .WriteClientCharacteristicConfigurationDescriptorWithResultAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::None,
                ) {
                Ok(operation) => {
                    if let Err(err) =
                        wait_gatt_write_result(operation, Duration::from_secs(2), "diagnostic CCCD")
                    {
                        log::warn!("[embedded-ble] diagnostic log CCCD disable skipped: {err}");
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] diagnostic log CCCD disable operation could not start: {err}"
                    );
                }
            }
            self.notify_disabled = true;
        }
    }

    impl Drop for DiagnosticNotifyCleanup {
        fn drop(&mut self) {
            self.disable_notify();
        }
    }

    struct NotifyCleanup {
        capture_id: u64,
        target: OpenNotifyTarget,
        token: Option<EventRegistrationToken>,
        connection_status_token: Option<EventRegistrationToken>,
        session_status_token: Option<EventRegistrationToken>,
        audio_control_registration: Option<ActiveAudioControlRegistration>,
        type_heartbeat_open: bool,
        notify_disabled: bool,
    }

    impl NotifyCleanup {
        fn new(capture_id: u64, target: OpenNotifyTarget) -> Self {
            Self {
                capture_id,
                target,
                token: None,
                connection_status_token: None,
                session_status_token: None,
                audio_control_registration: None,
                type_heartbeat_open: false,
                notify_disabled: false,
            }
        }

        fn set_token(&mut self, token: EventRegistrationToken) {
            self.token = Some(token);
        }

        fn set_connection_status_token(&mut self, token: EventRegistrationToken) {
            self.connection_status_token = Some(token);
        }

        fn set_session_status_token(&mut self, token: EventRegistrationToken) {
            self.session_status_token = Some(token);
        }

        fn set_audio_control_registration(&mut self, registration: ActiveAudioControlRegistration) {
            self.audio_control_registration = Some(registration);
        }

        fn mark_type_heartbeat_open(&mut self) {
            self.type_heartbeat_open = true;
        }

        fn defer_type_heartbeat_bye_until_processing_done(&mut self) {
            self.type_heartbeat_open = false;
        }

        fn write_type_heartbeat(&self, command: &[u8], label: &str) -> Result<(), String> {
            let Some(control) = self.target.control.as_ref() else {
                return Err("audio control unavailable".to_string());
            };
            let write_option = type_heartbeat_write_option(control, label);

            match write_gatt_value_with_timeout(
                control,
                command,
                write_option,
                TYPE_HEARTBEAT_WRITE_TIMEOUT,
                label,
            ) {
                Ok(_) => {
                    if label == "Type heartbeat" {
                        log::debug!("[embedded-ble] capture #{}: {label} sent", self.capture_id);
                    } else {
                        log::info!("[embedded-ble] capture #{}: {label} sent", self.capture_id);
                    }
                    Ok(())
                }
                Err(err) => Err(format!("{label} failed: {err}")),
            }
        }

        fn disable_notify(&mut self) {
            self.finish(NotifyCccdTeardown::Disable);
        }

        fn finish(&mut self, teardown: NotifyCccdTeardown) {
            if self.notify_disabled {
                return;
            }
            if self.type_heartbeat_open {
                let _ = self.write_type_heartbeat(b"TYPE:BYE\n", "Type heartbeat bye");
                self.type_heartbeat_open = false;
            }
            self.remove_status_handlers();
            self.remove_handler();
            match teardown {
                NotifyCccdTeardown::Disable => {
                    log::info!(
                        "[embedded-ble] capture #{}: disabling notify CCCD",
                        self.capture_id
                    );
                    match self
                        .target
                        .characteristic
                        .WriteClientCharacteristicConfigurationDescriptorWithResultAsync(
                            GattClientCharacteristicConfigurationDescriptorValue::None,
                        ) {
                        Ok(operation) => {
                            match wait_gatt_write_result(operation, Duration::from_secs(2), "CCCD")
                            {
                                Ok(status) => log::info!(
                                    "[embedded-ble] capture #{}: notify CCCD disabled status={status:?}",
                                    self.capture_id
                                ),
                                Err(err) => log::warn!(
                                    "[embedded-ble] capture #{}: notify CCCD disable skipped: {err}",
                                    self.capture_id
                                ),
                            }
                        }
                        Err(err) => {
                            log::warn!(
                                "[embedded-ble] capture #{}: notify CCCD disable operation could not start: {err}",
                                self.capture_id
                            );
                        }
                    }
                }
                NotifyCccdTeardown::LeaveEnabled => {
                    log::info!(
                        "[embedded-ble] capture #{}: leaving notify CCCD enabled after readiness probe",
                        self.capture_id
                    );
                }
            }
            let _ = self.audio_control_registration.take();
            self.notify_disabled = true;
        }

        fn handle_audio_control_request(&self, request: AudioControlRequest) {
            let result = match self.target.control.as_ref() {
                Some(control) => write_audio_control_value_with_timeout(
                    control,
                    &request.bytes,
                    request.timeout,
                    &request.label,
                ),
                None => Err(
                    "active Listener BLE capture has no audio control characteristic".to_string(),
                ),
            };
            let _ = request.result_tx.send(result);
        }

        fn log_embedded_audio_status_snapshot(&self, label: &str) {
            let Some(service) = self.target.service.as_ref() else {
                log::warn!(
                    "[embedded-ble] capture #{}: {label} status snapshot unavailable: service not retained",
                    self.capture_id
                );
                return;
            };
            let status = read_embedded_audio_status_from_service(service);
            log::info!(
                "[embedded-ble] capture #{}: {label} status readiness={:?} capabilities={:?}",
                self.capture_id,
                status.readiness,
                status.capabilities
            );
        }

        fn remove_status_handlers(&mut self) {
            if let Some(token) = self.connection_status_token.take() {
                if let Some(device) = self.target.device.as_ref() {
                    match device.RemoveConnectionStatusChanged(token) {
                        Ok(()) => log::info!(
                            "[embedded-ble] capture #{}: connection status handler removed",
                            self.capture_id
                        ),
                        Err(err) => log::warn!(
                            "[embedded-ble] capture #{}: connection status handler remove failed: {err}",
                            self.capture_id
                        ),
                    }
                }
            }
            if let Some(token) = self.session_status_token.take() {
                if let Some(session) = self.target.session.as_ref() {
                    match session.RemoveSessionStatusChanged(token) {
                        Ok(()) => log::info!(
                            "[embedded-ble] capture #{}: GATT session status handler removed",
                            self.capture_id
                        ),
                        Err(err) => log::warn!(
                            "[embedded-ble] capture #{}: GATT session status handler remove failed: {err}",
                            self.capture_id
                        ),
                    }
                }
            }
        }

        fn remove_handler(&mut self) {
            if let Some(token) = self.token.take() {
                match self.target.characteristic.RemoveValueChanged(token) {
                    Ok(()) => log::info!(
                        "[embedded-ble] capture #{}: ValueChanged handler removed",
                        self.capture_id
                    ),
                    Err(err) => log::warn!(
                        "[embedded-ble] capture #{}: ValueChanged handler remove failed: {err}",
                        self.capture_id
                    ),
                }
            }
        }
    }

    fn type_heartbeat_write_option(control: &GattCharacteristic, _label: &str) -> GattWriteOption {
        let Ok(properties) = control.CharacteristicProperties() else {
            return GattWriteOption::WriteWithResponse;
        };
        type_heartbeat_write_option_from_properties(properties)
    }

    fn type_heartbeat_write_option_from_properties(
        properties: GattCharacteristicProperties,
    ) -> GattWriteOption {
        if properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
            GattWriteOption::WriteWithoutResponse
        } else {
            GattWriteOption::WriteWithResponse
        }
    }

    fn write_audio_control_value_with_timeout(
        control: &GattCharacteristic,
        bytes: &[u8],
        timeout: Duration,
        label: &str,
    ) -> Result<(), String> {
        let (primary, fallback) = audio_control_write_options(control, label)?;
        match write_gatt_value_with_timeout(control, bytes, primary, timeout, label) {
            Ok(_) => Ok(()),
            Err(primary_err) => {
                let Some(fallback) = fallback else {
                    return Err(primary_err);
                };
                log::warn!(
                    "[embedded-ble] {label} failed with {primary:?}: {primary_err}; retrying with {fallback:?}"
                );
                write_gatt_value_with_timeout(control, bytes, fallback, timeout, label)
                    .map(|_| ())
                    .map_err(|fallback_err| {
                        format!(
                            "{primary_err}; fallback {fallback:?} for {label} also failed: {fallback_err}"
                        )
                    })
            }
        }
    }

    fn audio_control_write_options(
        control: &GattCharacteristic,
        label: &str,
    ) -> Result<(GattWriteOption, Option<GattWriteOption>), String> {
        let properties = control
            .CharacteristicProperties()
            .map_err(|err| format!("BLE {label} characteristic properties read failed: {err}"))?;
        audio_control_write_options_from_properties(properties).ok_or_else(|| {
            "BLE audio control characteristic must support Write or WriteWithoutResponse."
                .to_string()
        })
    }

    fn audio_control_write_options_from_properties(
        properties: GattCharacteristicProperties,
    ) -> Option<(GattWriteOption, Option<GattWriteOption>)> {
        let supports_write = properties.contains(GattCharacteristicProperties::Write);
        let supports_without_response =
            properties.contains(GattCharacteristicProperties::WriteWithoutResponse);
        if supports_without_response {
            return Some((
                GattWriteOption::WriteWithoutResponse,
                supports_write.then_some(GattWriteOption::WriteWithResponse),
            ));
        }
        if supports_write {
            return Some((
                GattWriteOption::WriteWithoutResponse,
                Some(GattWriteOption::WriteWithResponse),
            ));
        }
        None
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum NotifyCccdTeardown {
        Disable,
        LeaveEnabled,
    }

    impl NotifyCccdTeardown {
        fn for_probe_success() -> Self {
            Self::Disable
        }
    }

    impl Drop for NotifyCleanup {
        fn drop(&mut self) {
            self.disable_notify();
            if let Some(session) = self.target.session.take() {
                let _ = session.Close();
            }
            if let Some(service) = self.target.service.take() {
                let _ = service.Close();
            }
            if let Some(device) = self.target.device.take() {
                let _ = device.Close();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn active_audio_control_test_lock() -> &'static Mutex<()> {
            static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            LOCK.get_or_init(|| Mutex::new(()))
        }

        #[cfg(feature = "companion-dev")]
        fn stm32wb_st_ota_confirm_env_test_lock() -> &'static Mutex<()> {
            static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            LOCK.get_or_init(|| Mutex::new(()))
        }

        #[test]
        #[cfg(feature = "companion-dev")]
        fn stm32wb_st_ota_confirm_indication_defaults_on_and_accepts_explicit_opt_out() {
            let _guard = stm32wb_st_ota_confirm_env_test_lock().lock().unwrap();
            const ENV_NAME: &str = "COMPANION_STM32WB_ST_OTA_CONFIRM";
            let previous = std::env::var_os(ENV_NAME);

            std::env::remove_var(ENV_NAME);
            assert!(stm32wb_st_ota_use_confirm_indication());

            for value in ["0", "false", "no", "off", " OFF "] {
                std::env::set_var(ENV_NAME, value);
                assert!(!stm32wb_st_ota_use_confirm_indication(), "{value}");
            }

            for value in ["1", "true", "yes", "on", "unexpected"] {
                std::env::set_var(ENV_NAME, value);
                assert!(stm32wb_st_ota_use_confirm_indication(), "{value}");
            }

            match previous {
                Some(value) => std::env::set_var(ENV_NAME, value),
                None => std::env::remove_var(ENV_NAME),
            }
        }

        #[test]
        fn ota_write_option_prefers_with_response_and_accepts_overrides() {
            let both = GattCharacteristicProperties::Write
                | GattCharacteristicProperties::WriteWithoutResponse;

            assert_eq!(
                ota_data_write_option_from_request(both, None),
                Ok(GattWriteOption::WriteWithResponse)
            );
            assert_eq!(
                ota_data_write_option_from_request(both, Some("without-response")),
                Ok(GattWriteOption::WriteWithoutResponse)
            );
            assert!(ota_data_write_option_from_request(
                GattCharacteristicProperties::WriteWithoutResponse,
                Some("with-response")
            )
            .is_err());
        }

        #[test]
        fn type_heartbeat_prefers_no_response_when_available() {
            let both = GattCharacteristicProperties::Write
                | GattCharacteristicProperties::WriteWithoutResponse;
            assert_eq!(
                type_heartbeat_write_option_from_properties(both),
                GattWriteOption::WriteWithoutResponse
            );
            assert_eq!(
                type_heartbeat_write_option_from_properties(GattCharacteristicProperties::Write),
                GattWriteOption::WriteWithResponse
            );
        }

        #[test]
        fn audio_control_prefers_no_response_when_available() {
            let both = GattCharacteristicProperties::Write
                | GattCharacteristicProperties::WriteWithoutResponse;
            assert_eq!(
                audio_control_write_options_from_properties(both),
                Some((
                    GattWriteOption::WriteWithoutResponse,
                    Some(GattWriteOption::WriteWithResponse)
                ))
            );
            assert_eq!(
                audio_control_write_options_from_properties(GattCharacteristicProperties::Write),
                Some((
                    GattWriteOption::WriteWithoutResponse,
                    Some(GattWriteOption::WriteWithResponse)
                ))
            );
            assert_eq!(
                audio_control_write_options_from_properties(
                    GattCharacteristicProperties::WriteWithoutResponse
                ),
                Some((GattWriteOption::WriteWithoutResponse, None))
            );
        }

        #[test]
        #[cfg(feature = "companion-dev")]
        fn stm32wb_st_ota_prefers_with_response_when_available() {
            let both = GattCharacteristicProperties::Write
                | GattCharacteristicProperties::WriteWithoutResponse;
            assert_eq!(
                stm32wb_st_ota_write_option(both),
                Ok(GattWriteOption::WriteWithResponse)
            );
            assert_eq!(
                stm32wb_st_ota_write_option(GattCharacteristicProperties::Write),
                Ok(GattWriteOption::WriteWithResponse)
            );
            assert_eq!(
                stm32wb_st_ota_write_option(GattCharacteristicProperties::WriteWithoutResponse),
                Ok(GattWriteOption::WriteWithoutResponse)
            );
        }

        #[test]
        #[cfg(feature = "companion-dev")]
        fn stm32wb_st_ota_advertisement_name_is_case_insensitive() {
            assert!(stm32wb_st_ota_advertisement_name_matches("companion"));
            assert!(stm32wb_st_ota_advertisement_name_matches(" Companion "));
            assert!(!stm32wb_st_ota_advertisement_name_matches("listener"));
        }

        #[test]
        fn ble_advertisement_name_matching_trims_expected_target() {
            assert!(ble_advertisement_name_matches(" Companion ", "companion"));
            assert!(ble_advertisement_name_matches("companion", " Companion "));
            assert!(!ble_advertisement_name_matches("Blistener", "companion"));
        }

        #[test]
        fn listener_pairing_name_requires_exact_configured_target() {
            assert!(listener_pairing_name_matches(
                "OfficeType01",
                Some("OfficeType01")
            ));
            assert!(!listener_pairing_name_matches(
                "Blistener",
                Some("OfficeType01")
            ));
            assert!(!listener_pairing_name_matches(
                "listenerB",
                Some("OfficeType01")
            ));
        }

        #[test]
        fn listener_target_name_set_does_not_fuzzy_match_listener_family() {
            let target_names = vec!["Blistener".to_string(), "OfficeType01".to_string()];
            assert!(bluetooth_name_matches_any("Blistener", &target_names));
            assert!(bluetooth_name_matches_any("OfficeType01", &target_names));
            assert!(!bluetooth_name_matches_any("listenerB", &target_names));
            assert!(!bluetooth_name_matches_any(
                "some-listener-device",
                &target_names
            ));
        }

        #[test]
        #[cfg(feature = "companion-dev")]
        fn stm32wb_st_ota_base_address_command_uses_st_24bit_flash_offset() {
            assert_eq!(
                stm32wb_st_ota_base_address_command(0x0800_7000),
                [STM32WB_ST_OTA_APPLICATION_UPLOAD, 0x00, 0x70, 0x00]
            );
        }

        #[test]
        #[cfg(feature = "companion-dev")]
        fn stm32wb_st_ota_uses_large_confirmed_chunks_and_caps_no_response() {
            assert_eq!(
                stm32wb_st_ota_raw_chunk_bytes(None, GattWriteOption::WriteWithoutResponse),
                ATT_DEFAULT_PAYLOAD_BYTES
            );
            assert_eq!(
                stm32wb_st_ota_raw_chunk_bytes(None, GattWriteOption::WriteWithResponse),
                ATT_DEFAULT_PAYLOAD_BYTES
            );
            assert_eq!(
                stm32wb_st_ota_raw_chunk_bytes_from_payload(
                    248,
                    GattWriteOption::WriteWithoutResponse
                ),
                ATT_DEFAULT_PAYLOAD_BYTES
            );
            assert_eq!(
                stm32wb_st_ota_raw_chunk_bytes_from_payload(
                    248,
                    GattWriteOption::WriteWithResponse
                ),
                STM32WB_ST_OTA_RAW_DATA_SIZE
            );
        }

        #[test]
        fn ota_transfer_chunk_selection_uses_transport_and_test_override() {
            assert_eq!(
                ota_transfer_chunk_bytes_with_override(514, 500, None),
                Ok(500)
            );
            assert_eq!(
                ota_transfer_chunk_bytes_with_override(244, 500, None),
                Ok(244)
            );
            assert_eq!(
                ota_transfer_chunk_bytes_with_override(514, 500, Some(244)),
                Ok(244)
            );
            assert!(ota_transfer_chunk_bytes_with_override(0, 500, None).is_err());
            assert!(ota_data_test_chunk_bytes_override_from(Some("0")).is_err());
            assert_eq!(
                ota_data_inter_chunk_delay_from(Some("25")),
                Ok(Duration::from_millis(25))
            );
        }

        #[test]
        fn active_audio_control_registration_only_clears_matching_capture() {
            let _guard = active_audio_control_test_lock().lock().unwrap();
            *active_audio_control_slot().lock().unwrap() = None;

            let (tx1, _rx1) = mpsc::channel();
            let registration1 = ActiveAudioControlRegistration::install(10, tx1);
            assert_eq!(active_audio_control_sender().unwrap().capture_id, 10);

            let (tx2, _rx2) = mpsc::channel();
            let registration2 = ActiveAudioControlRegistration::install(20, tx2);
            assert_eq!(active_audio_control_sender().unwrap().capture_id, 20);

            drop(registration1);
            assert_eq!(active_audio_control_sender().unwrap().capture_id, 20);

            drop(registration2);
            assert!(active_audio_control_sender().is_none());
        }

        #[test]
        fn device_settings_command_prefers_active_capture() {
            let _guard = active_audio_control_test_lock().lock().unwrap();
            *active_audio_control_slot().lock().unwrap() = None;

            let (tx, rx) = mpsc::channel();
            let registration = ActiveAudioControlRegistration::install(42, tx);
            let receiver = std::thread::spawn(move || {
                let signal = rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("active control request");
                match signal {
                    BleCaptureSignal::AudioControl(request) => {
                        assert_eq!(request.bytes, b"DEVICE:SET knob_rotation=system_volume\n");
                        assert_eq!(request.label, "device settings");
                        request.result_tx.send(Ok(())).expect("send result");
                    }
                    BleCaptureSignal::Notification(_) => {
                        panic!("unexpected notification signal")
                    }
                    BleCaptureSignal::Disconnected(reason) => {
                        panic!("unexpected disconnect signal: {reason}")
                    }
                }
            });

            send_device_settings_command(
                "DEVICE:SET knob_rotation=system_volume",
                Duration::from_millis(200),
            )
            .expect("device settings command should use active capture");

            receiver.join().expect("receiver thread");
            drop(registration);
            assert!(active_audio_control_sender().is_none());
        }

        #[test]
        fn device_settings_ble_name_commands_skip_active_capture() {
            assert!(!device_settings_command_allows_active_capture(
                "DEVICE:SET ble_name=Blistener"
            ));
            assert!(!device_settings_command_allows_active_capture(
                "DEVICE:SET name=Blistener"
            ));
            assert!(device_settings_command_allows_active_capture(
                "DEVICE:SET knob_rotation=system_volume"
            ));
            assert!(device_settings_command_allows_active_capture(
                "DEVICE:STATUS"
            ));
        }

        #[test]
        fn recording_stop_retries_with_fresh_gatt_after_active_transient_error() {
            assert_eq!(
                recording_stop_active_transient_fallback(),
                ActiveControlTransientFallback::TryFreshGatt
            );
        }

        #[test]
        fn processing_start_does_not_retry_with_late_fresh_gatt() {
            assert_eq!(
                processing_state_active_transient_fallback(true),
                ActiveControlTransientFallback::ReturnError
            );
            assert_eq!(
                processing_state_active_transient_fallback(false),
                ActiveControlTransientFallback::TryFreshGatt
            );
        }

        #[test]
        fn foreground_probe_success_disables_notify_cccd() {
            assert_eq!(
                NotifyCccdTeardown::for_probe_success(),
                NotifyCccdTeardown::Disable
            );
        }
    }
}

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
pub fn send_recording_control_toggle(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_toggle(timeout)
}

#[cfg(target_os = "windows")]
pub fn send_recording_control_cancel(timeout: Duration) -> Result<(), String> {
    windows_ble::send_recording_control_cancel(timeout)
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
pub fn read_device_settings_status(timeout: Duration) -> Result<DeviceSettingsStatus, String> {
    windows_ble::read_device_settings_status(timeout)
}

#[cfg(target_os = "windows")]
pub fn capture_notification_events(
    timeout: Duration,
    on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    windows_ble::capture_notification_events(timeout, on_event)
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
pub fn transfer_firmware_ota(
    version: &str,
    firmware_sha256: &str,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_firmware_ota(
        version,
        firmware_sha256,
        firmware_bytes,
        manifest_chunk_bytes,
        on_progress,
    )
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub fn transfer_stm32wb_st_ota(
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_stm32wb_st_ota(firmware_bytes, manifest_chunk_bytes, on_progress)
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub fn transfer_stm32wb_st_ota(
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err(
        "STM32WB ST BLE OTA requires a Listener Type build with the companion-dev feature."
            .to_string(),
    )
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub fn transfer_companion_ota_v2(
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_companion_ota_v2(firmware_bytes, manifest_chunk_bytes, on_progress)
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub fn transfer_companion_ota_v2(
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err(
        "Companion OTA v2 requires a Listener Type build with the companion-dev feature."
            .to_string(),
    )
}

#[cfg(target_os = "windows")]
pub fn transfer_listener_ota_v2(
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_listener_ota_v2(firmware_bytes, manifest_chunk_bytes, on_progress)
}

#[cfg(target_os = "windows")]
pub struct FirmwareOtaPreparedTransfer(windows_ble::PreparedFirmwareOtaTransfer);

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub struct Stm32wbStOtaPreparedTransfer(windows_ble::PreparedStm32wbStOtaTransfer);

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub struct Stm32wbStOtaPreparedTransfer;

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub struct CompanionOtaV2PreparedTransfer(windows_ble::PreparedCompanionOtaV2Transfer);

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub struct CompanionOtaV2PreparedTransfer;

#[cfg(target_os = "windows")]
pub struct ListenerOtaV2PreparedTransfer(windows_ble::PreparedListenerOtaV2Transfer);

#[cfg(target_os = "windows")]
impl FirmwareOtaPreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        self.0.snapshot()
    }

    pub fn transfer(
        self,
        version: &str,
        firmware_sha256: &str,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        self.0.transfer(
            version,
            firmware_sha256,
            firmware_bytes,
            manifest_chunk_bytes,
            on_progress,
        )
    }
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
impl Stm32wbStOtaPreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        self.0.snapshot()
    }

    pub fn transfer(
        self,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        self.0
            .transfer(firmware_bytes, manifest_chunk_bytes, on_progress)
    }
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
impl Stm32wbStOtaPreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_stm32wb_st_ota_transfer requires companion-dev")
    }

    pub fn transfer(
        self,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
        _on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err(
            "STM32WB ST BLE OTA requires a Listener Type build with the companion-dev feature."
                .to_string(),
        )
    }
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
impl CompanionOtaV2PreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        self.0.snapshot()
    }

    pub fn transfer(
        self,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        self.0
            .transfer(firmware_bytes, manifest_chunk_bytes, on_progress)
    }
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
impl CompanionOtaV2PreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_companion_ota_v2_transfer requires companion-dev")
    }

    pub fn transfer(
        self,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
        _on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err(
            "Companion OTA v2 requires a Listener Type build with the companion-dev feature."
                .to_string(),
        )
    }
}

#[cfg(target_os = "windows")]
impl ListenerOtaV2PreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        self.0.snapshot()
    }

    pub fn transfer(
        self,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        self.0
            .transfer(firmware_bytes, manifest_chunk_bytes, on_progress)
    }
}

#[cfg(target_os = "windows")]
pub fn prepare_firmware_ota_transfer() -> Result<FirmwareOtaPreparedTransfer, String> {
    windows_ble::prepare_firmware_ota_transfer().map(FirmwareOtaPreparedTransfer)
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub fn prepare_stm32wb_st_ota_transfer() -> Result<Stm32wbStOtaPreparedTransfer, String> {
    windows_ble::prepare_stm32wb_st_ota_transfer().map(Stm32wbStOtaPreparedTransfer)
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub fn prepare_stm32wb_st_ota_transfer() -> Result<Stm32wbStOtaPreparedTransfer, String> {
    Err(
        "STM32WB ST BLE OTA requires a Listener Type build with the companion-dev feature."
            .to_string(),
    )
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub fn prepare_companion_ota_v2_transfer() -> Result<CompanionOtaV2PreparedTransfer, String> {
    windows_ble::prepare_companion_ota_v2_transfer().map(CompanionOtaV2PreparedTransfer)
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub fn prepare_companion_ota_v2_transfer() -> Result<CompanionOtaV2PreparedTransfer, String> {
    Err(
        "Companion OTA v2 requires a Listener Type build with the companion-dev feature."
            .to_string(),
    )
}

#[cfg(target_os = "windows")]
pub fn prepare_listener_ota_v2_transfer() -> Result<ListenerOtaV2PreparedTransfer, String> {
    windows_ble::prepare_listener_ota_v2_transfer().map(ListenerOtaV2PreparedTransfer)
}

#[cfg(target_os = "windows")]
pub fn firmware_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::firmware_ota_device_snapshot()
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub fn stm32wb_st_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::stm32wb_st_ota_device_snapshot()
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub fn stm32wb_st_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some(
            "STM32WB ST BLE OTA requires a Listener Type build with the companion-dev feature."
                .to_string(),
        ),
    }
}

#[cfg(all(target_os = "windows", feature = "companion-dev"))]
pub fn companion_ota_v2_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::companion_ota_v2_device_snapshot()
}

#[cfg(all(target_os = "windows", not(feature = "companion-dev")))]
pub fn companion_ota_v2_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some(
            "Companion OTA v2 requires a Listener Type build with the companion-dev feature."
                .to_string(),
        ),
    }
}

#[cfg(target_os = "windows")]
pub fn listener_ota_v2_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::listener_ota_v2_device_snapshot()
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
pub fn prompt_listener_pairing(expected_name: Option<&str>) -> BleDevicePairingPromptResult {
    windows_ble::prompt_listener_pairing(expected_name)
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
pub fn send_recording_control_stop(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording stop is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_recovery(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recovery is only supported on Windows".to_string())
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
pub fn read_device_settings_status(_timeout: Duration) -> Result<DeviceSettingsStatus, String> {
    Err("Listener device settings refresh is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn capture_notification_events(
    _timeout: Duration,
    _on_event: &mut BleNotificationHandler<'_>,
) -> Result<(), String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
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
pub fn transfer_firmware_ota(
    _version: &str,
    _firmware_sha256: &str,
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("Firmware OTA over Listener BLE is only supported on Windows".to_string())
}

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub fn transfer_stm32wb_st_ota(
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("STM32WB ST BLE OTA is only supported on Windows".to_string())
}

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub fn transfer_companion_ota_v2(
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("Companion OTA v2 over BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn transfer_listener_ota_v2(
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("Listener OTA v2 over BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub struct FirmwareOtaPreparedTransfer;

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub struct Stm32wbStOtaPreparedTransfer;

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub struct CompanionOtaV2PreparedTransfer;

#[cfg(not(target_os = "windows"))]
pub struct ListenerOtaV2PreparedTransfer;

#[cfg(not(target_os = "windows"))]
impl FirmwareOtaPreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_firmware_ota_transfer is unsupported on this platform")
    }

    pub fn transfer(
        self,
        _version: &str,
        _firmware_sha256: &str,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err("Firmware OTA over Listener BLE is only supported on Windows".to_string())
    }
}

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
impl Stm32wbStOtaPreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_stm32wb_st_ota_transfer is unsupported on this platform")
    }

    pub fn transfer(
        self,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
        _on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err("STM32WB ST BLE OTA is only supported on Windows".to_string())
    }
}

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
impl CompanionOtaV2PreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_companion_ota_v2_transfer is unsupported on this platform")
    }

    pub fn transfer(
        self,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
        _on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err("Companion OTA v2 over BLE is only supported on Windows".to_string())
    }
}

#[cfg(not(target_os = "windows"))]
impl ListenerOtaV2PreparedTransfer {
    pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
        unreachable!("prepare_listener_ota_v2_transfer is unsupported on this platform")
    }

    pub fn transfer(
        self,
        _firmware_bytes: &[u8],
        _manifest_chunk_bytes: usize,
        _on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err("Listener OTA v2 over BLE is only supported on Windows".to_string())
    }
}

#[cfg(not(target_os = "windows"))]
pub fn prepare_firmware_ota_transfer() -> Result<FirmwareOtaPreparedTransfer, String> {
    Err("Firmware OTA over Listener BLE is only supported on Windows".to_string())
}

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub fn prepare_stm32wb_st_ota_transfer() -> Result<Stm32wbStOtaPreparedTransfer, String> {
    Err("STM32WB ST BLE OTA is only supported on Windows".to_string())
}

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub fn prepare_companion_ota_v2_transfer() -> Result<CompanionOtaV2PreparedTransfer, String> {
    Err("Companion OTA v2 over BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn prepare_listener_ota_v2_transfer() -> Result<ListenerOtaV2PreparedTransfer, String> {
    Err("Listener OTA v2 over BLE is only supported on Windows".to_string())
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

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub fn stm32wb_st_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("STM32WB ST BLE OTA is only supported on Windows".to_string()),
    }
}

#[cfg(all(not(target_os = "windows"), feature = "companion-dev"))]
pub fn companion_ota_v2_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("Companion OTA v2 over BLE is only supported on Windows".to_string()),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn listener_ota_v2_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some("Listener OTA v2 over BLE is only supported on Windows".to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded_audio::{
        build_audio_data_notification, build_session_cancel_notification,
        build_session_error_notification, build_session_start_notification,
        build_session_stop_notification, SessionCollector, SessionErrorCode,
    };

    #[cfg(target_os = "windows")]
    static DEVICE_SETTINGS_HARDWARE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(target_os = "windows")]
    fn usb_serial_port(
        port_name: &str,
        vid: u16,
        pid: u16,
        manufacturer: &str,
        product: &str,
    ) -> serialport::SerialPortInfo {
        serialport::SerialPortInfo {
            port_name: port_name.to_string(),
            port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid,
                pid,
                serial_number: Some(format!("{port_name}-serial")),
                manufacturer: Some(manufacturer.to_string()),
                product: Some(product.to_string()),
            }),
        }
    }

    #[test]
    fn terminal_detection_only_matches_stop_cancel_error() {
        assert!(!is_terminal_notification(
            &build_session_start_notification(1)
        ));
        assert!(!is_terminal_notification(
            &build_audio_data_notification(1, 0, &[1, 2]).expect("audio packet")
        ));
        assert!(is_terminal_notification(&build_session_stop_notification(
            1, 1
        )));
        assert!(is_terminal_notification(
            &build_session_cancel_notification(1, 1)
        ));
        assert!(is_terminal_notification(&build_session_error_notification(
            1,
            1,
            SessionErrorCode::LinkLost,
        )));
    }

    #[test]
    fn invalid_notification_is_not_terminal() {
        assert!(!is_terminal_notification(b"not-vka1"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn active_capture_link_recovery_only_during_open_audio_session() {
        let mut collector = SessionCollector::default();
        assert!(!windows_ble::collector_has_active_recoverable_session(
            &collector
        ));

        collector
            .handle_notification(&build_session_start_notification(7))
            .expect("start notification");
        assert!(windows_ble::collector_has_active_recoverable_session(
            &collector
        ));

        collector
            .handle_notification(
                &build_audio_data_notification(7, 0, &[1, 2]).expect("audio notification"),
            )
            .expect("audio notification");
        assert!(windows_ble::collector_has_active_recoverable_session(
            &collector
        ));

        collector
            .handle_notification(&build_session_stop_notification(7, 1))
            .expect("stop notification");
        assert!(!windows_ble::collector_has_active_recoverable_session(
            &collector
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn active_capture_link_recovery_timeout_stays_bounded() {
        let (heartbeat_interval, recovery_timeout) =
            windows_ble::active_capture_recovery_timing_for_test();
        assert_eq!(heartbeat_interval, Duration::from_secs(8));
        assert_eq!(recovery_timeout, Duration::from_secs(5));
        assert!(recovery_timeout < heartbeat_interval);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn type_heartbeat_runs_only_for_background_capture() {
        let (one_shot, background) =
            windows_ble::type_heartbeat_terminal_behavior_matrix_for_test();
        assert!(!one_shot);
        assert!(background);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn notify_target_open_retry_classifies_windows_gatt_transients() {
        assert!(windows_ble::is_transient_notify_target_open_error(
            "BLE characteristic discovery returned status=GattCommunicationStatus(3)"
        ));
        assert!(windows_ble::is_transient_notify_target_open_error(
            "BLE service open wait failed: HRESULT(0x800706BA)"
        ));
        assert!(!windows_ble::is_transient_notify_target_open_error(
            "notify characteristic not found"
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn pnp_device_instance_normalizer_extracts_bthle_devnode() {
        assert_eq!(
            windows_ble::normalize_pnp_device_instance_id(
                r#"\\?\BTHLE#DEV_D41A50FBF35E#9&B465B9E&0&D41A50FBF35E#{0000180a-0000-1000-8000-00805f9b34fb}"#
            ),
            Some(r#"BTHLE\DEV_D41A50FBF35E\9&B465B9E&0&D41A50FBF35E"#.to_string())
        );
        assert_eq!(
            windows_ble::normalize_pnp_device_instance_id(
                r#"BTHLE\DEV_D41A50FBF35E\9&B465B9E&0&D41A50FBF35E"#
            ),
            Some(r#"BTHLE\DEV_D41A50FBF35E\9&B465B9E&0&D41A50FBF35E"#.to_string())
        );
        assert_eq!(
            windows_ble::normalize_pnp_device_instance_id(r#"USB\VID_0000&PID_0000"#),
            None
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn pnp_device_instance_normalizer_keeps_btledevice_and_hid_children() {
        assert_eq!(
            windows_ble::normalize_pnp_device_instance_id(
                r#"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#
            ),
            Some(
                r#"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#
                    .to_string()
            )
        );
        assert_eq!(
            windows_ble::normalize_pnp_device_instance_id(
                r#"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D&COL01\A&220D8BA7&0&0000"#
            ),
            Some(
                r#"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D&COL01\A&220D8BA7&0&0000"#
                    .to_string()
            )
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn pnp_listener_service_signature_finds_arbitrary_renamed_device_tree() {
        let service_id = r#"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3091A}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#;
        assert!(windows_ble::pnp_instance_has_listener_service_signature(
            service_id
        ));
        assert_eq!(
            windows_ble::parse_bluetooth_address_from_device_id(service_id),
            Some(0xFD2F_988D_B40D)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn pnp_cleanup_match_uses_service_signature_without_name_fallback() {
        let service_entry = windows_ble::ListenerPnpEntry {
            name: "Bluetooth LE GATT Service".to_string(),
            instance_id:
                r#"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3092A}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#
                    .to_string(),
            address: Some(0xFD2F_988D_B40D),
            has_listener_service_signature: true,
            is_ble_device_root: false,
        };
        assert!(windows_ble::listener_pnp_entry_matches_cleanup(
            &service_entry,
            false,
            false
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn pnp_ble_device_root_detection_is_strict() {
        assert!(windows_ble::pnp_instance_is_ble_device_root(
            r#"BTHLE\DEV_FD2F988DB40D\8&25948282&0&FD2F988DB40D"#
        ));
        assert!(!windows_ble::pnp_instance_is_ble_device_root(
            r#"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_FD2F988DB40D\9&2E60A20C&0&004B"#
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn bthport_device_name_decoder_accepts_ascii_and_utf16() {
        assert_eq!(
            windows_ble::decode_bthport_device_name(b"Blistener\0\0"),
            "Blistener"
        );
        assert_eq!(
            windows_ble::decode_bthport_device_name(&[
                b'l', 0, b'i', 0, b's', 0, b't', 0, b'e', 0, b'n', 0, b'e', 0, b'r', 0, b'B', 0, 0,
                0,
            ]),
            "listenerB"
        );
    }

    #[test]
    fn ble_failure_taxonomy_covers_customer_recovery_cases() {
        let cases = [
            (
                "Embedded audio BLE service not found; ensure device is paired and online",
                BleFailureKind::DeviceMissing,
                false,
            ),
            (
                "Listener BLE device asleep; press KEY4 wake key before retry",
                BleFailureKind::DeviceAsleep,
                false,
            ),
            (
                "No paired BLE device found in Windows Bluetooth pairing store",
                BleFailureKind::MissingPairing,
                false,
            ),
            (
                "BLE idle disconnect reason=546 produced transport_not_ready before reconnect",
                BleFailureKind::LowPowerIdleDisconnect,
                true,
            ),
            (
                "BLE device connection status changed to Disconnected; transport_not_ready",
                BleFailureKind::PairedButDisconnected,
                true,
            ),
            (
                "BLE validation injected disconnect through notify wait; transport_not_ready",
                BleFailureKind::PairedButDisconnected,
                true,
            ),
            (
                "stale cached GATT path after BLE reason=546 returned transport_not_ready",
                BleFailureKind::StaleGattService,
                true,
            ),
            (
                "BLE Uncached service discovery returned status=Unreachable after timeout",
                BleFailureKind::PairedButDisconnected,
                true,
            ),
            (
                "Unknown GATT service from stale cached service table",
                BleFailureKind::StaleGattService,
                true,
            ),
            (
                "BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected",
                BleFailureKind::StaleGattService,
                true,
            ),
            (
                "BLE CCCD notify write returned status=ProtocolError protocol_error=3",
                BleFailureKind::CccdProtocolError,
                true,
            ),
            (
                "DIS firmware revision missing from preflight snapshot",
                BleFailureKind::MissingDisFirmwareRevision,
                false,
            ),
            (
                "background listener already active; foreground probe skipped",
                BleFailureKind::BackgroundListenerContention,
                true,
            ),
            (
                "OTA reboot window: version confirm failed after OTA",
                BleFailureKind::OtaRebootWindow,
                true,
            ),
            (
                "Windows Bluetooth service reset needed after adapter radio error",
                BleFailureKind::WindowsBluetoothServiceResetNeeded,
                false,
            ),
        ];

        for (message, expected_kind, expected_auto) in cases {
            let classification = classify_ble_failure(message);
            assert_eq!(classification.kind, expected_kind, "{message}");
            assert_eq!(
                classification.automatic_recovery, expected_auto,
                "{message}"
            );
            assert!(!classification.user_action.is_empty());
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn ota_finish_reboot_handoff_accepts_windows_ble_disconnect_errors() {
        assert!(super::windows_ble::is_ota_finish_reboot_handoff_error(
            "BLE OTA control finish write async error: Some(HRESULT(0x800704C7))"
        ));
        assert!(super::windows_ble::is_ota_finish_reboot_handoff_error(
            "BLE OTA control finish write async error: Some(HRESULT(0x800706BA))"
        ));
        assert!(super::windows_ble::is_ota_finish_reboot_handoff_error(
            "BLE device connection status changed to Disconnected; transport_not_ready"
        ));
        assert!(!super::windows_ble::is_ota_finish_reboot_handoff_error(
            "BLE OTA data write async error: Some(HRESULT(0x80070057))"
        ));
        assert!(!super::windows_ble::is_ota_finish_reboot_handoff_error(
            "BLE OTA control begin write returned status=GattCommunicationStatus(1)"
        ));
    }

    #[cfg(all(target_os = "windows", feature = "companion-dev"))]
    #[test]
    fn stm32wb_finish_reboot_handoff_accepts_windows_timeout_then_disconnect() {
        assert!(
            super::windows_ble::is_stm32wb_st_ota_finish_reboot_handoff_error(
                "BLE STM32WB ST OTA finish write timed out after 8000 ms; fallback GattWriteOption(1) for STM32WB ST OTA finish also failed: BLE STM32WB ST OTA finish write async error: Some(HRESULT(0x80000013))"
            )
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_bluetooth_address_from_service_instance_id() {
        assert_eq!(
            super::windows_ble::parse_bluetooth_address_from_device_id(
                r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3091A}_DCB4D91112CE"
            ),
            Some(0xDCB4_D911_12CE)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_bluetooth_address_from_device_instance_id() {
        assert_eq!(
            super::windows_ble::parse_bluetooth_address_from_device_id(
                r"BTHLE\DEV_DCB4D91112CE\7&29C9821A&0&0000"
            ),
            Some(0xDCB4_D911_12CE)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_bluetooth_address_from_bluetooth_le_selector_id() {
        assert_eq!(
            super::windows_ble::parse_bluetooth_address_from_device_id(
                r"BluetoothLE#BluetoothLE00:11:22:33:44:55-D4:1A:50:FB:F3:5E"
            ),
            Some(0xD41A_50FB_F35E)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_bluetooth_address_from_vid_pid_service_instance_id() {
        assert_eq!(
            super::windows_ble::parse_bluetooth_address_from_device_id(
                r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3092A}_DEV_VID&0216C0_PID&05DF_REV&0001_14C19F48FE72\A&B5FDFC&D&0009"
            ),
            Some(0x14C1_9F48_FE72)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_configured_bluetooth_address_hex_forms() {
        assert_eq!(
            super::windows_ble::parse_bluetooth_address_hex("D41A50FBF35E"),
            Some(0xD41A_50FB_F35E)
        );
        assert_eq!(
            super::windows_ble::parse_bluetooth_address_hex("D4:1A:50:FB:F3:5E"),
            Some(0xD41A_50FB_F35E)
        );
        assert_eq!(
            super::windows_ble::parse_bluetooth_address_hex("D4-1A-50-FB-F3-5E"),
            Some(0xD41A_50FB_F35E)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn device_settings_serial_candidates_reject_single_stlink_port() {
        let ports = vec![usb_serial_port(
            "COM13",
            0x0483,
            0x374b,
            "STMicroelectronics",
            "STLink Virtual COM Port",
        )];

        assert!(super::windows_ble::listener_usb_serial_candidates(&ports).is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn device_settings_serial_candidates_keep_listener_esp32_port() {
        let ports = vec![
            usb_serial_port(
                "COM13",
                0x0483,
                0x374b,
                "STMicroelectronics",
                "STLink Virtual COM Port",
            ),
            usb_serial_port("COM11", 0x303a, 0x1001, "Microsoft", "USB Serial Device"),
        ];

        let candidates = super::windows_ble::listener_usb_serial_candidates(&ports);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].port_name, "COM11");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn ota_chunk_selection_uses_manifest_and_transport_limits() {
        assert_eq!(
            super::windows_ble::ota_transfer_chunk_bytes(514, 500),
            Ok(500)
        );
        assert_eq!(
            super::windows_ble::ota_transfer_chunk_bytes(499, 500),
            Ok(499)
        );
        assert!(super::windows_ble::ota_transfer_chunk_bytes(0, 500).is_err());
        assert!(super::windows_ble::ota_transfer_chunk_bytes(514, 499).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a paired or USB-connected Listener device"]
    fn device_settings_command_hardware_smoke() {
        let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
            .lock()
            .expect("device settings hardware test mutex poisoned");
        let command = std::env::var("LISTENER_DEVICE_SETTINGS_COMMAND")
            .unwrap_or_else(|_| "DEVICE:SET knob_rotation=system_volume".to_string());
        super::windows_ble::send_device_settings_command(&command, Duration::from_secs(4))
            .expect("device settings command should be acknowledged by firmware");
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a USB-connected or paired Listener device"]
    fn ble_name_recovery_hardware_smoke() {
        let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
            .lock()
            .expect("device settings hardware test mutex poisoned");
        super::windows_ble::send_recording_control_recovery(Duration::from_secs(4))
            .expect("BLE recovery control should be sent to firmware");
        std::thread::sleep(Duration::from_secs(2));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "renames the paired Listener hardware and clears Windows Bluetooth cache"]
    fn ble_name_rename_hardware_roundtrip() {
        let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
            .lock()
            .expect("device settings hardware test mutex poisoned");
        let old_name = std::env::var("LISTENER_BLE_NAME_RENAME_OLD")
            .expect("set LISTENER_BLE_NAME_RENAME_OLD to the currently advertised BLE name");
        let target_name = std::env::var("LISTENER_BLE_NAME_RENAME_TARGET")
            .expect("set LISTENER_BLE_NAME_RENAME_TARGET to the desired BLE name");

        super::windows_ble::set_configured_bluetooth_target_name(&old_name);
        super::windows_ble::send_device_settings_command(
            &format!("DEVICE:SET ble_name={target_name}"),
            Duration::from_secs(4),
        )
        .expect("BLE name write should be acknowledged by firmware");
        super::windows_ble::send_recording_control_recovery(Duration::from_secs(4))
            .expect("BLE recovery control should restart advertising");
        std::thread::sleep(Duration::from_secs(2));

        let recovery_names = vec![old_name.clone(), target_name.clone()];
        let unpair = super::windows_ble::unpair_listener_devices_for_names(&recovery_names);
        assert!(
            !unpair.needs_user_action || unpair.failed_devices == 0,
            "automatic stale pairing cleanup should not fail: {unpair:?}"
        );
        std::thread::sleep(Duration::from_millis(1200));

        let pairing = super::windows_ble::prompt_listener_pairing(Some(&target_name));
        assert!(
            !pairing.open_bluetooth_settings,
            "Windows pairing should be completed or already clean: {pairing:?}"
        );

        super::windows_ble::set_configured_bluetooth_target_name(&target_name);
        let status = super::windows_ble::read_embedded_audio_status(Duration::from_secs(20))
            .expect("renamed Listener BLE audio status should be reachable");
        assert!(
            status.connected,
            "renamed Listener BLE status should be connected"
        );
        let settings = super::windows_ble::read_device_settings_status(Duration::from_secs(4))
            .expect("renamed Listener device settings should be readable");
        assert_eq!(settings.ble_name, target_name);
        if settings.ble_name_pending_restart {
            eprintln!(
                "firmware still reports ble_name_pending_restart=1 after advertising {target_name}; Type treats exact-name BLE reachability as applied"
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_device_settings_status_line_preserves_ble_name_order() {
        let status = super::windows_ble::parse_device_settings_status_line(
            "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=external low_power_idle_ms=60000 knob_rotation=screen_brightness ble_name=\"Blistener\" ble_name_pending=0 external_power_present=1 usb_power_present=1 charging=0 charge_full=1"
        )
        .expect("parse Blistener device settings");

        assert_eq!(status.ble_name, "Blistener");
        assert_ne!(status.ble_name, "listenerB");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_device_settings_ms_jitter_without_rounding_up_full_minute() {
        let status = super::windows_ble::parse_device_settings_status_line(
            "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=battery low_power_idle_ms=600001 plugged_low_power_idle_ms=600001 battery_low_power_idle_ms=600001 plugged_low_power_enabled=1 auto_shutdown_ms=600001 plugged_auto_shutdown_ms=0 battery_auto_shutdown_ms=600001 knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=0 external_power_present=0 usb_power_present=0 charging=0 charge_full=0"
        )
        .expect("parse device settings with millisecond jitter");

        assert_eq!(status.low_power_idle_minutes, 10);
        assert_eq!(status.plugged_low_power_idle_minutes, 10);
        assert_eq!(status.battery_low_power_idle_minutes, 10);
        assert_eq!(status.plugged_auto_shutdown_minutes, 0);
        assert_eq!(status.battery_auto_shutdown_minutes, 10);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_device_settings_status_line() {
        let status = super::windows_ble::parse_device_settings_status_line(
            "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK plugged_brightness=80 battery_brightness=50 active_power=external active_brightness=80 led_status=70 led_key=65 led_ec11=60 led_edge=55 low_power_idle_ms=60000 plugged_low_power_idle_ms=120000 battery_low_power_idle_minutes=3 plugged_low_power_enabled=1 low_power_idle_mode=power_mode auto_shutdown_ms=1800000 plugged_auto_shutdown_ms=0 battery_auto_shutdown_minutes=45 auto_shutdown_mode=power_mode knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=1 ble_name_apply=restart_ble_or_reboot loaded_from_nvs=1 external_power_present=1 usb_power_present=1 charging=0 charge_full=1 valid_ranges=brightness_0_100,led_zone_brightness_0_100,low_power_idle_ms_0_86400000"
        )
        .expect("parse device settings");
        assert_eq!(status.status_led_brightness_percent, 70);
        assert_eq!(status.key_led_brightness_percent, 65);
        assert_eq!(status.knob_led_brightness_percent, 60);
        assert_eq!(status.edge_led_brightness_percent, 55);
        assert!(status.led_zone_brightness_supported);
        assert_eq!(status.low_power_idle_minutes, 2);
        assert_eq!(status.plugged_low_power_idle_minutes, 2);
        assert_eq!(status.battery_low_power_idle_minutes, 3);
        assert!(status.plugged_low_power_enabled);
        assert_eq!(status.plugged_auto_shutdown_minutes, 0);
        assert_eq!(status.battery_auto_shutdown_minutes, 45);
        assert_eq!(status.knob_rotation_action, "screen_brightness");
        assert_eq!(status.ble_name, "listener-dev");
        assert!(status.ble_name_pending_restart);
        assert!(status.external_power_present);
        assert!(status.usb_power_present);
        assert!(!status.charging);
        assert!(status.charge_full);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_legacy_device_settings_status_without_led_zone_brightness() {
        let status = super::windows_ble::parse_device_settings_status_line(
            "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=external low_power_idle_ms=60000 knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=0 external_power_present=1 usb_power_present=1 charging=0 charge_full=1"
        )
        .expect("parse legacy device settings");
        assert_eq!(status.status_led_brightness_percent, 100);
        assert_eq!(status.key_led_brightness_percent, 100);
        assert_eq!(status.knob_led_brightness_percent, 100);
        assert_eq!(status.edge_led_brightness_percent, 100);
        assert!(!status.led_zone_brightness_supported);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a USB-connected Listener device"]
    fn device_settings_status_refresh_hardware_smoke() {
        let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
            .lock()
            .expect("device settings hardware test mutex poisoned");
        let status = super::windows_ble::read_device_settings_status(Duration::from_secs(4))
            .expect("device settings status should be read from firmware");
        assert!(!status.ble_name.is_empty());
        assert!(status.status_led_brightness_percent <= 100);
        assert!(status.key_led_brightness_percent <= 100);
        assert!(status.knob_led_brightness_percent <= 100);
        assert!(status.edge_led_brightness_percent <= 100);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_zero_low_power_idle_as_disabled() {
        let status = super::windows_ble::parse_device_settings_status_line(
            "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=external low_power_idle_ms=0 plugged_low_power_idle_ms=0 battery_low_power_idle_ms=0 plugged_low_power_enabled=0 knob_rotation=system_volume ble_name=\"listener-dev\" ble_name_pending=0 external_power_present=1 usb_power_present=1 charging=0 charge_full=0"
        )
        .expect("parse zero low-power device settings");

        assert_eq!(status.low_power_idle_minutes, 0);
        assert_eq!(status.plugged_low_power_idle_minutes, 0);
        assert_eq!(status.battery_low_power_idle_minutes, 0);
        assert!(!status.plugged_low_power_enabled);
    }

    #[test]
    fn crc32_matches_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(format_crc32(0xcbf4_3926), "0xcbf43926");
    }
}
