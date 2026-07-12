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
pub const LISTENER_OTA_V1_SERVICE_UUID_TEXT: &str = denzic_ota_core::GATT_SERVICE_UUID;
pub const LISTENER_OTA_V1_CONTROL_UUID_TEXT: &str = denzic_ota_core::GATT_CONTROL_UUID;
pub const LISTENER_OTA_V1_DATA_UUID_TEXT: &str = denzic_ota_core::GATT_DATA_UUID;
pub const LISTENER_OTA_V1_STATUS_UUID_TEXT: &str = denzic_ota_core::GATT_STATUS_UUID;
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListenerRecoveryPairingAdvertisementProbe {
    pub visible: bool,
    pub has_random_identity: bool,
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
    use super::DeviceSettingsCommandTransport;
    use std::cell::RefCell;
    use std::fmt;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Serialize};
    use serialport::{SerialPortInfo, SerialPortType};
    use windows::core::{IInspectable, Interface, GUID, HSTRING, PCWSTR};
    use windows::Devices::Bluetooth::Advertisement::{
        BluetoothLEAdvertisement, BluetoothLEAdvertisementReceivedEventArgs,
        BluetoothLEAdvertisementWatcher, BluetoothLEScanningMode,
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
        DeviceInformationKind, DeviceInformationPairing, DevicePairingKinds,
        DevicePairingRequestedEventArgs, DevicePairingResultStatus, DeviceUnpairingResultStatus,
    };
    use windows::Foundation::{
        AsyncStatus, EventRegistrationToken, IAsyncOperation, IPropertyValue, TypedEventHandler,
    };
    use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Locate_DevNodeW, CM_Query_And_Remove_SubTreeW, CM_LOCATE_DEVNODE_NORMAL,
        CM_LOCATE_DEVNODE_PHANTOM, CM_REMOVE_NO_RESTART, CM_REMOVE_UI_NOT_OK, CONFIGRET,
        CR_ACCESS_DENIED, CR_NO_SUCH_DEVINST, CR_NO_SUCH_DEVNODE, CR_QUERY_VETOED,
        CR_REMOVE_VETOED, CR_SUCCESS, PNP_VETO_TYPE,
    };
    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const SERVICE_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091a);
    const NOTIFY_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091b);
    const AUDIO_CONTROL_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091e);
    const OTA_SERVICE_UUID: GUID = GUID::from_u128(denzic_ota_core::GATT_SERVICE_UUID_U128);
    const LISTENER_OTA_V1_SERVICE_UUID: GUID = OTA_SERVICE_UUID;
    const LISTENER_OTA_V1_CONTROL_UUID: GUID =
        GUID::from_u128(denzic_ota_core::GATT_CONTROL_UUID_U128);
    const LISTENER_OTA_V1_DATA_UUID: GUID = GUID::from_u128(denzic_ota_core::GATT_DATA_UUID_U128);
    const LISTENER_OTA_V1_STATUS_UUID: GUID =
        GUID::from_u128(denzic_ota_core::GATT_STATUS_UUID_U128);
    const OTA_READINESS_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091c);
    const OTA_CAPABILITIES_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091d);
    const WINDOWS_BLE_AEP_SELECTOR: &str =
        "(System.Devices.Aep.ProtocolId:=\"{bb7bb05e-5972-42b5-94fc-76eaa7084d49}\")";
    const WINDOWS_BLE_AEP_CONNECTABLE_SELECTOR: &str =
        "(System.Devices.Aep.ProtocolId:=\"{bb7bb05e-5972-42b5-94fc-76eaa7084d49}\") AND (System.Devices.Aep.Bluetooth.Le.IsConnectable:=System.StructuredQueryType.Boolean#True)";
    const WINDOWS_AEP_DEVICE_ADDRESS_PROPERTY: &str = "System.Devices.Aep.DeviceAddress";
    const WINDOWS_AEP_IS_PAIRED_PROPERTY: &str = "System.Devices.Aep.IsPaired";
    const WINDOWS_AEP_IS_CONNECTED_PROPERTY: &str = "System.Devices.Aep.IsConnected";
    const WINDOWS_AEP_IS_PRESENT_PROPERTY: &str = "System.Devices.Aep.IsPresent";
    const WINDOWS_AEP_BLE_IS_CONNECTABLE_PROPERTY: &str =
        "System.Devices.Aep.Bluetooth.Le.IsConnectable";
    const WINDOWS_ITEM_NAME_DISPLAY_PROPERTY: &str = "System.ItemNameDisplay";

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
    const TYPE_READY_RECOVERY_PAIRING_ADV_PROBE_TIMEOUT: Duration = Duration::from_millis(900);
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
    const TYPE_READY_COMMAND_ENV: &str = "LISTENER_TYPE_EMBEDDED_BLE_READY_COMMAND";
    const AUDIO_ADVERTISEMENT_SCAN_TIMEOUT: Duration = Duration::from_secs(12);
    const BLE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
    const BLE_PAIRING_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(45);
    const BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6);
    const BLE_PAIRING_PROMPT_TIMEOUT: Duration = Duration::from_secs(45);
    const BLE_PAIRING_FAST_FAILURE_AEP_FALLBACK_THRESHOLD: Duration = Duration::from_secs(5);
    const BLE_PAIRING_PROMPT_SUPPRESS_WINDOW: Duration = Duration::from_secs(60 * 60);
    const BLE_PAIRING_MAINTENANCE_MAX_WINDOW: Duration = Duration::from_secs(3 * 60);
    const BLE_PAIRING_MAINTENANCE_MUTEX_NAME: PCWSTR =
        windows::core::w!("Local\\Denzic.Listener.Type.PairingMaintenance");
    const BLE_OTA_OPERATION_MUTEX_NAME: PCWSTR =
        windows::core::w!("Local\\Denzic.Listener.Type.BleOtaOperation");
    const BACKGROUND_LISTENER_DEFERRED_FOR_OTA: &str =
        "embedded_ble_background_listener_deferred_for_ota";
    const BLE_RECENT_PAIRING_FAST_GATT_WINDOW: Duration = Duration::from_secs(45);
    const BLE_RECENT_PAIRING_CACHED_PROBE_WINDOW: Duration = Duration::from_millis(2250);
    const RECENT_PAIRING_CACHED_GATT_PROBE_ERROR: &str =
        "recent pairing cached GATT readiness probe";
    const BLE_ADAPTER_RESTART_SETTLE: Duration = Duration::from_millis(2500);
    const BLE_PAIRING_IN_PROGRESS_SETTLE: Duration = Duration::from_millis(2200);
    const WINDOWS_CREATE_NO_WINDOW: u32 = 0x08000000;
    const DEVICE_SETTINGS_SERIAL_BAUD_RATE: u32 = 115_200;
    const DEVICE_SETTINGS_SERIAL_READ_CHUNK_BYTES: usize = 256;

    // A background capture owns the cross-process GATT gate.  Keep its caller's
    // cancellation flag available to the blocking WinRT wait helpers so a rename,
    // OTA handoff, or recovery cleanup does not leave an obsolete target holding
    // the gate until its long Windows timeout expires.
    thread_local! {
        static ACTIVE_NOTIFY_CAPTURE_CANCEL: RefCell<Option<Arc<AtomicBool>>> = RefCell::new(None);
    }

    static NOTIFY_CAPTURE_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);

    struct NotifyCaptureCancelScope {
        previous: Option<Arc<AtomicBool>>,
    }

    impl NotifyCaptureCancelScope {
        fn install(cancel_requested: &Arc<AtomicBool>) -> Self {
            let previous = ACTIVE_NOTIFY_CAPTURE_CANCEL.with(|slot| {
                std::mem::replace(&mut *slot.borrow_mut(), Some(Arc::clone(cancel_requested)))
            });
            Self { previous }
        }
    }

    impl Drop for NotifyCaptureCancelScope {
        fn drop(&mut self) {
            ACTIVE_NOTIFY_CAPTURE_CANCEL.with(|slot| {
                *slot.borrow_mut() = self.previous.take();
            });
        }
    }

    fn notify_capture_cancel_requested() -> bool {
        ACTIVE_NOTIFY_CAPTURE_CANCEL.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|cancel| cancel.load(Ordering::SeqCst))
        })
    }

    fn notify_capture_cancelled_error(label: &str) -> String {
        format!("BLE {label} cancelled by background listener recovery")
    }

    pub(super) fn notify_capture_session_active() -> bool {
        NOTIFY_CAPTURE_SESSION_ACTIVE.load(Ordering::SeqCst)
    }
    const DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION: Duration = Duration::from_millis(1800);
    const DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION: Duration = Duration::from_millis(180);
    const DEFAULT_BLUETOOTH_TARGET_NAME: &str = "listener";
    const BTHPORT_DEVICE_CACHE_REGISTRY_PATH: &str =
        r"SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices";
    const ATT_WRITE_HEADER_BYTES: usize = 3;
    const ATT_DEFAULT_PAYLOAD_BYTES: usize = 20;
    const SERVICE_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3091a";
    const OTA_SERVICE_UUID_TEXT: &str = denzic_ota_core::GATT_SERVICE_UUID;
    const LISTENER_SERVICE_UUID_TEXTS: [&str; 3] = [
        SERVICE_UUID_TEXT,
        OTA_SERVICE_UUID_TEXT,
        crate::embedded_ble::DIAGNOSTIC_SERVICE_UUID_TEXT,
    ];
    const LISTENER_OTA_V1_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(150);
    const LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES: usize = 500;
    const LISTENER_OTA_V1_DEFAULT_WINDOW_CHUNKS: usize = 100;
    const LISTENER_OTA_V1_INACTIVE_LINK_WINDOW_CHUNKS: usize = 4;
    const LISTENER_OTA_V1_WINDOW_ENV: &str = "LISTENER_OTA_V1_WINDOW_CHUNKS";
    const LISTENER_OTA_V1_STATUS_READ_TIMEOUT: Duration = Duration::from_secs(3);
    const LISTENER_OTA_V1_RECONNECT_SETTLE: Duration = Duration::from_millis(500);
    const RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT: Duration = Duration::from_millis(700);
    const DIS_SERVICE_UUID_TEXT: &str = "0000180a-0000-1000-8000-00805f9b34fb";
    const BLE_TARGET_ADDRESS_CACHE_WINDOW: Duration = Duration::from_secs(60 * 60);
    const BLE_RENAME_ADDRESS_GRACE_WINDOW: Duration = Duration::from_secs(10 * 60);
    const BLE_DEVICE_STATE_FILE: &str = "ble_device_state.json";
    const STARTUP_NOTIFY_FAST_PATH_TIMEOUT: Duration = Duration::from_millis(2500);
    const STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT: Duration = Duration::from_millis(1200);
    const STARTUP_NOTIFY_FAST_PATH_GATT_TIMEOUT: Duration = Duration::from_millis(1800);
    static RUNTIME_BLUETOOTH_TARGET_NAME: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    static RUNTIME_BLUETOOTH_TARGET_ADDRESS: OnceLock<
        Mutex<Option<RuntimeBluetoothTargetAddress>>,
    > = OnceLock::new();
    static LAST_PAIRING_PROMPT: OnceLock<Mutex<Option<PairingPromptThrottleState>>> =
        OnceLock::new();
    static LISTENER_PAIRING_MAINTENANCE_TOKEN: AtomicUsize = AtomicUsize::new(1);

    enum BleCaptureSignal {
        Notification(Vec<u8>),
        Disconnected(String),
    }

    struct AudioControlRequest {
        bytes: Vec<u8>,
        label: String,
        timeout: Duration,
        queued_at: Instant,
        result_tx: mpsc::Sender<Result<(), String>>,
    }

    #[derive(Clone)]
    struct ActiveAudioControlSender {
        capture_id: u64,
        tx: mpsc::Sender<AudioControlRequest>,
    }

    #[derive(Clone)]
    struct RuntimeBluetoothTargetAddress {
        address: u64,
        target_name: String,
        learned_at: Instant,
        valid_for: Duration,
    }

    #[derive(Debug, Default, Deserialize, Serialize)]
    struct PersistedBleDeviceState {
        #[serde(default)]
        last_successful_address: Option<String>,
        #[serde(default)]
        target_name: Option<String>,
        #[serde(default)]
        updated_at: Option<String>,
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
        fn install(capture_id: u64, tx: mpsc::Sender<AudioControlRequest>) -> Self {
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

    fn notify_cccd_prereset_required(terminal_behavior: CaptureTerminalBehavior) -> bool {
        terminal_behavior == CaptureTerminalBehavior::StopCapture
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
        let target_name = extra_names
            .iter()
            .find_map(|name| {
                let trimmed = name.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .map(ToString::to_string)
            .unwrap_or_else(|| effective_bluetooth_target_name(None));
        let Some(_maintenance) =
            try_begin_listener_pairing_maintenance("unpair", &target_name, Instant::now())
        else {
            return crate::embedded_ble::BleDeviceUnpairResult {
                status: crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean,
                attempted: false,
                matched_devices: 0,
                unpaired_devices: 0,
                already_unpaired_devices: 0,
                failed_devices: 0,
                needs_user_action: false,
                details: vec![format!(
                    "Listener pairing/cache maintenance is already running for {target_name}; deferring unpair."
                )],
            };
        };
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

    pub fn unpair_listener_devices_for_known_addresses(
        extra_names: &[String],
        addresses: &[u64],
    ) -> crate::embedded_ble::BleDeviceUnpairResult {
        let target_name = extra_names
            .iter()
            .find_map(|name| {
                let trimmed = name.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .map(ToString::to_string)
            .unwrap_or_else(|| effective_bluetooth_target_name(None));
        let Some(_maintenance) =
            try_begin_listener_pairing_maintenance("unpair-fast", &target_name, Instant::now())
        else {
            return crate::embedded_ble::BleDeviceUnpairResult {
                status: crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean,
                attempted: false,
                matched_devices: 0,
                unpaired_devices: 0,
                already_unpaired_devices: 0,
                failed_devices: 0,
                needs_user_action: false,
                details: vec![format!(
                    "Listener pairing/cache maintenance is already running for {target_name}; deferring fast unpair."
                )],
            };
        };
        match unpair_listener_devices_for_known_addresses_inner(extra_names, addresses) {
            Ok(result) => result,
            Err(err) => {
                log::warn!("[embedded-ble] fast Listener address unpair unavailable: {err}");
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
        let candidates = listener_unpair_candidates(&target_names, &mut target_addresses)?;
        push_listener_recovery_target_addresses_from_candidates(&mut target_addresses, &candidates);
        if target_addresses.is_empty() {
            push_listener_recovery_advertised_addresses(&mut target_addresses, &target_names);
        }
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

    fn unpair_listener_devices_for_known_addresses_inner(
        extra_names: &[String],
        addresses: &[u64],
    ) -> Result<crate::embedded_ble::BleDeviceUnpairResult, String> {
        let target_names = listener_target_names(extra_names);
        let mut target_addresses = Vec::new();
        for address in addresses.iter().copied() {
            push_unique_address(&mut target_addresses, address);
        }
        if target_addresses.is_empty() {
            return Err(
                "fast Listener address unpair requires at least one known BLE address".to_string(),
            );
        }

        let mut candidates = Vec::new();
        let mut seen_ids = Vec::new();
        let mut errors = Vec::new();
        match push_address_unpair_candidates(&mut candidates, &mut seen_ids, &target_addresses) {
            Ok(()) => {}
            Err(err) => errors.push(err),
        }
        match push_ble_device_unpair_candidates(
            &mut candidates,
            &mut seen_ids,
            &target_addresses,
            &target_names,
        ) {
            Ok(()) => {}
            Err(err) => errors.push(err),
        }

        if candidates.is_empty() {
            if errors.is_empty() {
                return Ok(crate::embedded_ble::BleDeviceUnpairResult {
                    status: crate::embedded_ble::BleDeviceUnpairStatus::NotFound,
                    attempted: false,
                    matched_devices: 0,
                    unpaired_devices: 0,
                    already_unpaired_devices: 0,
                    failed_devices: 0,
                    needs_user_action: true,
                    details: vec![
                        "No Listener pairing entry was found for the known recovery address."
                            .to_string(),
                    ],
                });
            }
            return Err(errors.join("; "));
        }

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
                        "Removed stale Listener pairing by known recovery address: {}",
                        candidate.label
                    ));
                }
                Ok(DeviceUnpairOutcome::AlreadyUnpaired) => {
                    result.already_unpaired_devices =
                        result.already_unpaired_devices.saturating_add(1);
                    result.details.push(format!(
                        "Listener pairing was already removed by known recovery address: {}",
                        candidate.label
                    ));
                }
                Err(err) => {
                    result.failed_devices = result.failed_devices.saturating_add(1);
                    result
                        .details
                        .push(format!("Could not fast-remove {}: {err}", candidate.label));
                }
            }
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
        address: Option<u64>,
        fresh_pairing_advertisement: bool,
    }

    #[derive(Clone)]
    struct PairingPromptThrottleState {
        target_name: String,
        attempted_at: Instant,
    }

    #[derive(Clone)]
    struct PairingMaintenanceState {
        owner: &'static str,
        target_name: String,
        started_at: Instant,
        token: usize,
    }

    struct PairingMaintenanceProcessGuard {
        handle: HANDLE,
        owner: &'static str,
        target_name: String,
    }

    impl Drop for PairingMaintenanceProcessGuard {
        fn drop(&mut self) {
            if let Err(err) = unsafe { ReleaseMutex(self.handle) } {
                log::warn!(
                    "[embedded-ble] release Listener pairing maintenance mutex failed owner={} target={:?}: {err}",
                    self.owner,
                    self.target_name
                );
            }
            if let Err(err) = unsafe { CloseHandle(self.handle) } {
                log::warn!(
                    "[embedded-ble] close Listener pairing maintenance mutex failed owner={} target={:?}: {err}",
                    self.owner,
                    self.target_name
                );
            }
        }
    }

    struct BleOtaProcessGuard {
        handle: HANDLE,
        owner: &'static str,
    }

    impl Drop for BleOtaProcessGuard {
        fn drop(&mut self) {
            if let Err(err) = unsafe { ReleaseMutex(self.handle) } {
                log::warn!(
                    "[embedded-ble] release BLE OTA operation mutex failed owner={}: {err}",
                    self.owner
                );
            }
            if let Err(err) = unsafe { CloseHandle(self.handle) } {
                log::warn!(
                    "[embedded-ble] close BLE OTA operation mutex failed owner={}: {err}",
                    self.owner
                );
            }
        }
    }

    struct PairingMaintenanceGuard {
        owner: &'static str,
        target_name: String,
        token: usize,
        _process_guard: PairingMaintenanceProcessGuard,
    }

    impl Drop for PairingMaintenanceGuard {
        fn drop(&mut self) {
            let lock = listener_pairing_maintenance_slot();
            let Ok(mut guard) = lock.lock() else {
                return;
            };
            let Some(state) = guard.as_ref() else {
                return;
            };
            if state.token == self.token {
                log::info!(
                    "[embedded-ble] Listener pairing maintenance finished owner={} target={:?}",
                    self.owner,
                    self.target_name
                );
                *guard = None;
            }
        }
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

    #[derive(Clone)]
    struct RecentPairingFastGattState {
        target_name: String,
        address: Option<u64>,
        attempted_at: Instant,
    }

    fn recent_pairing_fast_gatt_slot() -> &'static Mutex<Option<RecentPairingFastGattState>> {
        static SLOT: OnceLock<Mutex<Option<RecentPairingFastGattState>>> = OnceLock::new();
        SLOT.get_or_init(|| Mutex::new(None))
    }

    fn listener_pairing_maintenance_slot() -> &'static Mutex<Option<PairingMaintenanceState>> {
        static SLOT: OnceLock<Mutex<Option<PairingMaintenanceState>>> = OnceLock::new();
        SLOT.get_or_init(|| Mutex::new(None))
    }

    fn listener_pairing_maintenance_active_for_state(
        state: &PairingMaintenanceState,
        now: Instant,
    ) -> bool {
        now.saturating_duration_since(state.started_at) < BLE_PAIRING_MAINTENANCE_MAX_WINDOW
    }

    fn try_acquire_listener_pairing_process_mutex(
        owner: &'static str,
        target_name: &str,
    ) -> Option<PairingMaintenanceProcessGuard> {
        let handle =
            unsafe { CreateMutexW(None, false, BLE_PAIRING_MAINTENANCE_MUTEX_NAME) }.map_err(
                |err| {
                    log::warn!(
                        "[embedded-ble] create Listener pairing maintenance mutex failed owner={owner} target={target_name:?}: {err}"
                    );
                    err
                },
            ).ok()?;
        let wait = unsafe { WaitForSingleObject(handle, 0) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            return Some(PairingMaintenanceProcessGuard {
                handle,
                owner,
                target_name: target_name.to_string(),
            });
        }
        if wait != WAIT_TIMEOUT {
            log::warn!(
                "[embedded-ble] Listener pairing maintenance mutex wait returned {wait:?} owner={owner} target={target_name:?}"
            );
        }
        if let Err(err) = unsafe { CloseHandle(handle) } {
            log::warn!(
                "[embedded-ble] close deferred Listener pairing maintenance mutex failed owner={owner} target={target_name:?}: {err}"
            );
        }
        None
    }

    fn try_acquire_ble_ota_process_mutex(owner: &'static str) -> Option<BleOtaProcessGuard> {
        let handle = unsafe { CreateMutexW(None, false, BLE_OTA_OPERATION_MUTEX_NAME) }
            .map_err(|err| {
                log::warn!(
                    "[embedded-ble] create BLE OTA operation mutex failed owner={owner}: {err}"
                );
                err
            })
            .ok()?;
        let wait = unsafe { WaitForSingleObject(handle, 0) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            return Some(BleOtaProcessGuard { handle, owner });
        }
        if wait != WAIT_TIMEOUT {
            log::warn!(
                "[embedded-ble] BLE OTA operation mutex wait returned {wait:?} owner={owner}"
            );
        }
        if let Err(err) = unsafe { CloseHandle(handle) } {
            log::warn!(
                "[embedded-ble] close deferred BLE OTA operation mutex failed owner={owner}: {err}"
            );
        }
        None
    }

    fn acquire_ble_ota_process_mutex(owner: &'static str) -> Result<BleOtaProcessGuard, String> {
        try_acquire_ble_ota_process_mutex(owner).ok_or_else(|| {
            format!("BLE OTA is already active in another Listener Type process ({owner}).")
        })
    }

    fn ble_ota_process_mutex_busy() -> bool {
        match try_acquire_ble_ota_process_mutex("probe") {
            Some(_guard) => false,
            None => true,
        }
    }

    pub(super) fn is_background_listener_deferred_for_ota_error(err: &str) -> bool {
        err.contains(BACKGROUND_LISTENER_DEFERRED_FOR_OTA)
    }

    fn background_listener_deferred_for_ota_error() -> String {
        BACKGROUND_LISTENER_DEFERRED_FOR_OTA.to_string()
    }

    fn listener_pairing_process_mutex_busy() -> bool {
        match try_acquire_listener_pairing_process_mutex("probe", "active-check") {
            Some(_guard) => false,
            None => true,
        }
    }

    fn try_begin_listener_pairing_maintenance(
        owner: &'static str,
        target_name: &str,
        now: Instant,
    ) -> Option<PairingMaintenanceGuard> {
        let lock = listener_pairing_maintenance_slot();
        let mut guard = lock.lock().ok()?;
        if let Some(active) = guard.as_ref() {
            if listener_pairing_maintenance_active_for_state(active, now) {
                log::warn!(
                    "[embedded-ble] Listener pairing maintenance busy owner={} target={:?}; deferring owner={} target={:?}",
                    active.owner,
                    active.target_name,
                    owner,
                    target_name
                );
                return None;
            }
            log::warn!(
                "[embedded-ble] Listener pairing maintenance stale owner={} target={:?}; replacing with owner={} target={:?}",
                active.owner,
                active.target_name,
                owner,
                target_name
            );
        }
        let Some(process_guard) = try_acquire_listener_pairing_process_mutex(owner, target_name)
        else {
            log::warn!(
                "[embedded-ble] Listener pairing maintenance busy in another process; deferring owner={owner} target={target_name:?}"
            );
            return None;
        };
        let token = LISTENER_PAIRING_MAINTENANCE_TOKEN.fetch_add(1, Ordering::SeqCst);
        *guard = Some(PairingMaintenanceState {
            owner,
            target_name: target_name.to_string(),
            started_at: now,
            token,
        });
        log::info!(
            "[embedded-ble] Listener pairing maintenance started owner={owner} target={target_name:?}"
        );
        Some(PairingMaintenanceGuard {
            owner,
            target_name: target_name.to_string(),
            token,
            _process_guard: process_guard,
        })
    }

    pub fn listener_pairing_maintenance_active() -> bool {
        let now = Instant::now();
        let lock = listener_pairing_maintenance_slot();
        let Ok(mut guard) = lock.lock() else {
            return true;
        };
        match guard.as_ref() {
            Some(state) if listener_pairing_maintenance_active_for_state(state, now) => true,
            Some(state) => {
                log::warn!(
                    "[embedded-ble] Listener pairing maintenance stale owner={} target={:?}; clearing",
                    state.owner,
                    state.target_name
                );
                *guard = None;
                listener_pairing_process_mutex_busy()
            }
            None => listener_pairing_process_mutex_busy(),
        }
    }

    fn remember_recent_pairing_fast_gatt(target_name: &str, address: Option<u64>, now: Instant) {
        if let Ok(mut guard) = recent_pairing_fast_gatt_slot().lock() {
            *guard = Some(RecentPairingFastGattState {
                target_name: target_name.to_string(),
                address,
                attempted_at: now,
            });
        }
    }

    fn recent_pairing_fast_gatt_active(now: Instant) -> Option<RecentPairingFastGattState> {
        let mut guard = recent_pairing_fast_gatt_slot().lock().ok()?;
        let state = guard.as_ref()?.clone();
        if now.saturating_duration_since(state.attempted_at) >= BLE_RECENT_PAIRING_FAST_GATT_WINDOW
        {
            *guard = None;
            return None;
        }
        Some(state)
    }

    pub fn prompt_listener_pairing(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match prompt_listener_pairing_inner(expected_name, false, false, true, false, &[]) {
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

    pub fn prompt_listener_pairing_for_recovery(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match prompt_listener_pairing_inner(expected_name, true, false, true, false, &[]) {
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

    pub fn prompt_listener_pairing_for_recovery_without_user_prompt(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match prompt_listener_pairing_inner(expected_name, true, false, false, false, &[]) {
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
                    open_bluetooth_settings: false,
                    details: vec![format!(
                        "{err}; user pairing prompt suppressed for BLE name recovery"
                    )],
                }
            }
        }
    }

    pub fn prompt_listener_pairing_after_type_recovery(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match prompt_listener_pairing_inner(expected_name, true, true, true, true, &[]) {
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

    pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match prompt_listener_pairing_inner(expected_name, true, true, false, true, &[]) {
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
                    open_bluetooth_settings: false,
                    details: vec![format!(
                        "{err}; user pairing prompt suppressed for BLE name recovery"
                    )],
                }
            }
        }
    }

    pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
            expected_name,
            &[],
        )
    }

    pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
        expected_name: Option<&str>,
        observed_recovery_addresses: &[u64],
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match prompt_listener_pairing_inner(
            expected_name,
            true,
            true,
            false,
            false,
            observed_recovery_addresses,
        ) {
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
                    open_bluetooth_settings: false,
                    details: vec![format!(
                        "{err}; user pairing prompt suppressed for BLE name recovery"
                    )],
                }
            }
        }
    }

    pub fn query_listener_pairing(
        expected_name: Option<&str>,
    ) -> crate::embedded_ble::BleDevicePairingPromptResult {
        match query_listener_pairing_inner(expected_name) {
            Ok(result) => result,
            Err(err) => {
                log::warn!("[embedded-ble] Listener pairing query unavailable: {err}");
                crate::embedded_ble::BleDevicePairingPromptResult {
                    status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
                    attempted: false,
                    matched_devices: 0,
                    prompted_devices: 0,
                    already_paired_devices: 0,
                    failed_devices: 0,
                    open_bluetooth_settings: false,
                    details: vec![err],
                }
            }
        }
    }

    fn query_listener_pairing_inner(
        expected_name: Option<&str>,
    ) -> Result<crate::embedded_ble::BleDevicePairingPromptResult, String> {
        let target_name = effective_bluetooth_target_name(expected_name);
        let target_addresses = listener_recovery_target_addresses();
        let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)
            .map_err(|err| format!("paired BLE device selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("paired BLE device query failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "paired BLE device query")
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("paired BLE device collection size failed: {err}"))?;
        let mut result = crate::embedded_ble::BleDevicePairingPromptResult {
            status: crate::embedded_ble::BleDevicePairingPromptStatus::NotFound,
            attempted: true,
            matched_devices: 0,
            prompted_devices: 0,
            already_paired_devices: 0,
            failed_devices: 0,
            open_bluetooth_settings: false,
            details: Vec::new(),
        };

        for index in 0..count {
            let info = devices
                .GetAt(index)
                .map_err(|err| format!("paired BLE device entry {index} read failed: {err}"))?;
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
            let name_matches = bluetooth_name_matches_expected(&name, &target_name);
            if !address_matches && !name_matches {
                continue;
            }

            result.matched_devices = result.matched_devices.saturating_add(1);
            let paired = info
                .Pairing()
                .and_then(|pairing| pairing.IsPaired())
                .unwrap_or(true);
            if paired {
                result.already_paired_devices = result.already_paired_devices.saturating_add(1);
                result.details.push(format!(
                    "Listener is paired: {}",
                    listener_pairing_candidate_label(&name, address)
                ));
            } else {
                result.failed_devices = result.failed_devices.saturating_add(1);
                result.details.push(format!(
                    "Listener matched but is not paired: {}",
                    listener_pairing_candidate_label(&name, address)
                ));
            }
        }

        result.status = if result.matched_devices == 0 {
            crate::embedded_ble::BleDevicePairingPromptStatus::NotFound
        } else if result.failed_devices == 0
            && result.already_paired_devices == result.matched_devices
        {
            crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
        } else {
            result.open_bluetooth_settings = true;
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
        };
        Ok(result)
    }

    fn prompt_listener_pairing_inner(
        expected_name: Option<&str>,
        bypass_prompt_suppression: bool,
        type_recovery_command_confirmed: bool,
        allow_user_pairing_prompt: bool,
        pre_pair_stale_cleanup: bool,
        observed_recovery_addresses: &[u64],
    ) -> Result<crate::embedded_ble::BleDevicePairingPromptResult, String> {
        let target_name = effective_bluetooth_target_name(expected_name);
        set_configured_bluetooth_target_name(&target_name);
        let now = Instant::now();
        if !bypass_prompt_suppression {
            if let Some(remaining) = pairing_prompt_suppression_remaining(&target_name, now) {
                log::info!(
                    "[embedded-ble] suppressing repeated Windows pairing prompt target={target_name:?} remaining_ms={}",
                    remaining.as_millis()
                );
                return Ok(suppressed_pairing_prompt_result(&target_name, remaining));
            }
        }
        let Some(_maintenance) = try_begin_listener_pairing_maintenance("pair", &target_name, now)
        else {
            return Ok(pairing_maintenance_busy_prompt_result(&target_name));
        };

        if type_recovery_command_confirmed && pre_pair_stale_cleanup {
            match unpair_listener_devices_inner(std::slice::from_ref(&target_name)) {
                Ok(unpair) => {
                    log::warn!(
                        "[embedded-ble] Type recovery pre-pair stale cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={}",
                        unpair.status,
                        unpair.matched_devices,
                        unpair.unpaired_devices,
                        unpair.already_unpaired_devices,
                        unpair.failed_devices,
                        unpair.needs_user_action,
                    );
                    if unpair.unpaired_devices > 0 {
                        std::thread::sleep(Duration::from_millis(700));
                    }
                }
                Err(err) => log::warn!(
                    "[embedded-ble] Type recovery pre-pair stale cleanup failed before PairAsync: {err}"
                ),
            }
        }

        let mut candidates = if bypass_prompt_suppression {
            listener_recovery_pairing_candidates_for_addresses(
                Some(&target_name),
                observed_recovery_addresses,
            )?
        } else {
            listener_pairing_candidates(Some(&target_name))?
        };
        if type_recovery_command_confirmed {
            let mut trusted_addresses = None;
            for candidate in &mut candidates {
                if candidate.fresh_pairing_advertisement {
                    continue;
                }
                let trusted_addresses =
                    trusted_addresses.get_or_insert_with(listener_recovery_target_addresses);
                if listener_pairing_candidate_has_trusted_address(candidate, trusted_addresses) {
                    log::warn!(
                        "[embedded-ble] Type recovery command confirmed for {}; allowing stale paired cache cleanup without fresh advertisement",
                        candidate.label
                    );
                    candidate.fresh_pairing_advertisement = true;
                } else if candidate.address.is_some() {
                    log::warn!(
                        "[embedded-ble] Type recovery command confirmed, but {} is only a same-name cached candidate without trusted address proof; not using it for stale cache cleanup",
                        candidate.label
                    );
                }
            }
        }
        let mut result = crate::embedded_ble::BleDevicePairingPromptResult {
            status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
            attempted: true,
            matched_devices: candidates.len() as u32,
            prompted_devices: 0,
            already_paired_devices: 0,
            failed_devices: 0,
            open_bluetooth_settings: allow_user_pairing_prompt,
            details: Vec::new(),
        };

        let allow_adapter_restart = allow_user_pairing_prompt && !type_recovery_command_confirmed;
        let fast_recovery_pairing_failure = pair_listener_candidates_into_prompt_result(
            &mut result,
            candidates,
            &target_name,
            bypass_prompt_suppression,
            bypass_prompt_suppression,
            allow_adapter_restart,
            type_recovery_command_confirmed,
        );
        if bypass_prompt_suppression
            && result.prompted_devices == 0
            && result.already_paired_devices == 0
            && fast_recovery_pairing_failure
        {
            match listener_recovery_pairing_selector_fallback_candidates_for_addresses(
                Some(&target_name),
                observed_recovery_addresses,
            ) {
                Ok(fallback_candidates) if !fallback_candidates.is_empty() => {
                    log::warn!(
                        "[embedded-ble] recovery direct PairAsync failed before Windows pairing ceremony; trying {} slow AEP fallback candidate(s)",
                        fallback_candidates.len()
                    );
                    let failed_before_fallback = result.failed_devices;
                    let prompted_before_fallback = result.prompted_devices;
                    let already_before_fallback = result.already_paired_devices;
                    result.matched_devices = result
                        .matched_devices
                        .saturating_add(fallback_candidates.len() as u32);
                    let _ = pair_listener_candidates_into_prompt_result(
                        &mut result,
                        fallback_candidates,
                        &target_name,
                        false,
                        true,
                        allow_adapter_restart,
                        false,
                    );
                    if result.prompted_devices > prompted_before_fallback
                        || result.already_paired_devices > already_before_fallback
                    {
                        result.failed_devices =
                            result.failed_devices.saturating_sub(failed_before_fallback);
                        result.details.push(
                            "Windows pairing recovered via slow AEP fallback after direct address PairAsync failed before the pairing ceremony".to_string(),
                        );
                    }
                }
                Ok(_) => {
                    log::warn!(
                        "[embedded-ble] recovery direct PairAsync failed before Windows pairing ceremony, but slow AEP fallback found no candidate"
                    );
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] recovery slow AEP fallback failed after direct PairAsync failure: {err}"
                    );
                }
            }
        }

        if result.matched_devices == 0 {
            result.status = crate::embedded_ble::BleDevicePairingPromptStatus::NotFound;
            result.open_bluetooth_settings = allow_user_pairing_prompt;
            if allow_user_pairing_prompt {
                result.details.push(
                    "No pairable Listener device object was visible to Windows. Bluetooth settings will open for native pairing."
                        .to_string(),
                );
            } else {
                result.details.push(
                    "No pairable Listener device object was visible to Windows; user pairing prompt is suppressed for this Type-controlled recovery."
                        .to_string(),
                );
            }
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
            result.open_bluetooth_settings = allow_user_pairing_prompt;
            if !allow_user_pairing_prompt {
                result.details.push(
                    "Windows pairing did not complete automatically; user pairing prompt is suppressed for this Type-controlled recovery."
                        .to_string(),
                );
            }
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
        };
        if allow_user_pairing_prompt && (result.prompted_devices > 0 || result.failed_devices > 0) {
            remember_pairing_prompt_attempt(&target_name, now);
        }
        Ok(result)
    }

    fn pair_listener_candidates_into_prompt_result(
        result: &mut crate::embedded_ble::BleDevicePairingPromptResult,
        candidates: Vec<ListenerPairingCandidate>,
        target_name: &str,
        track_fast_failure_for_aep_fallback: bool,
        verify_already_paired_liveness: bool,
        allow_adapter_restart: bool,
        retry_failed_pairasync_once: bool,
    ) -> bool {
        let mut fast_pairing_failure = false;
        for candidate in candidates {
            let attempt_started = Instant::now();
            match pair_listener_candidate(
                &candidate,
                Some(target_name),
                verify_already_paired_liveness,
                allow_adapter_restart,
                retry_failed_pairasync_once,
            ) {
                Ok(DevicePairingOutcome::Paired) => {
                    remember_recent_pairing_fast_gatt(
                        target_name,
                        candidate.address,
                        Instant::now(),
                    );
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
                    if track_fast_failure_for_aep_fallback
                        && attempt_started.elapsed()
                            <= BLE_PAIRING_FAST_FAILURE_AEP_FALLBACK_THRESHOLD
                    {
                        fast_pairing_failure = true;
                    }
                    result.failed_devices = result.failed_devices.saturating_add(1);
                    result.details.push(format!(
                        "Could not start Windows pairing for {}: {err}",
                        candidate.label
                    ));
                }
            }
        }
        fast_pairing_failure
    }

    fn pairing_prompt_suppression_remaining(_target_name: &str, now: Instant) -> Option<Duration> {
        let prompt_lock = LAST_PAIRING_PROMPT.get_or_init(|| Mutex::new(None));
        let guard = prompt_lock.lock().ok()?;
        let last = guard.as_ref()?;
        pairing_prompt_suppression_remaining_for_state(last, _target_name, now)
    }

    fn pairing_prompt_suppression_remaining_for_state(
        last: &PairingPromptThrottleState,
        target_name: &str,
        now: Instant,
    ) -> Option<Duration> {
        if last.target_name != target_name {
            return None;
        }
        let elapsed = now.saturating_duration_since(last.attempted_at);
        if elapsed >= BLE_PAIRING_PROMPT_SUPPRESS_WINDOW {
            return None;
        }
        Some(BLE_PAIRING_PROMPT_SUPPRESS_WINDOW - elapsed)
    }

    fn remember_pairing_prompt_attempt(_target_name: &str, now: Instant) {
        let prompt_lock = LAST_PAIRING_PROMPT.get_or_init(|| Mutex::new(None));
        if let Ok(mut guard) = prompt_lock.lock() {
            *guard = Some(PairingPromptThrottleState {
                target_name: _target_name.to_string(),
                attempted_at: now,
            });
        }
    }

    #[cfg(test)]
    pub(super) fn pairing_prompt_suppression_remaining_for_test(
        last_target_name: &str,
        current_target_name: &str,
        elapsed: Duration,
    ) -> Option<Duration> {
        let attempted_at = Instant::now();
        let now = attempted_at.checked_add(elapsed).unwrap_or(attempted_at);
        let last = PairingPromptThrottleState {
            target_name: last_target_name.to_string(),
            attempted_at,
        };
        pairing_prompt_suppression_remaining_for_state(&last, current_target_name, now)
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

    fn pairing_maintenance_busy_prompt_result(
        target_name: &str,
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
                "Listener pairing/cache maintenance is already running for {target_name}; waiting for it to finish."
            )],
        }
    }

    fn listener_pairing_candidates(
        expected_name: Option<&str>,
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        let expected_name = expected_name
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut candidates =
            listener_pairing_candidates_from_unpaired_selector(expected_name, &[])?;

        if candidates.is_empty() {
            let mut seen_ids = Vec::new();
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

    fn listener_pairing_candidates_from_unpaired_selector(
        expected_name: Option<&str>,
        extra_target_addresses: &[u64],
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        listener_pairing_candidates_from_unpaired_selector_with_timeout(
            expected_name,
            extra_target_addresses,
            BLE_PAIRING_DISCOVERY_TIMEOUT,
        )
    }

    fn listener_pairing_candidates_from_unpaired_selector_with_timeout(
        expected_name: Option<&str>,
        extra_target_addresses: &[u64],
        discovery_timeout: Duration,
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        let mut target_addresses = listener_recovery_target_addresses();
        for address in extra_target_addresses.iter().copied() {
            push_unique_address(&mut target_addresses, address);
        }
        match listener_pairing_candidates_from_windows_ble_aep(
            expected_name,
            &target_addresses,
            discovery_timeout,
        ) {
            Ok(candidates) if !candidates.is_empty() => {
                log::info!(
                    "[embedded-ble] Windows BLE AEP pairing query found {} candidate(s)",
                    candidates.len()
                );
                return Ok(candidates);
            }
            Ok(_) => {
                log::warn!(
                    "[embedded-ble] Windows BLE AEP pairing query found no candidate; falling back to BluetoothLEDevice unpaired selector"
                );
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] Windows BLE AEP pairing query failed; falling back to BluetoothLEDevice unpaired selector: {err}"
                );
            }
        }

        let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(false)
            .map_err(|err| format!("unpaired BLE device selector failed: {err}"))?;
        let mut candidates = Vec::new();
        let mut seen_ids = Vec::new();
        let query_result = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("unpaired BLE device query failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, discovery_timeout, "unpaired BLE device query")
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
                        false,
                    );
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] unpaired BLE device query failed before pairing prompt; trying advertisement fallback: {err}"
                );
            }
        }

        Ok(candidates)
    }

    fn listener_pairing_candidates_from_windows_ble_aep(
        expected_name: Option<&str>,
        target_addresses: &[u64],
        discovery_timeout: Duration,
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        match listener_pairing_candidates_from_windows_ble_aep_selector(
            expected_name,
            target_addresses,
            WINDOWS_BLE_AEP_CONNECTABLE_SELECTOR,
            "connectable BLE AEP device query",
            true,
            discovery_timeout,
        ) {
            Ok(candidates) if !candidates.is_empty() => {
                log::info!(
                    "[embedded-ble] Windows connectable BLE AEP query found {} candidate(s)",
                    candidates.len()
                );
                return Ok(candidates);
            }
            Ok(_) => {
                log::warn!(
                    "[embedded-ble] Windows connectable BLE AEP query found no candidate; checking full AEP cache as fallback"
                );
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] Windows connectable BLE AEP query failed; checking full AEP cache as fallback: {err}"
                );
            }
        }

        listener_pairing_candidates_from_windows_ble_aep_selector(
            expected_name,
            target_addresses,
            WINDOWS_BLE_AEP_SELECTOR,
            "BLE AEP device query",
            false,
            discovery_timeout,
        )
    }

    fn listener_pairing_candidates_from_windows_ble_aep_selector(
        expected_name: Option<&str>,
        target_addresses: &[u64],
        selector_text: &str,
        query_label: &str,
        connectable_selector: bool,
        discovery_timeout: Duration,
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        let selector = HSTRING::from(selector_text);
        let query_result = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
            &selector,
            None::<&windows::Foundation::Collections::IIterable<HSTRING>>,
            DeviceInformationKind::AssociationEndpoint,
        )
        .map_err(|err| format!("{query_label} failed: {err}"))
        .and_then(|op| wait_async_operation(op, discovery_timeout, query_label));

        let mut candidates = Vec::new();
        let mut seen_ids = Vec::new();
        match query_result {
            Ok(devices) => {
                let count = devices
                    .Size()
                    .map_err(|err| format!("{query_label} collection size failed: {err}"))?;
                for index in 0..count {
                    let info = devices
                        .GetAt(index)
                        .map_err(|err| format!("{query_label} entry {index} read failed: {err}"))?;
                    push_listener_pairing_candidate_if_matching(
                        &mut candidates,
                        &mut seen_ids,
                        info,
                        expected_name,
                        target_addresses,
                        connectable_selector,
                    );
                }
            }
            Err(err) => return Err(err),
        }

        Ok(candidates)
    }

    fn listener_recovery_pairing_candidates(
        expected_name: Option<&str>,
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        listener_recovery_pairing_candidates_for_addresses(expected_name, &[])
    }

    fn listener_recovery_pairing_candidates_for_addresses(
        expected_name: Option<&str>,
        observed_recovery_addresses: &[u64],
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        let expected_name = expected_name
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut seen_ids = Vec::new();
        let mut addresses = if observed_recovery_addresses.is_empty() {
            listener_recovery_target_addresses()
        } else {
            Vec::new()
        };
        let mut fresh_advertised_addresses = Vec::new();

        if !observed_recovery_addresses.is_empty() {
            for address in observed_recovery_addresses.iter().copied() {
                push_unique_address(&mut addresses, address);
                push_unique_address(&mut fresh_advertised_addresses, address);
            }
            let observed_labels = fresh_advertised_addresses
                .iter()
                .copied()
                .map(crate::embedded_ble::format_bluetooth_address)
                .collect::<Vec<_>>();
            log::info!(
                "[embedded-ble] recovery pairing using Type-observed Swift Pair address(es) before any duplicate advertisement scan: {observed_labels:?}"
            );
            let candidates = listener_recovery_direct_pairing_candidates(
                &fresh_advertised_addresses,
                &fresh_advertised_addresses,
                expected_name,
                &mut seen_ids,
            );
            if !candidates.is_empty() {
                log::info!(
                    "[embedded-ble] recovery pairing using {} Type-observed direct address candidate(s) before slow advertisement/AEP discovery",
                    candidates.len()
                );
                return Ok(candidates);
            }
            log::warn!(
                "[embedded-ble] Type-observed recovery address(es) had no direct pairing DeviceInformation; falling back to recovery advertisement scan"
            );
        }

        match scan_listener_pairing_advertisements(expected_name) {
            Ok(advertised) => {
                for (address, _address_type, name) in advertised {
                    if !listener_pairing_name_matches(&name, expected_name)
                        && !addresses.contains(&address)
                    {
                        continue;
                    }
                    push_unique_address(&mut addresses, address);
                    push_unique_address(&mut fresh_advertised_addresses, address);
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] recovery pairing advertisement scan failed before direct pair: {err}"
                );
            }
        }

        let direct_addresses = if fresh_advertised_addresses.is_empty() {
            addresses.as_slice()
        } else {
            let fresh_labels = fresh_advertised_addresses
                .iter()
                .copied()
                .map(crate::embedded_ble::format_bluetooth_address)
                .collect::<Vec<_>>();
            log::info!(
                "[embedded-ble] recovery pairing using fresh advertised address(es) before stale configured/PnP addresses: {fresh_labels:?}"
            );
            fresh_advertised_addresses.as_slice()
        };
        let candidates = listener_recovery_direct_pairing_candidates(
            direct_addresses,
            &fresh_advertised_addresses,
            expected_name,
            &mut seen_ids,
        );
        if !candidates.is_empty() {
            log::info!(
                "[embedded-ble] recovery pairing using {} direct advertisement/address candidate(s) before slow Windows AEP selector",
                candidates.len()
            );
            return Ok(candidates);
        }

        match listener_pairing_candidates_from_unpaired_selector(expected_name, direct_addresses) {
            Ok(selector_candidates) if !selector_candidates.is_empty() => {
                log::info!(
                    "[embedded-ble] recovery pairing using {} Windows unpaired selector candidate(s) after direct address lookup found no candidate",
                    selector_candidates.len()
                );
                return Ok(selector_candidates);
            }
            Ok(_) => {
                log::warn!(
                    "[embedded-ble] recovery pairing Windows unpaired selector had no candidate after direct address lookup"
                );
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] recovery pairing unpaired selector query failed after direct lookup: {err}"
                );
            }
        }

        log::warn!(
            "[embedded-ble] recovery pairing selector and direct lookup had no candidate; falling back to normal pairing discovery"
        );
        listener_pairing_candidates(expected_name)
    }

    fn listener_recovery_pairing_selector_fallback_candidates(
        expected_name: Option<&str>,
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        listener_recovery_pairing_selector_fallback_candidates_for_addresses(expected_name, &[])
    }

    fn listener_recovery_pairing_selector_fallback_candidates_for_addresses(
        expected_name: Option<&str>,
        observed_recovery_addresses: &[u64],
    ) -> Result<Vec<ListenerPairingCandidate>, String> {
        let expected_name = expected_name
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut addresses = listener_recovery_target_addresses();
        let mut fresh_advertised_addresses = Vec::new();
        for address in observed_recovery_addresses.iter().copied() {
            push_unique_address(&mut addresses, address);
            push_unique_address(&mut fresh_advertised_addresses, address);
        }
        if !fresh_advertised_addresses.is_empty() {
            let observed_labels = fresh_advertised_addresses
                .iter()
                .copied()
                .map(crate::embedded_ble::format_bluetooth_address)
                .collect::<Vec<_>>();
            log::info!(
                "[embedded-ble] recovery AEP fallback using Type-observed Swift Pair address(es) without duplicate advertisement scan: {observed_labels:?}"
            );
        }
        if fresh_advertised_addresses.is_empty() {
            match scan_listener_pairing_advertisements(expected_name) {
                Ok(advertised) => {
                    for (address, _address_type, name) in advertised {
                        if listener_pairing_name_matches(&name, expected_name)
                            || addresses.contains(&address)
                        {
                            push_unique_address(&mut addresses, address);
                            push_unique_address(&mut fresh_advertised_addresses, address);
                        }
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] recovery AEP fallback advertisement scan failed: {err}"
                    );
                }
            }
        }

        let selector_addresses = if fresh_advertised_addresses.is_empty() {
            addresses.as_slice()
        } else {
            fresh_advertised_addresses.as_slice()
        };
        match listener_pairing_candidates_from_unpaired_selector_with_timeout(
            expected_name,
            selector_addresses,
            BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT,
        ) {
            Ok(candidates) if !candidates.is_empty() => Ok(candidates),
            Ok(_) => {
                if !fresh_advertised_addresses.is_empty() {
                    log::warn!(
                        "[embedded-ble] recovery AEP fallback found no candidate for fresh advertised address; not falling back to stale same-name cache"
                    );
                    return Ok(Vec::new());
                }
                log::warn!(
                    "[embedded-ble] recovery AEP fallback found no filtered unpaired selector candidate; using normal pairing discovery"
                );
                listener_pairing_candidates(expected_name)
            }
            Err(err) => {
                if !fresh_advertised_addresses.is_empty() {
                    log::warn!(
                        "[embedded-ble] recovery AEP fallback unpaired selector failed for fresh advertised address; not falling back to stale same-name cache: {err}"
                    );
                    return Ok(Vec::new());
                }
                log::warn!(
                    "[embedded-ble] recovery AEP fallback unpaired selector failed; using normal pairing discovery: {err}"
                );
                listener_pairing_candidates(expected_name)
            }
        }
    }

    fn listener_recovery_direct_pairing_candidates(
        addresses: &[u64],
        fresh_advertised_addresses: &[u64],
        expected_name: Option<&str>,
        seen_ids: &mut Vec<String>,
    ) -> Vec<ListenerPairingCandidate> {
        let mut candidates = Vec::new();
        for address in addresses.iter().copied() {
            let address_text = crate::embedded_ble::format_bluetooth_address(address);
            match pairing_device_information_from_bluetooth_address_handle(
                address,
                "recovery direct BLE pairing",
            ) {
                Ok(Some(info)) => {
                    let id = info
                        .Id()
                        .map(|value| value.to_string_lossy())
                        .unwrap_or_default();
                    if id.is_empty() || seen_ids.iter().any(|seen| seen == &id) {
                        continue;
                    }
                    let name = info
                        .Name()
                        .map(|value| value.to_string_lossy())
                        .unwrap_or_default();
                    if !listener_pairing_name_matches(&name, expected_name)
                        && parse_bluetooth_address_from_device_id(&id) != Some(address)
                    {
                        continue;
                    }
                    seen_ids.push(id);
                    log::info!(
                        "[embedded-ble] recovery direct pairing candidate name={name:?} address={address_text}"
                    );
                    let label = listener_pairing_candidate_label(&name, Some(address));
                    candidates.push(ListenerPairingCandidate {
                        label,
                        info,
                        address: Some(address),
                        fresh_pairing_advertisement: fresh_advertised_addresses.contains(&address),
                    });
                }
                Ok(None) => {
                    log::warn!(
                        "[embedded-ble] recovery direct pairing found no DeviceInformation for {address_text}"
                    );
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] recovery direct pairing address open failed for {address_text}: {err}"
                    );
                }
            }
        }
        candidates
    }

    fn push_listener_pairing_candidate_if_matching(
        candidates: &mut Vec<ListenerPairingCandidate>,
        seen_ids: &mut Vec<String>,
        info: DeviceInformation,
        expected_name: Option<&str>,
        target_addresses: &[u64],
        connectable_selector: bool,
    ) {
        let name = device_information_display_name(&info);
        let id = info
            .Id()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let address = device_information_bluetooth_address(&info)
            .or_else(|| parse_bluetooth_address_from_device_id(&id));
        let address_matches = address.is_some_and(|value| target_addresses.contains(&value));
        if !target_addresses.is_empty() && address.is_some() && !address_matches {
            let candidate_address = address.map(crate::embedded_ble::format_bluetooth_address);
            let target_address_labels = target_addresses
                .iter()
                .copied()
                .map(crate::embedded_ble::format_bluetooth_address)
                .collect::<Vec<_>>();
            log::info!(
                "[embedded-ble] skipping stale Windows pairing candidate name={name:?} id={id} address={candidate_address:?}; target_addresses={target_address_labels:?}"
            );
            return;
        }
        if !address_matches && !listener_pairing_name_matches(&name, expected_name) {
            return;
        }
        if id.is_empty() || seen_ids.iter().any(|seen| seen == &id) {
            return;
        }
        let kind = info.Kind().ok();
        let aep_address =
            device_information_property_string(&info, WINDOWS_AEP_DEVICE_ADDRESS_PROPERTY);
        let aep_is_paired = device_information_property_bool(&info, WINDOWS_AEP_IS_PAIRED_PROPERTY);
        let aep_is_connected =
            device_information_property_bool(&info, WINDOWS_AEP_IS_CONNECTED_PROPERTY);
        let aep_is_present =
            device_information_property_bool(&info, WINDOWS_AEP_IS_PRESENT_PROPERTY);
        let aep_is_connectable =
            device_information_property_bool(&info, WINDOWS_AEP_BLE_IS_CONNECTABLE_PROPERTY)
                .or(connectable_selector.then_some(true));
        if !connectable_selector
            && !target_addresses.is_empty()
            && aep_is_connectable == Some(false)
            && aep_is_connected != Some(true)
        {
            log::info!(
                "[embedded-ble] skipping non-connectable Windows BLE AEP name={name:?} id={id} address={:?} aep_address={aep_address:?} aep_is_present={aep_is_present:?} aep_is_connected={aep_is_connected:?}",
                address.map(crate::embedded_ble::format_bluetooth_address)
            );
            return;
        }
        log::info!(
            "[embedded-ble] Windows pairing discovery candidate name={name:?} id={id} kind={kind:?} address={:?} aep_address={aep_address:?} aep_is_paired={aep_is_paired:?} aep_is_connected={aep_is_connected:?} aep_is_present={aep_is_present:?} aep_is_connectable={aep_is_connectable:?} connectable_selector={connectable_selector}",
            address.map(crate::embedded_ble::format_bluetooth_address)
        );
        seen_ids.push(id);
        let label = listener_pairing_candidate_label(&name, address);
        candidates.push(ListenerPairingCandidate {
            label,
            info,
            address,
            fresh_pairing_advertisement: false,
        });
    }

    fn device_information_display_name(info: &DeviceInformation) -> String {
        info.Name()
            .map(|value| value.to_string_lossy())
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                device_information_property_string(info, WINDOWS_ITEM_NAME_DISPLAY_PROPERTY)
            })
            .unwrap_or_default()
    }

    fn device_information_bluetooth_address(info: &DeviceInformation) -> Option<u64> {
        device_information_property_string(info, WINDOWS_AEP_DEVICE_ADDRESS_PROPERTY)
            .as_deref()
            .and_then(parse_bluetooth_address_hex)
    }

    fn device_information_property_string(info: &DeviceInformation, key: &str) -> Option<String> {
        let properties = info.Properties().ok()?;
        let key = HSTRING::from(key);
        if !properties.HasKey(&key).ok()? {
            return None;
        }
        let value = properties.Lookup(&key).ok()?;
        let value = value.cast::<IPropertyValue>().ok()?;
        value.GetString().ok().map(|value| value.to_string_lossy())
    }

    fn device_information_property_bool(info: &DeviceInformation, key: &str) -> Option<bool> {
        let properties = info.Properties().ok()?;
        let key = HSTRING::from(key);
        if !properties.HasKey(&key).ok()? {
            return None;
        }
        let value = properties.Lookup(&key).ok()?;
        let value = value.cast::<IPropertyValue>().ok()?;
        value.GetBoolean().ok()
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
            candidates.push(ListenerPairingCandidate {
                label,
                info,
                address: Some(address),
                fresh_pairing_advertisement: true,
            });
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

        match pairing_device_information_from_bluetooth_address_handle(address, label) {
            Ok(Some(info)) => return Ok(Some(info)),
            Ok(None) => {}
            Err(err) => {
                log::warn!(
                    "[embedded-ble] BLE transient address open for {address_text} failed: {err}"
                );
            }
        }

        log::warn!(
            "[embedded-ble] BLE pairing selector had no pairable DeviceInformation for {address_text}; opening Bluetooth settings"
        );
        Ok(None)
    }

    fn pairing_device_information_from_bluetooth_address_handle(
        address: u64,
        label: &str,
    ) -> Result<Option<DeviceInformation>, String> {
        let address_text = crate::embedded_ble::format_bluetooth_address(address);
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(address)
            .map_err(|err| format!("{label} transient BLE device open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    BLE_DISCOVERY_TIMEOUT,
                    "transient advertised BLE pairing device open",
                )
            })?;
        let info = match device.DeviceInformation() {
            Ok(info) => {
                log::info!(
                    "[embedded-ble] opened transient BLE DeviceInformation for pairing address={address_text}"
                );
                Some(info)
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] transient BLE DeviceInformation unavailable for {address_text}: {err}"
                );
                None
            }
        };
        let _ = device.Close();
        Ok(info)
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

    fn listener_pairing_candidate_has_trusted_address(
        candidate: &ListenerPairingCandidate,
        trusted_addresses: &[u64],
    ) -> bool {
        candidate
            .address
            .is_some_and(|address| trusted_addresses.contains(&address))
    }

    fn listener_pairing_name_matches(name: &str, expected_name: Option<&str>) -> bool {
        let expected_name = effective_bluetooth_target_name(expected_name);
        bluetooth_name_matches_expected(name, &expected_name)
    }

    fn pair_listener_candidate(
        candidate: &ListenerPairingCandidate,
        expected_name: Option<&str>,
        verify_already_paired_liveness: bool,
        allow_adapter_restart: bool,
        retry_failed_pairasync_once: bool,
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
                if !candidate.fresh_pairing_advertisement {
                    let trusted_addresses = listener_recovery_target_addresses();
                    if !listener_pairing_candidate_has_trusted_address(
                        &candidate,
                        &trusted_addresses,
                    ) {
                        return Err(format!(
                            "Windows only reports {} from a same-name cached pairing without matching Listener address/service proof",
                            candidate.label
                        ));
                    }
                    if verify_already_paired_liveness {
                        if listener_trusted_paired_candidate_has_fresh_status(&candidate) {
                            log::info!(
                                "[embedded-ble] Windows reports {} is already paired and fresh GATT status is live; leaving pairing intact",
                                candidate.label
                            );
                            return Ok(DevicePairingOutcome::AlreadyPaired);
                        }
                        log::warn!(
                            "[embedded-ble] Windows reports {} is already paired, but fresh GATT status is not live during recovery; clearing stale host bond before PairAsync",
                            candidate.label
                        );
                        candidate.fresh_pairing_advertisement = true;
                    } else {
                        log::info!(
                        "[embedded-ble] Windows reports {} is already paired from trusted cached/AEP discovery; leaving pairing intact",
                        candidate.label
                    );
                        return Ok(DevicePairingOutcome::AlreadyPaired);
                    }
                }
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

            return pair_unpaired_listener_candidate(
                &candidate,
                &pairing,
                expected_name,
                allow_adapter_restart,
                retry_failed_pairasync_once,
            );
        }

        Err(
            "Windows stale pairing cleanup exhausted without a pairable Listener candidate"
                .to_string(),
        )
    }

    fn listener_trusted_paired_candidate_has_fresh_status(
        candidate: &ListenerPairingCandidate,
    ) -> bool {
        let Some(address) = candidate.address else {
            return false;
        };
        let deadline = Instant::now() + Duration::from_millis(4500);
        match open_embedded_audio_status_target_for_device(address, deadline) {
            Ok(target) => {
                let status = read_embedded_audio_status_from_target_bounded(&target, deadline);
                if status.connected {
                    return true;
                }
                log::warn!(
                    "[embedded-ble] already-paired recovery liveness check failed for {}: {}",
                    candidate.label,
                    status.detail.unwrap_or_else(|| {
                        "fresh BLE status read did not confirm live link".to_string()
                    })
                );
                false
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] already-paired recovery liveness check could not open {}: {err}",
                    candidate.label
                );
                false
            }
        }
    }

    fn pair_unpaired_listener_candidate(
        candidate: &ListenerPairingCandidate,
        pairing: &DeviceInformationPairing,
        expected_name: Option<&str>,
        allow_adapter_restart: bool,
        retry_failed_pairasync_once: bool,
    ) -> Result<DevicePairingOutcome, String> {
        pair_unpaired_listener_candidate_with_adapter_recovery(
            candidate,
            pairing,
            expected_name,
            allow_adapter_restart,
            retry_failed_pairasync_once,
        )
    }

    fn pair_unpaired_listener_candidate_with_adapter_recovery(
        candidate: &ListenerPairingCandidate,
        pairing: &DeviceInformationPairing,
        expected_name: Option<&str>,
        allow_adapter_restart: bool,
        retry_failed_pairasync_once: bool,
    ) -> Result<DevicePairingOutcome, String> {
        let can_pair = pairing
            .CanPair()
            .map_err(|err| format!("read pairing capability failed: {err}"))?;
        let candidate_id = candidate
            .info
            .Id()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        log::info!(
            "[embedded-ble] Windows pairing candidate label={} id={} can_pair={}",
            candidate.label,
            candidate_id,
            can_pair
        );
        if !can_pair {
            return pairing_status_or_reachable(
                DevicePairingResultStatus::NotReadyToPair,
                expected_name,
                "Windows reports the device is not ready to pair",
            );
        }

        let mut status = run_default_pairing_once(pairing, &candidate.label, "standard pairing")?;
        match pairing_status_or_reachable(status, expected_name, "") {
            Ok(outcome) => return Ok(outcome),
            Err(_) => {}
        }
        if pairing_status_should_retry_after_settle(status) {
            let settle = Duration::from_millis(700);
            log::warn!(
                "[embedded-ble] Windows standard pairing returned retryable status={status:?} for {}; retrying once after {} ms so Listener can refresh pairing advertising",
                candidate.label,
                settle.as_millis()
            );
            std::thread::sleep(settle);
            status = run_default_pairing_once(pairing, &candidate.label, "standard pairing retry")?;
            match pairing_status_or_reachable(status, expected_name, "") {
                Ok(outcome) => return Ok(outcome),
                Err(_) => {}
            }
        }
        let mut custom_pairing_already_in_progress = false;
        if pairing_status_should_try_custom_fallback(status) {
            log::warn!(
                "[embedded-ble] Windows standard pairing returned status={status:?} for {}; trying custom PairAsync fallback",
                candidate.label
            );
            match custom_pair_listener_candidate(pairing, &candidate.label, expected_name) {
                Ok(outcome) => return Ok(outcome),
                Err(err) => {
                    if err.contains("already pairing") {
                        custom_pairing_already_in_progress = true;
                    }
                    log::warn!(
                        "[embedded-ble] Windows custom pairing fallback failed for {} after standard status={status:?}: {err}",
                        candidate.label
                    );
                }
            }
        }
        log::warn!(
            "[embedded-ble] Windows standard pairing final status={status:?} for {}",
            candidate.label
        );
        let final_result = pairing_status_or_reachable(
            status,
            expected_name,
            &format!("Windows returned pairing status={status:?}"),
        );
        if final_result.is_err()
            && !retry_failed_pairasync_once
            && !allow_adapter_restart
            && (custom_pairing_already_in_progress
                || pairing_status_suggests_adapter_restart(status))
        {
            log::warn!(
                "[embedded-ble] Windows PairAsync did not finish cleanly for {}, but Type automatic recovery will not restart the local Bluetooth adapter; waiting {} ms for in-progress Windows pairing to settle",
                candidate.label,
                BLE_PAIRING_IN_PROGRESS_SETTLE.as_millis()
            );
            std::thread::sleep(BLE_PAIRING_IN_PROGRESS_SETTLE);
            match refresh_listener_pairing_candidate(candidate, expected_name)? {
                Some(refreshed) => {
                    let refreshed_pairing = refreshed
                        .info
                        .Pairing()
                        .map_err(|err| format!("read refreshed pairing info failed: {err}"))?;
                    if refreshed_pairing
                        .IsPaired()
                        .map_err(|err| format!("read refreshed pairing state failed: {err}"))?
                    {
                        log::info!(
                            "[embedded-ble] Windows reports {} paired after passive in-progress PairAsync settle",
                            refreshed.label
                        );
                        return Ok(DevicePairingOutcome::AlreadyPaired);
                    }
                }
                None => {
                    log::warn!(
                        "[embedded-ble] Windows PairAsync settle finished for {}, but Listener pairing candidate was not visible yet",
                        candidate.label
                    );
                }
            }
        }
        if final_result.is_err()
            && retry_failed_pairasync_once
            && !allow_adapter_restart
            && pairing_status_suggests_adapter_restart(status)
        {
            log::warn!(
                "[embedded-ble] Type automatic recovery retrying direct PairAsync once after Windows Failed(19) for {} without restarting the local Bluetooth adapter",
                candidate.label
            );
            std::thread::sleep(Duration::from_millis(1200));
            match refresh_listener_pairing_candidate(candidate, expected_name)? {
                Some(refreshed) => {
                    let refreshed_pairing = refreshed
                        .info
                        .Pairing()
                        .map_err(|err| format!("read refreshed pairing info failed: {err}"))?;
                    match pair_unpaired_listener_candidate_with_adapter_recovery(
                        &refreshed,
                        &refreshed_pairing,
                        expected_name,
                        false,
                        false,
                    ) {
                        Ok(outcome) => return Ok(outcome),
                        Err(err) => log::warn!(
                            "[embedded-ble] Type automatic recovery one-shot PairAsync retry failed for {}: {err}",
                            refreshed.label
                        ),
                    }
                }
                None => {
                    log::warn!(
                        "[embedded-ble] Type automatic recovery one-shot PairAsync retry skipped for {} because the fresh Listener candidate was not visible",
                        candidate.label
                    );
                }
            }
        }
        if final_result.is_err()
            && allow_adapter_restart
            && pairing_status_suggests_adapter_restart(status)
        {
            match restart_windows_bluetooth_adapter_after_pairing_failure(&candidate.label) {
                Ok(detail) => {
                    log::warn!(
                        "[embedded-ble] Windows Bluetooth adapter restart completed after PairAsync failure for {}: {detail}",
                        candidate.label
                    );
                    std::thread::sleep(BLE_ADAPTER_RESTART_SETTLE);
                    match refresh_listener_pairing_candidate(candidate, expected_name)? {
                        Some(refreshed) => {
                            let refreshed_pairing = refreshed.info.Pairing().map_err(|err| {
                                format!("read refreshed pairing info failed: {err}")
                            })?;
                            if refreshed_pairing.IsPaired().map_err(|err| {
                                format!("read refreshed pairing state failed: {err}")
                            })? {
                                log::info!(
                                    "[embedded-ble] Windows reports {} paired after adapter restart",
                                    refreshed.label
                                );
                                return Ok(DevicePairingOutcome::AlreadyPaired);
                            }
                            return pair_unpaired_listener_candidate_with_adapter_recovery(
                                &refreshed,
                                &refreshed_pairing,
                                expected_name,
                                false,
                                false,
                            );
                        }
                        None => {
                            log::warn!(
                                "[embedded-ble] Windows Bluetooth adapter restarted, but Listener pairing candidate was not visible yet"
                            );
                        }
                    }
                }
                Err(restart_err) => {
                    log::warn!(
                        "[embedded-ble] Windows Bluetooth adapter restart failed after PairAsync failure for {}: {restart_err}",
                        candidate.label
                    );
                }
            }
        }
        final_result
    }

    fn run_default_pairing_once(
        pairing: &DeviceInformationPairing,
        candidate_label: &str,
        label: &str,
    ) -> Result<DevicePairingResultStatus, String> {
        let operation = pairing
            .PairAsync()
            .map_err(|err| format!("Windows default pairing operation failed to start: {err}"))?;
        let pair = wait_async_operation(operation, BLE_PAIRING_PROMPT_TIMEOUT, label)?;
        let status = pair
            .Status()
            .map_err(|err| format!("Windows pairing status read failed: {err}"))?;
        log::warn!(
            "[embedded-ble] Windows {label} returned status={status:?} for {candidate_label}"
        );
        Ok(status)
    }

    fn pairing_status_should_retry_after_settle(status: DevicePairingResultStatus) -> bool {
        matches!(
            status,
            DevicePairingResultStatus::NotReadyToPair
                | DevicePairingResultStatus::OperationAlreadyInProgress
        )
    }

    fn pairing_status_should_try_custom_fallback(status: DevicePairingResultStatus) -> bool {
        matches!(
            status,
            DevicePairingResultStatus::RequiredHandlerNotRegistered
                | DevicePairingResultStatus::InvalidCeremonyData
                | DevicePairingResultStatus::Failed
        )
    }

    fn pairing_status_suggests_adapter_restart(status: DevicePairingResultStatus) -> bool {
        matches!(status, DevicePairingResultStatus::Failed)
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
            .PairAsync(supported_pairing_kinds)
            .map_err(|err| format!("Windows custom pairing operation failed to start: {err}"))
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
        log::warn!(
            "[embedded-ble] Windows custom pairing returned status={status:?} for {candidate_label}"
        );
        pairing_status_or_reachable(
            status,
            expected_name,
            &format!("Windows custom pairing returned status={status:?}"),
        )
    }

    fn pairing_kind_accepts_without_user(kind: DevicePairingKinds) -> bool {
        kind == DevicePairingKinds::ConfirmOnly || kind == DevicePairingKinds::ConfirmPinMatch
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

        if let Some(address) = stale_address {
            match pairing_device_information_from_bluetooth_address_handle(
                address,
                "refreshed BLE pairing device query",
            )? {
                Some(info) => {
                    let name = info
                        .Name()
                        .map(|value| value.to_string_lossy())
                        .unwrap_or_default();
                    let label = listener_pairing_candidate_label(&name, Some(address));
                    return Ok(Some(ListenerPairingCandidate {
                        label,
                        info,
                        address: Some(address),
                        fresh_pairing_advertisement: stale_candidate.fresh_pairing_advertisement,
                    }));
                }
                None => {
                    log::info!(
                        "[embedded-ble] direct BLE pairing refresh found no DeviceInformation for {}; falling back to Windows selector query",
                        crate::embedded_ble::format_bluetooth_address(address)
                    );
                }
            }
        }

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
        Ok(None)
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
        let mut addresses = Vec::new();
        if let Some(address) = configured_bluetooth_address() {
            push_unique_address(&mut addresses, address);
        }
        match listener_pnp_service_signature_addresses() {
            Ok(pnp_addresses) => {
                for address in pnp_addresses {
                    push_unique_address(&mut addresses, address);
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] Listener PnP service-signature address discovery failed: {err}"
                );
            }
        }
        addresses
    }

    fn listener_pnp_service_signature_addresses() -> Result<Vec<u64>, String> {
        let mut addresses = Vec::new();
        let devices = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::All)
            .map_err(|err| format!("Windows PnP device query failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "PnP device query"))?;
        let count = devices
            .Size()
            .map_err(|err| format!("Windows PnP device collection size failed: {err}"))?;
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
                if entry.has_listener_service_signature {
                    if let Some(address) = entry.address {
                        push_unique_address(&mut addresses, address);
                    }
                }
            }
        }
        match powershell_listener_pnp_entries() {
            Ok(entries) => {
                for entry in entries {
                    if entry.has_listener_service_signature {
                        if let Some(address) = entry.address {
                            push_unique_address(&mut addresses, address);
                        }
                    }
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] PowerShell Listener PnP service-signature address fallback failed: {err}"
                );
            }
        }
        if !addresses.is_empty() {
            let labels = addresses
                .iter()
                .copied()
                .map(crate::embedded_ble::format_bluetooth_address)
                .collect::<Vec<_>>();
            log::info!("[embedded-ble] Listener PnP service-signature addresses: {labels:?}");
        }
        Ok(addresses)
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
        for target_name in listener_recovery_advertisement_scan_names(target_names) {
            if target_name.trim().is_empty()
                || scanned_names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&target_name))
            {
                continue;
            }
            scanned_names.push(target_name.clone());
            match scan_listener_pairing_advertisements(Some(&target_name)) {
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

    fn listener_recovery_advertisement_scan_names(target_names: &[String]) -> Vec<String> {
        let mut scan_names = Vec::new();
        if let Some(name) = configured_bluetooth_target_name() {
            push_unique_target_name(&mut scan_names, &name);
        }
        for target_name in target_names.iter().rev() {
            push_unique_target_name(&mut scan_names, target_name);
        }
        for target_name in target_names {
            push_unique_target_name(&mut scan_names, target_name);
        }
        scan_names
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
        let output = run_hidden_pwsh_script(script, "Get-PnpDevice Listener PnP enumeration")?;

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

    fn restart_windows_bluetooth_adapter_after_pairing_failure(
        candidate_label: &str,
    ) -> Result<String, String> {
        log::warn!(
            "[embedded-ble] restarting local Windows Bluetooth adapter once after PairAsync Failed(19) for {candidate_label}"
        );
        let script = r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$adapter = Get-PnpDevice -Class Bluetooth -ErrorAction Stop |
  Where-Object {
    $_.InstanceId -match '^(USB|PCI|ACPI)\\' -and
    $_.FriendlyName -notmatch '枚举器|Enumerator|RFCOMM|LE Enumerator' -and
    $_.FriendlyName -match 'Bluetooth|蓝牙|Realtek|Intel|Qualcomm|MediaTek|Adapter|Wireless'
  } |
  Sort-Object @{ Expression = { if ($_.Status -eq 'OK') { 0 } else { 1 } } }, FriendlyName |
  Select-Object -First 1
if ($null -eq $adapter) {
  throw 'No physical Windows Bluetooth adapter was found'
}
Disable-PnpDevice -InstanceId $adapter.InstanceId -Confirm:$false -ErrorAction Stop | Out-Null
Start-Sleep -Milliseconds 2500
Enable-PnpDevice -InstanceId $adapter.InstanceId -Confirm:$false -ErrorAction Stop | Out-Null
Start-Sleep -Milliseconds 4000
$after = Get-PnpDevice -InstanceId $adapter.InstanceId -ErrorAction Stop
[PSCustomObject]@{
  friendlyName = $adapter.FriendlyName
  instanceId = $adapter.InstanceId
  status = $after.Status
} | ConvertTo-Json -Compress
"#;
        let output = run_hidden_pwsh_script(script, "restart Windows Bluetooth adapter")?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            Ok("Windows Bluetooth adapter restarted".to_string())
        } else {
            Ok(stdout)
        }
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

        if matches!(cm_outcome, Ok(DeviceUnpairOutcome::Unpaired)) {
            return cm_outcome;
        }

        match remove_pnp_device_candidate_with_pnputil(candidate) {
            Ok(DeviceUnpairOutcome::Unpaired) => Ok(DeviceUnpairOutcome::Unpaired),
            Ok(DeviceUnpairOutcome::AlreadyUnpaired) => cm_outcome,
            Err(pnputil_err) => {
                log::warn!(
                    "[embedded-ble] pnputil stale-node cleanup failed for {}: {pnputil_err}",
                    candidate.label
                );
                match cm_outcome {
                    Ok(DeviceUnpairOutcome::AlreadyUnpaired) => Err(format!(
                        "cfgmgr32 could not locate stale node and pnputil failed: {pnputil_err}"
                    )),
                    Err(cm_err) => Err(format!(
                        "cfgmgr32 failed: {cm_err}; pnputil failed: {pnputil_err}"
                    )),
                    Ok(DeviceUnpairOutcome::Unpaired) => Ok(DeviceUnpairOutcome::Unpaired),
                }
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
        let mut locate = unsafe {
            CM_Locate_DevNodeW(
                &mut devinst,
                PCWSTR(wide_id.as_ptr()),
                CM_LOCATE_DEVNODE_NORMAL,
            )
        };
        if locate == CR_NO_SUCH_DEVINST || locate == CR_NO_SUCH_DEVNODE {
            locate = unsafe {
                CM_Locate_DevNodeW(
                    &mut devinst,
                    PCWSTR(wide_id.as_ptr()),
                    CM_LOCATE_DEVNODE_PHANTOM,
                )
            };
        }
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
            .args(["/remove-device", &candidate.instance_id, "/subtree"])
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

    fn hidden_pwsh_command() -> Command {
        hidden_command("pwsh")
    }

    fn run_hidden_pwsh_script(script: &str, label: &str) -> Result<Output, String> {
        let output = hidden_pwsh_command()
            .args(["-NoProfile", "-Command", script])
            .output()
            .map_err(|err| format!("start pwsh for {label} failed: {err}"))?;
        if output.status.success() {
            return Ok(output);
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("pwsh for {label} exited with status {}", output.status)
        } else {
            format!(
                "pwsh for {label} exited with status {}: {stderr}",
                output.status
            )
        })
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
        let status =
            write_cccd_notify_with_retry(0, "diagnostic log", &data, CCCD_ENABLE_TIMEOUT, None)?;
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
        let recovery_probe_address = cleanup.target.bluetooth_address;
        let status = write_cccd_notify_with_retry(
            capture_id,
            "probe",
            &characteristic,
            notify_timeout,
            recovery_probe_address,
        )?;
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

    fn processing_state_active_transient_fallback(active: bool) -> ActiveControlTransientFallback {
        let _ = active;
        ActiveControlTransientFallback::ReturnError
    }

    fn bounded_recording_stop_active_timeout(timeout: Duration) -> Duration {
        if timeout > RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT {
            RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT
        } else {
            timeout
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

    fn send_processing_hint_control_command(
        command: &[u8],
        serial_command: &'static str,
        timeout: Duration,
        label: &'static str,
    ) -> Result<(), String> {
        let mut active_error = None;
        if let Some(result) = send_audio_control_via_active_capture(command, timeout, label) {
            match result {
                Ok(()) => {
                    log::info!("[embedded-ble] {label} sent via active capture");
                    return Ok(());
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] active {label} write failed; trying USB serial processing hint fallback: {err}"
                    );
                    active_error = Some(err);
                }
            }
        }

        match send_control_command_via_usb_serial(serial_command, timeout) {
            Ok(()) => {
                log::info!("[embedded-ble] {label} sent via USB serial fallback");
                Ok(())
            }
            Err(err) => {
                let active_part = active_error
                    .map(|err| format!("active BLE failed: {err}; "))
                    .unwrap_or_default();
                Err(format!("{active_part}USB serial fallback failed: {err}"))
            }
        }
    }

    fn send_recording_stop_control_command(timeout: Duration) -> Result<(), String> {
        let command = b"VREC:STOP\n";
        let label = "audio control stop";
        let active_timeout = bounded_recording_stop_active_timeout(timeout);
        let mut active_error = None;
        if let Some(result) = send_audio_control_via_active_capture(command, active_timeout, label)
        {
            match result {
                Ok(()) => {
                    log::info!(
                        "[embedded-ble] {label} sent via active capture active_timeout_ms={}",
                        active_timeout.as_millis()
                    );
                    return Ok(());
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] active {label} write failed; trying USB serial stop fallback: {err}"
                    );
                    active_error = Some(err);
                }
            }
        }

        match send_control_command_via_usb_serial("VREC:STOP", timeout) {
            Ok(()) => {
                log::info!("[embedded-ble] {label} sent via USB serial fallback");
                Ok(())
            }
            Err(err) => {
                let active_part = active_error
                    .map(|err| format!("active BLE failed: {err}; "))
                    .unwrap_or_default();
                Err(format!("{active_part}USB serial fallback failed: {err}"))
            }
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
        send_recording_stop_control_command(timeout)
    }

    pub fn send_recording_control_recovery(timeout: Duration) -> Result<(), String> {
        let serial_result =
            send_recovery_control_command_via_usb_serial("VREC:RECOVERY:TYPE", timeout);
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
            b"VREC:RECOVERY:TYPE\n",
            timeout,
            "audio control recovery",
            ActiveControlTransientFallback::TryFreshGatt,
        )
    }

    pub fn send_recording_control_silent_recovery(timeout: Duration) -> Result<(), String> {
        let serial_result =
            send_recovery_control_command_via_usb_serial("VREC:RECOVERY:TYPE:SILENT", timeout);
        match &serial_result {
            Ok(()) => {
                log::info!("[embedded-ble] audio control silent recovery sent via USB serial");
                return Ok(());
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] audio control silent recovery USB serial path unavailable; trying BLE control: {err}"
                );
            }
        }
        send_recording_control_command(
            b"VREC:RECOVERY:TYPE:SILENT\n",
            timeout,
            "audio control silent recovery",
            ActiveControlTransientFallback::TryFreshGatt,
        )
    }

    pub fn send_recording_control_type_bye(timeout: Duration) -> Result<(), String> {
        if let Some(result) =
            send_audio_control_via_active_capture(b"TYPE:BYE\n", timeout, "audio type bye")
        {
            return result;
        }
        Err("active Listener BLE audio control unavailable for shutdown bye".to_string())
    }

    pub fn send_recording_processing_state(active: bool, timeout: Duration) -> Result<(), String> {
        let command = if active {
            b"VREC:PROCESSING:START\n".as_slice()
        } else {
            b"VREC:PROCESSING:STOP\n".as_slice()
        };
        let serial_command = if active {
            "VREC:PROCESSING:START"
        } else {
            "VREC:PROCESSING:STOP"
        };
        let label = if active {
            "audio processing start"
        } else {
            "audio processing stop"
        };
        send_processing_hint_control_command(command, serial_command, timeout, label)
    }

    pub fn send_recording_processing_done(timeout: Duration) -> Result<(), String> {
        send_processing_hint_control_command(
            b"VREC:PROCESSING:DONE\n",
            "VREC:PROCESSING:DONE",
            timeout,
            "audio processing done",
        )
    }

    pub fn send_recording_processing_warning(timeout: Duration) -> Result<(), String> {
        send_processing_hint_control_command(
            b"VREC:PROCESSING:WARN\n",
            "VREC:PROCESSING:WARN",
            timeout,
            "audio processing warning",
        )
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

    fn device_settings_payload(command: &str) -> Result<String, String> {
        if !command.starts_with("DEVICE:") {
            return Err("device settings command must start with DEVICE:".to_string());
        }
        if command.contains('\r') || command.contains('\n') {
            return Err("device settings command must be a single line".to_string());
        }
        Ok(format!("{command}\n"))
    }

    pub fn send_device_settings_command_via_active_capture_only(
        command: &str,
        timeout: Duration,
        label: &str,
    ) -> Result<(), String> {
        let payload = device_settings_payload(command)?;
        let payload_len = payload.as_bytes().len();
        if payload_len >= 64 || !device_settings_command_allows_active_capture(command) {
            return Err(format!(
                "{label} command is not eligible for active Listener BLE audio control"
            ));
        }
        match send_audio_control_via_active_capture(payload.as_bytes(), timeout, label) {
            Some(result) => result,
            None => Err(format!(
                "{label} active Listener BLE audio control is not ready"
            )),
        }
    }

    fn send_device_settings_command_with_transport(
        command: &str,
        timeout: Duration,
    ) -> Result<DeviceSettingsCommandTransport, String> {
        let payload = device_settings_payload(command)?;
        let payload_len = payload.as_bytes().len();
        let mut active_capture_error: Option<String> = None;
        let ble_name_target = device_settings_command_ble_name_target(command);
        if let Some(target_name) = ble_name_target.as_deref() {
            let _ = remember_current_bluetooth_target_address_for_name(
                target_name,
                timeout,
                "device settings BLE rename",
            );
        }

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
                        return Ok(DeviceSettingsCommandTransport::ActiveCapture);
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
                return Ok(DeviceSettingsCommandTransport::UsbSerial);
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
        Ok(DeviceSettingsCommandTransport::FreshGatt)
    }

    pub fn send_device_settings_command(command: &str, timeout: Duration) -> Result<(), String> {
        send_device_settings_command_with_transport(command, timeout).map(|_| ())
    }

    pub fn apply_pending_ble_name(
        timeout: Duration,
    ) -> Result<DeviceSettingsCommandTransport, String> {
        if !has_active_runtime_bluetooth_target_address(Instant::now()) {
            let target_name = effective_bluetooth_target_name(None);
            let _ = remember_current_bluetooth_target_address_for_name(
                &target_name,
                timeout,
                "device settings BLE name apply",
            );
        }
        send_device_settings_command_with_transport("DEVICE:APPLY_BLE_NAME", timeout)
    }

    pub fn send_status_led_command(command: &str, timeout: Duration) -> Result<(), String> {
        let command = command.strip_prefix('~').unwrap_or(command);
        if !command.starts_with("LED:") {
            return Err("status LED command must start with LED:".to_string());
        }
        if command.contains('\r') || command.contains('\n') {
            return Err("status LED command must be a single line".to_string());
        }

        let payload = format!("{command}\n");
        let payload_len = payload.as_bytes().len();
        let mut active_capture_error: Option<String> = None;

        if payload_len < 64 {
            if let Some(result) =
                send_audio_control_via_active_capture(payload.as_bytes(), timeout, "status LED")
            {
                match result {
                    Ok(()) => {
                        log::info!("[embedded-ble] status LED command sent via active capture");
                        return Ok(());
                    }
                    Err(err) => {
                        log::warn!(
                            "[embedded-ble] active status LED write failed; trying USB serial/fresh GATT fallback: {err}"
                        );
                        active_capture_error = Some(err);
                    }
                }
            }
        }

        let serial_result = send_control_command_via_usb_serial(command, timeout);
        match &serial_result {
            Ok(()) => {
                log::info!("[embedded-ble] status LED command acknowledged via USB serial");
                return Ok(());
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] status LED USB serial path unavailable; trying BLE control: {err}"
                );
            }
        }

        if payload_len >= 64 {
            return Err(format!(
                "status LED command is too long for BLE control characteristic: {} bytes (max 63 including newline); USB serial fallback failed: {}",
                payload_len,
                serial_result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "not attempted".to_string())
            ));
        }

        let _fresh_guard = BleFreshGattGuard::enter("status LED")?;
        let target = open_audio_control_target_with_retry("status LED").map_err(|err| {
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
        write_audio_control_value_with_timeout(
            &target.control,
            payload.as_bytes(),
            timeout,
            "status LED",
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
        log::info!("[embedded-ble] status LED command sent");
        Ok(())
    }

    #[cfg(test)]
    pub fn read_status_led_brightness_status(timeout: Duration) -> Result<String, String> {
        exchange_status_led_via_usb_serial(
            "LED:STATUS detail=brightness",
            "~LED:STATUS detail=brightness",
            timeout,
        )
        .map_err(|err| format!("USB serial status LED brightness refresh failed: {err}"))
    }

    fn device_settings_command_allows_active_capture(command: &str) -> bool {
        !device_settings_command_updates_ble_name(command)
    }

    fn device_settings_command_updates_ble_name(command: &str) -> bool {
        let Some(arguments) = command.strip_prefix("DEVICE:SET ") else {
            return false;
        };
        arguments
            .split_ascii_whitespace()
            .any(device_settings_token_updates_ble_name)
    }

    fn device_settings_command_ble_name_target(command: &str) -> Option<String> {
        let arguments = command.strip_prefix("DEVICE:SET ")?;
        arguments
            .split_ascii_whitespace()
            .find_map(|token| {
                token
                    .strip_prefix("ble_name=")
                    .or_else(|| token.strip_prefix("name="))
            })
            .and_then(normalize_bluetooth_target_name)
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

    fn send_recovery_control_command_via_usb_serial(
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
            match send_recovery_control_command_via_serial_port(&port.port_name, command, timeout) {
                Ok(()) => return Ok(()),
                Err(err) => errors.push(format!("{}: {err}", port.port_name)),
            }
        }

        Err(DeviceSettingsSerialError::Transport(format!(
            "all Listener USB serial candidates failed: {}",
            errors.join("; ")
        )))
    }

    #[cfg(test)]
    fn exchange_status_led_via_usb_serial(
        command: &str,
        expected_fragment: &str,
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
            match exchange_status_led_via_serial_port(
                &port.port_name,
                command,
                expected_fragment,
                timeout,
            ) {
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

        drain_serial_input_until_quiet(
            &mut *port,
            DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION,
            DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION,
        );
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

        drain_serial_input_until_quiet(
            &mut *port,
            DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION,
            DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION,
        );
        let payload = format!("~{command}\n");
        port.write_all(payload.as_bytes())
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("write failed: {err}")))?;
        port.flush()
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("flush failed: {err}")))?;
        std::thread::sleep(timeout.min(Duration::from_millis(500)));
        Ok(())
    }

    fn send_recovery_control_command_via_serial_port(
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

        // Recovery commands are write-only. Draining diagnostics cannot validate
        // one, and delaying it shortens the firmware's pairing recovery window.
        let payload = format!("~{command}\n");
        port.write_all(payload.as_bytes())
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("write failed: {err}")))?;
        port.flush()
            .map_err(|err| DeviceSettingsSerialError::Transport(format!("flush failed: {err}")))?;
        std::thread::sleep(timeout.min(Duration::from_millis(500)));
        Ok(())
    }

    #[cfg(test)]
    fn exchange_status_led_via_serial_port(
        port_name: &str,
        command: &str,
        expected_fragment: &str,
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

        drain_serial_input_until_quiet(
            &mut *port,
            DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION,
            DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION,
        );
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
                    if let Some(line) = response.lines().find(|line| line.contains("~LED:ERROR")) {
                        return Err(DeviceSettingsSerialError::FirmwareRejected(format!(
                            "firmware rejected status LED command: {line}"
                        )));
                    }
                    if let Some(line) = complete_status_led_line(&response, expected_fragment) {
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
            "timed out waiting for {expected_fragment}; received={tail:?}"
        )))
    }

    fn drain_serial_input_until_quiet(
        port: &mut dyn serialport::SerialPort,
        max_duration: Duration,
        quiet_duration: Duration,
    ) {
        let deadline = Instant::now() + max_duration;
        let mut quiet_since = Instant::now();
        let mut drained_bytes = 0_usize;
        let mut read_buf = [0_u8; DEVICE_SETTINGS_SERIAL_READ_CHUNK_BYTES];
        while Instant::now() < deadline {
            match port.read(&mut read_buf) {
                Ok(0) => {}
                Ok(count) => {
                    drained_bytes = drained_bytes.saturating_add(count);
                    quiet_since = Instant::now();
                }
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {
                    if quiet_since.elapsed() >= quiet_duration {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if drained_bytes > 0 {
            log::debug!(
                "[embedded-ble] drained {drained_bytes} stale USB serial bytes before command"
            );
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

    #[cfg(test)]
    fn complete_status_led_line(response: &str, expected_fragment: &str) -> Option<String> {
        let mut lines: Vec<&str> = response.split('\n').collect();
        if !response.ends_with('\n') {
            let _ = lines.pop();
        }
        lines
            .into_iter()
            .map(str::trim)
            .find(|line| line.contains(expected_fragment))
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
        let default_brightness = crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT;
        let plugged_brightness_percent =
            optional_u8_field(&fields, "plugged_brightness").unwrap_or(default_brightness);
        let battery_brightness_percent =
            optional_u8_field(&fields, "battery_brightness").unwrap_or(plugged_brightness_percent);
        let brightness_percent =
            optional_u8_field(&fields, "active_brightness").unwrap_or_else(|| {
                if require_field(&fields, "active_power").unwrap_or("battery") == "external" {
                    plugged_brightness_percent
                } else {
                    battery_brightness_percent
                }
            });
        let led_zone_brightness_supported = fields.contains_key("led_status")
            || fields.contains_key("led_key")
            || fields.contains_key("led_ec11")
            || fields.contains_key("led_edge");
        let compact_set_supported = optional_bool_field(&fields, "compact_set").unwrap_or(false);
        let default_status_brightness = crate::types::DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT;
        let default_key_brightness = crate::types::DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT;
        let default_zone_brightness = crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT;
        let status_led_brightness_percent =
            optional_u8_field(&fields, "led_status").unwrap_or(default_status_brightness);
        let key_led_brightness_percent =
            optional_u8_field(&fields, "led_key").unwrap_or(default_key_brightness);
        let knob_led_brightness_percent =
            optional_u8_field(&fields, "led_ec11").unwrap_or(default_zone_brightness);
        let edge_led_brightness_percent =
            optional_u8_field(&fields, "led_edge").unwrap_or(default_zone_brightness);
        let legacy_low_power_idle_minutes =
            optional_u32_field(&fields, "low_power_idle_ms").map(low_power_minutes_from_ms);
        let plugged_low_power_idle_minutes =
            optional_u32_field(&fields, "plugged_low_power_idle_ms")
                .or_else(|| optional_minutes_field(&fields, "plugged_low_power_idle_minutes"))
                .map(low_power_minutes_from_ms)
                .unwrap_or_else(|| {
                    legacy_low_power_idle_minutes
                        .unwrap_or(crate::types::DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES)
                });
        let battery_low_power_idle_minutes =
            optional_u32_field(&fields, "battery_low_power_idle_ms")
                .or_else(|| optional_minutes_field(&fields, "battery_low_power_idle_minutes"))
                .map(low_power_minutes_from_ms)
                .unwrap_or_else(|| {
                    legacy_low_power_idle_minutes
                        .unwrap_or(crate::types::DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES)
                });
        let plugged_low_power_enabled = optional_bool_field(&fields, "plugged_low_power_enabled")
            .unwrap_or_else(|| {
                if legacy_low_power_idle_minutes.is_some()
                    || fields.contains_key("plugged_low_power_idle_ms")
                    || fields.contains_key("plugged_low_power_idle_minutes")
                    || fields.contains_key("battery_low_power_idle_ms")
                    || fields.contains_key("battery_low_power_idle_minutes")
                {
                    false
                } else {
                    crate::types::DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED
                }
            });
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
            brightness_percent,
            plugged_brightness_percent,
            battery_brightness_percent,
            status_led_brightness_percent,
            key_led_brightness_percent,
            knob_led_brightness_percent,
            edge_led_brightness_percent,
            led_zone_brightness_supported,
            compact_set_supported,
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
        minutes_from_ms_floor(ms, crate::types::MAX_DEVICE_LOW_POWER_IDLE_MINUTES)
    }

    fn auto_shutdown_minutes_from_ms(ms: u32) -> u32 {
        minutes_from_ms_floor(ms, crate::types::MAX_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES)
    }

    fn minutes_from_ms_floor(ms: u32, max_minutes: u32) -> u32 {
        if ms == 0 {
            0
        } else {
            ms.checked_div(60_000).unwrap_or(0).clamp(1, max_minutes)
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
            queued_at: Instant::now(),
            result_tx,
        };
        if active.tx.send(request).is_err() {
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
            None,
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
            None,
        )
    }

    pub fn capture_notification_events_continuous_with_connection_handoff_until_cancelled(
        idle_timeout: Option<Duration>,
        cancel_requested: Arc<AtomicBool>,
        leave_notify_cccd_enabled_on_cancel: Arc<AtomicBool>,
        on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
        on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
    ) -> Result<(), String> {
        capture_notification_events_until_cancelled_impl(
            idle_timeout,
            cancel_requested,
            on_ready,
            on_event,
            CaptureTerminalBehavior::ContinueListening,
            Some(leave_notify_cccd_enabled_on_cancel),
        )
    }

    fn capture_notification_events_until_cancelled_impl(
        idle_timeout: Option<Duration>,
        cancel_requested: Arc<AtomicBool>,
        on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
        on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
        terminal_behavior: CaptureTerminalBehavior,
        leave_notify_cccd_enabled_on_cancel: Option<Arc<AtomicBool>>,
    ) -> Result<(), String> {
        let _cancel_scope = NotifyCaptureCancelScope::install(&cancel_requested);
        if notify_capture_cancel_requested() {
            return Err(notify_capture_cancelled_error("notify capture open"));
        }
        let capture_guard = BleCaptureGuard::enter(idle_timeout)?;
        let capture_id = capture_guard.session_id();
        if terminal_behavior == CaptureTerminalBehavior::ContinueListening
            && ble_ota_process_mutex_busy()
        {
            log::info!(
                "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; deferring background listener before notify open"
            );
            return Err(background_listener_deferred_for_ota_error());
        }
        let target = open_notify_target_with_retry(capture_id)?;
        let characteristic = target.characteristic.clone();
        let (tx, rx) = mpsc::channel::<BleCaptureSignal>();
        let (control_tx, control_rx) = mpsc::channel::<AudioControlRequest>();
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
                capture_id, control_tx,
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

        if notify_cccd_prereset_required(terminal_behavior) {
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
                    if let Some(recovery_error) =
                        cccd_notify_recovery_pairing_error(&err, cleanup.target.bluetooth_address)
                    {
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: notify CCCD reset hit recovery pairing window; entering Type PairAsync recovery before notify-enable retries: {err}"
                        );
                        return Err(recovery_error);
                    }
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: notify CCCD reset skipped: {err}"
                    )
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        } else {
            log::info!(
                "[embedded-ble] capture #{capture_id}: continuous listener enables notify CCCD without pre-reset"
            );
        }
        log::info!("[embedded-ble] capture #{capture_id}: enabling notify CCCD");
        let status = write_cccd_notify_with_retry(
            capture_id,
            "capture",
            &characteristic,
            CCCD_ENABLE_TIMEOUT,
            cleanup.target.bluetooth_address,
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
        if type_heartbeat_enabled && ble_ota_process_mutex_busy() {
            log::info!(
                "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; closing background listener before Type heartbeat ready"
            );
            cleanup.disable_notify();
            return Err(background_listener_deferred_for_ota_error());
        }
        match cleanup.write_type_heartbeat(&type_ready_command, "Type heartbeat ready") {
            Ok(()) => {
                cleanup.mark_type_heartbeat_open();
                if let Some(address) = cleanup.target.bluetooth_address {
                    persist_successful_notify_target_address(address, "Type heartbeat ready");
                }
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
            cleanup.drain_audio_control_requests(&control_rx);
            let now = Instant::now();
            if type_heartbeat_enabled
                && !collector_has_active_recoverable_session(&collector)
                && ble_ota_process_mutex_busy()
            {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; closing idle background listener"
                );
                cleanup.disable_notify();
                return Err(background_listener_deferred_for_ota_error());
            }
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
                cleanup.finish_after_caller_cancel(
                    terminal_behavior,
                    leave_notify_cccd_enabled_on_cancel
                        .as_ref()
                        .is_some_and(|handoff| handoff.load(Ordering::SeqCst)),
                );
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
                        cleanup.finish_after_caller_cancel(
                            terminal_behavior,
                            leave_notify_cccd_enabled_on_cancel
                                .as_ref()
                                .is_some_and(|handoff| handoff.load(Ordering::SeqCst)),
                        );
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
                        if let Some(recovery_error) =
                            active_capture_disconnect_recovery_pairing_error(&reason)
                        {
                            let stats = collector.stats();
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: recovery advertising proves the active session cannot resume; entering Type PairAsync recovery without the active-session wait (session_id={:?}, packets={})",
                                stats.session_id,
                                stats.received_packet_count,
                            );
                            cleanup.disable_notify();
                            return Err(recovery_error);
                        }
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

    struct OpenListenerOtaV1Target {
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

    struct PreparedListenerOtaV1Characteristics {
        control: GattCharacteristic,
        data: GattCharacteristic,
        status: GattCharacteristic,
        data_write_option: GattWriteOption,
        data_chunk_payload_bytes: usize,
        session: Option<GattSession>,
    }

    pub(super) struct PreparedListenerOtaV1Transfer {
        target: OpenListenerOtaV1Target,
        snapshot: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
        transfer_guard: BleCaptureGuard,
        _ota_process_guard: BleOtaProcessGuard,
    }

    impl PreparedListenerOtaV1Transfer {
        pub(super) fn snapshot(&self) -> &crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            &self.snapshot
        }

        pub(super) fn transfer(
            self,
            _firmware_sha256: &str,
            firmware_bytes: &[u8],
            manifest_chunk_bytes: usize,
            on_progress: Option<&dyn Fn(usize, usize)>,
        ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
            transfer_denzic_ota_v1_to_target(
                &self.target,
                self.transfer_guard.session_id(),
                firmware_bytes,
                manifest_chunk_bytes,
                on_progress,
            )
        }
    }

    struct ListenerOtaV1Transport<'a> {
        target: &'a OpenListenerOtaV1Target,
        transfer_id: u64,
        data_write_option: GattWriteOption,
    }

    impl denzic_ota_core::OtaV1Transport for ListenerOtaV1Transport<'_> {
        fn write_control(
            &mut self,
            packet: &[u8; denzic_ota_core::CONTROL_BYTES],
        ) -> Result<(), String> {
            let label = match packet[4] {
                denzic_ota_core::OP_BEGIN => "Denzic OTA v1 begin",
                denzic_ota_core::OP_SYNC => "Denzic OTA v1 sync",
                denzic_ota_core::OP_ABORT => "Denzic OTA v1 abort",
                _ => "Denzic OTA v1 control",
            };
            write_gatt_value_with_timeout(
                &self.target.control,
                packet,
                GattWriteOption::WriteWithResponse,
                OTA_WRITE_TIMEOUT,
                label,
            )
            .map(|_| ())
        }

        fn write_data(&mut self, packet: &[u8]) -> Result<(), String> {
            self.data_write_option = write_listener_ota_v1_value_with_fallback(
                &self.target.data,
                packet,
                self.data_write_option,
                OTA_WRITE_TIMEOUT,
                "Denzic OTA v1 data",
            )?;
            Ok(())
        }

        fn read_status(&mut self) -> Result<Vec<u8>, String> {
            read_characteristic_bytes_with_timeout(
                &self.target.status,
                BluetoothCacheMode::Uncached,
                "Denzic OTA v1 status",
                LISTENER_OTA_V1_STATUS_READ_TIMEOUT,
            )
        }

        fn status_retry_wait(&mut self, attempt: u8) {
            log::warn!(
                "[embedded-ble] Denzic OTA v1 #{}: retrying transient status read (attempt={})",
                self.transfer_id,
                attempt + 1
            );
            std::thread::sleep(LISTENER_OTA_V1_STATUS_POLL_INTERVAL);
        }

        fn write_finish(
            &mut self,
            packet: &[u8; denzic_ota_core::CONTROL_BYTES],
        ) -> Result<(), String> {
            match write_gatt_value_with_timeout(
                &self.target.control,
                packet,
                GattWriteOption::WriteWithResponse,
                OTA_FINISH_WRITE_TIMEOUT,
                "Denzic OTA v1 finish",
            ) {
                Ok(_) => Ok(()),
                Err(error) if is_ota_finish_reboot_handoff_error(&error) => {
                    log::info!(
                        "[embedded-ble] Denzic OTA v1 #{}: finish completed through reboot handoff: {}",
                        self.transfer_id,
                        error
                    );
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
    }

    fn transfer_denzic_ota_v1_to_target(
        target: &OpenListenerOtaV1Target,
        transfer_id: u64,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if manifest_chunk_bytes != LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES {
            return Err(format!(
                "Denzic OTA v1 manifest chunk size must be {LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES} bytes, got {manifest_chunk_bytes}."
            ));
        }
        let configured_window = listener_ota_v1_window_chunks()?;
        let chunk_payload_bytes = manifest_chunk_bytes
            .min(target.data_chunk_payload_bytes)
            .min(u16::MAX as usize)
            .max(1) as u16;
        let window_chunks = configured_window.min(u16::MAX as usize).max(1) as u16;
        let mut transport = ListenerOtaV1Transport {
            target,
            transfer_id,
            data_write_option: target.data_write_option,
        };

        let started_at = Instant::now();
        let report = denzic_ota_core::transfer(
            &mut transport,
            firmware_bytes,
            denzic_ota_core::TransferOptions {
                chunk_payload_bytes,
                window_chunks,
                status_read_attempts: 5,
                max_stalled_windows: 3,
                inactive_link_window_chunks: Some(
                    LISTENER_OTA_V1_INACTIVE_LINK_WINDOW_CHUNKS as u16,
                ),
            },
            |completed, total| {
                if let Some(callback) = on_progress {
                    callback(completed, total);
                }
            },
        )?;
        log::info!(
            "[embedded-ble] Denzic OTA v1 #{transfer_id}: transferred {}/{} bytes in {} data writes, {} status reads, {} offset recoveries, active_link_confirmed={}, elapsed_ms={}",
            report.firmware_bytes,
            firmware_bytes.len(),
            report.data_writes,
            report.status_reads,
            report.recovered_offsets,
            report.active_link_confirmed,
            started_at.elapsed().as_millis()
        );
        Ok(crate::embedded_ble::FirmwareOtaTransferStats {
            bytes_transferred: report.firmware_bytes,
            chunks_sent: report.data_writes as usize,
            transport: denzic_ota_core::PROTOCOL_NAME,
        })
    }

    pub(super) fn request_listener_ota_v1_active_link() -> Result<(), String> {
        send_recording_control_command(
            b"TYPE:OTA\n",
            Duration::from_secs(3),
            "Listener OTA v1 reconnect handoff",
            ActiveControlTransientFallback::ReturnError,
        )
    }

    pub(super) fn prepare_listener_ota_v1_transfer() -> Result<PreparedListenerOtaV1Transfer, String>
    {
        prepare_listener_ota_v1_transfer_impl(true)
    }

    fn prepare_listener_ota_v1_transfer_after_active_link_hint(
    ) -> Result<PreparedListenerOtaV1Transfer, String> {
        prepare_listener_ota_v1_transfer_impl(false)
    }

    fn prepare_listener_ota_v1_transfer_impl(
        send_active_link_hint: bool,
    ) -> Result<PreparedListenerOtaV1Transfer, String> {
        let ota_process_guard = acquire_ble_ota_process_mutex("listener_ota_v1")?;
        if send_active_link_hint {
            request_listener_ota_v1_active_link()?;
            log::info!("[embedded-ble] Listener OTA v1 reconnect handoff accepted");
        } else {
            log::info!(
                "[embedded-ble] Listener OTA v1 prepare: reusing reconnect handoff sent before background pause"
            );
        }
        log::info!("[embedded-ble] Listener OTA v1 prepare: acquiring BLE capture guard");
        let transfer_guard = BleCaptureGuard::enter(None)?;
        log::info!(
            "[embedded-ble] Listener OTA v1 prepare: waiting {} ms for reconnect handoff",
            LISTENER_OTA_V1_RECONNECT_SETTLE.as_millis()
        );
        std::thread::sleep(LISTENER_OTA_V1_RECONNECT_SETTLE);
        let _fresh_guard = BleFreshGattGuard::enter("Listener OTA v1 prepare")?;
        log::info!("[embedded-ble] Listener OTA v1 prepare: discovering Listener OTA v1 service");
        let target = open_listener_ota_v1_target()?;
        let snapshot = listener_ota_v1_gatt_probe_snapshot_from_target(&target);
        log::info!(
            "[embedded-ble] Listener OTA v1 prepare: ready (detail={})",
            snapshot.detail.as_deref().unwrap_or("unknown")
        );
        Ok(PreparedListenerOtaV1Transfer {
            target,
            snapshot,
            transfer_guard,
            _ota_process_guard: ota_process_guard,
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

    pub fn transfer_listener_ota_v1(
        firmware_sha256: &str,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let prepared = prepare_listener_ota_v1_transfer()?;
        prepared.transfer(
            firmware_sha256,
            firmware_bytes,
            manifest_chunk_bytes,
            on_progress,
        )
    }

    pub fn transfer_listener_ota_v1_after_active_link_hint(
        firmware_sha256: &str,
        firmware_bytes: &[u8],
        manifest_chunk_bytes: usize,
        on_progress: Option<&dyn Fn(usize, usize)>,
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let prepared = prepare_listener_ota_v1_transfer_after_active_link_hint()?;
        prepared.transfer(
            firmware_sha256,
            firmware_bytes,
            manifest_chunk_bytes,
            on_progress,
        )
    }

    fn listener_ota_v1_window_chunks() -> Result<usize, String> {
        let configured = std::env::var(LISTENER_OTA_V1_WINDOW_ENV)
            .ok()
            .and_then(ota_env_value);
        match configured.as_deref() {
            None => Ok(LISTENER_OTA_V1_DEFAULT_WINDOW_CHUNKS),
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|window| (1..=100).contains(window))
                .ok_or_else(|| {
                    format!(
                        "Unsupported {LISTENER_OTA_V1_WINDOW_ENV}={value}; use a window from 1 to 100 chunks."
                    )
                }),
        }
    }

    fn write_listener_ota_v1_value_with_fallback(
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

    pub fn firmware_ota_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        listener_ota_v1_device_snapshot()
    }

    pub fn listener_ota_v1_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let _fresh_guard = match BleFreshGattGuard::enter("Listener OTA v1 snapshot") {
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
        match open_listener_ota_v1_target() {
            Ok(target) => listener_ota_v1_device_snapshot_from_target(&target),
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

    pub fn listener_ota_v1_gatt_probe_snapshot(
        timeout: Duration,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let _fresh_guard = match BleFreshGattGuard::enter("Listener OTA v1 GATT probe") {
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
        let deadline = Instant::now() + timeout.max(Duration::from_millis(1));
        match open_listener_ota_v1_target_with_deadline(deadline) {
            Ok(target) => listener_ota_v1_gatt_probe_snapshot_from_target(&target),
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

    fn listener_ota_v1_device_snapshot_from_target(
        target: &OpenListenerOtaV1Target,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let mut snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: None,
            firmware_version: None,
            capabilities: vec![denzic_ota_core::PROTOCOL_NAME.to_string()],
            battery_percent: None,
            usb_powered: None,
            detail: None,
        };
        // The OTA v1 service itself is the capability proof. DIS metadata is best-effort:
        // read it once when Windows exposes it, but do not make it a hard preflight blocker.
        if !snapshot
            .capabilities
            .iter()
            .any(|item| item == denzic_ota_core::PROTOCOL_NAME)
        {
            snapshot
                .capabilities
                .push(denzic_ota_core::PROTOCOL_NAME.to_string());
        }
        let (dis_model, dis_hardware, dis_firmware, dis_battery) =
            read_dis_metadata_from_discovered_services(target.bluetooth_address);
        snapshot.hardware_revision =
            normalize_optional_hardware_revision(dis_hardware).or(dis_model);
        snapshot.firmware_version = dis_firmware;
        snapshot.battery_percent = dis_battery;
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
        if snapshot.hardware_revision.is_none() && snapshot.firmware_version.is_none() {
            let address = target.bluetooth_address.map(|value| {
                format!(
                    " at {}",
                    crate::embedded_ble::format_bluetooth_address(value)
                )
            });
            snapshot.detail = Some(format!(
                "Listener OTA v1 service is reachable{}, but DIS identity metadata was not exposed in this BLE session.",
                address.as_deref().unwrap_or("")
            ));
        } else {
            snapshot.detail = Some("Listener OTA v1 service is reachable; DIS metadata was read when Windows exposed it.".to_string());
        }
        log::info!(
            "[embedded-ble] Listener OTA v1 snapshot connected={} hardware={:?} firmware={:?} battery={:?} usb_powered={:?} detail={:?}",
            snapshot.connected,
            snapshot.hardware_revision,
            snapshot.firmware_version,
            snapshot.battery_percent,
            snapshot.usb_powered,
            snapshot.detail
        );
        snapshot
    }

    fn listener_ota_v1_gatt_probe_snapshot_from_target(
        target: &OpenListenerOtaV1Target,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        let address = target.bluetooth_address.map(|value| {
            format!(
                " at {}",
                crate::embedded_ble::format_bluetooth_address(value)
            )
        });
        let snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: None,
            firmware_version: None,
            capabilities: vec![denzic_ota_core::PROTOCOL_NAME.to_string()],
            battery_percent: None,
            usb_powered: None,
            detail: Some(format!(
                "Listener OTA v1 service is reachable{}; bounded GATT probe skipped optional DIS metadata.",
                address.as_deref().unwrap_or("")
            )),
        };
        log::info!(
            "[embedded-ble] Listener OTA v1 GATT probe connected={} capabilities={:?} detail={:?}",
            snapshot.connected,
            snapshot.capabilities,
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

    fn remaining_ble_timeout(
        deadline: Instant,
        cap: Duration,
        label: &str,
    ) -> Result<Duration, String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("BLE {label} timed out before operation started"));
        }
        Ok(remaining.min(cap).max(Duration::from_millis(1)))
    }

    fn open_notify_target_for_startup_cached_address(
        address: u64,
    ) -> Result<OpenNotifyTarget, String> {
        let deadline = Instant::now() + STARTUP_NOTIFY_FAST_PATH_TIMEOUT;
        let device = open_ble_device_with_timeout(
            address,
            remaining_ble_timeout(
                deadline,
                STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT,
                "persisted startup device open",
            )?,
        )?;
        if let Ok(operation) = device.RequestAccessAsync() {
            if let Some(access) = wait_async_operation(
                operation,
                remaining_ble_timeout(
                    deadline,
                    STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT,
                    "persisted startup device access",
                )?,
                "persisted startup device access",
            )
            .ok()
            {
                if access != DeviceAccessStatus::Allowed
                    && access != DeviceAccessStatus::Unspecified
                {
                    return Err(format!("BLE device access denied status={access:?}"));
                }
            }
        }

        let services_result = device
            .GetGattServicesForUuidWithCacheModeAsync(SERVICE_UUID, BluetoothCacheMode::Cached)
            .map_err(|err| format!("BLE persisted startup Cached service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    remaining_ble_timeout(
                        deadline,
                        STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT,
                        "persisted startup Cached service",
                    )?,
                    "persisted startup Cached service",
                )
                .map_err(|err| {
                    format!("BLE persisted startup Cached service discovery wait failed: {err}")
                })
            })?;
        let status = services_result.Status().map_err(|err| {
            format!("BLE persisted startup Cached service status read failed: {err}")
        })?;
        if status != GattCommunicationStatus::Success {
            return Err(format!(
                "BLE persisted startup Cached service discovery returned status={status:?}"
            ));
        }

        let services = services_result.Services().map_err(|err| {
            format!("BLE persisted startup Cached service list read failed: {err}")
        })?;
        let count = services.Size().map_err(|err| {
            format!("BLE persisted startup Cached service list size failed: {err}")
        })?;
        if count == 0 {
            return Err(format!(
                "service {SERVICE_UUID:?} not found from persisted startup Cached device"
            ));
        }

        let mut last_error = None;
        for index in 0..count {
            let service = match services.GetAt(index) {
                Ok(service) => service,
                Err(err) => {
                    last_error = Some(format!(
                        "read persisted startup Cached service failed: {err}"
                    ));
                    continue;
                }
            };
            match open_notify_characteristic_from_service_for_startup_fast_path(&service, deadline)
            {
                Ok(prepared) => {
                    return Ok(OpenNotifyTarget {
                        characteristic: prepared.characteristic,
                        control: prepared.control,
                        service: Some(service),
                        session: prepared.session,
                        device: Some(device),
                        bluetooth_address: Some(address),
                    });
                }
                Err(err) => {
                    last_error = Some(format!("persisted startup Cached index={index}: {err}"));
                    let _ = service.Close();
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No subscribable embedded audio BLE notify characteristic found on persisted startup device"
                .to_string()
        }))
    }

    fn open_notify_target() -> Result<OpenNotifyTarget, String> {
        if notify_capture_cancel_requested() {
            return Err(notify_capture_cancelled_error("notify target open"));
        }
        let recent_pairing = recent_pairing_fast_gatt_active(Instant::now());
        if let Some(state) = recent_pairing.as_ref() {
            let paired_elapsed = Instant::now().saturating_duration_since(state.attempted_at);
            if let Some(address) = state
                .address
                .filter(|_| paired_elapsed < BLE_RECENT_PAIRING_CACHED_PROBE_WINDOW)
            {
                match open_notify_target_for_startup_cached_address(address) {
                    Ok(target) => {
                        remember_runtime_bluetooth_target_address_for_current(
                            address,
                            "recent pairing cached audio notify",
                        );
                        log::info!(
                            "[embedded-ble] selected recent-pairing cached GATT readiness path target={:?}",
                            state.target_name
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        return Err(format!(
                            "{RECENT_PAIRING_CACHED_GATT_PROBE_ERROR} target={:?}: {err}",
                            state.target_name
                        ));
                    }
                }
            }
            match open_notify_target_for_known_addresses("recent pairing fast GATT", state.address)
            {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected recent-pairing fast GATT path target={:?}",
                        state.target_name
                    );
                    return Ok(target);
                }
                Err(err) => {
                    if notify_capture_cancel_requested() {
                        return Err(err);
                    }
                    log::info!(
                        "[embedded-ble] recent-pairing fast GATT path not ready target={:?}: {}",
                        state.target_name,
                        err.chars().take(240).collect::<String>()
                    );
                }
            }
        }

        if recent_pairing.is_none() {
            if let Some(address) = persisted_successful_notify_target_address_for_current() {
                if recovery_swift_pair_advertisement_visible_for_persisted_address(address) {
                    return Err(format!(
                        "Listener recovery Swift Pair advertisement visible for persisted address {address:012X} before TYPE:READY; missing pairing must use Type automatic PairAsync recovery before declaring notify ready"
                    ));
                }
                match open_notify_target_for_startup_cached_address(address) {
                    Ok(target) => {
                        remember_runtime_bluetooth_target_address_for_current(
                            address,
                            "persisted startup audio notify",
                        );
                        log::info!(
                            "[embedded-ble] selected persisted startup audio notify address={address:012X}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        log::info!(
                            "[embedded-ble] persisted startup audio notify address={address:012X} not ready: {}",
                            err.chars().take(240).collect::<String>()
                        );
                    }
                }
            }
        }

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
                            remember_runtime_bluetooth_target_address_for_candidate(
                                address,
                                &name,
                                "audio notify device path",
                            );
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
                        if let Some(address) =
                            parse_bluetooth_address_from_device_id(&id.to_string_lossy())
                        {
                            remember_runtime_bluetooth_target_address_for_candidate(
                                address,
                                &name,
                                "audio notify service-id fallback",
                            );
                        }
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

        if let Some(state) = recent_pairing.as_ref() {
            let service_error = last_error.unwrap_or_else(|| {
                "No subscribable embedded audio BLE notify characteristic found by service selector"
                    .to_string()
            });
            return match open_notify_target_from_recent_pairing_advertisement(state) {
                Ok(target) => Ok(target),
                Err(advertisement_error) => Err(format!(
                    "{service_error}; recent pairing target={:?}; fresh paired advertisement fallback failed: {advertisement_error}",
                    state.target_name
                )),
            };
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
            if notify_capture_cancel_requested() {
                return Err(notify_capture_cancelled_error("notify target open"));
            }
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
                    if notify_capture_cancel_requested() {
                        return Err(err);
                    }
                    if attempt > NOTIFY_TARGET_OPEN_RETRY_DELAYS.len()
                        || !is_transient_notify_target_open_error(&err)
                    {
                        return Err(err);
                    }
                    let delay = if err.contains(RECENT_PAIRING_CACHED_GATT_PROBE_ERROR) {
                        Duration::from_millis(250)
                    } else {
                        NOTIFY_TARGET_OPEN_RETRY_DELAYS[attempt - 1]
                    };
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

    fn read_embedded_audio_status_string_once(
        service: &GattDeviceService,
        characteristic_uuid: GUID,
        deadline: Instant,
        label: &str,
    ) -> Option<String> {
        let timeout = remaining_ble_timeout(deadline, Duration::from_millis(900), label).ok()?;
        read_optional_string_characteristic_from_service_with_timeout(
            service,
            characteristic_uuid,
            BluetoothCacheMode::Uncached,
            timeout,
        )
    }

    fn read_embedded_audio_status_strings_with_recovery(
        service: &GattDeviceService,
        deadline: Instant,
    ) -> (Option<String>, Option<String>) {
        let mut readiness = None;
        let mut capabilities = None;
        let mut attempt = 1usize;
        loop {
            if readiness.is_none() {
                readiness = read_embedded_audio_status_string_once(
                    service,
                    OTA_READINESS_UUID,
                    deadline,
                    "readiness",
                );
                if readiness.is_some() && attempt > 1 {
                    log::info!("[embedded-ble] status readiness recovered on attempt {attempt}");
                }
            }
            if capabilities.is_none() {
                capabilities = read_embedded_audio_status_string_once(
                    service,
                    OTA_CAPABILITIES_UUID,
                    deadline,
                    "capabilities",
                );
                if capabilities.is_some() && attempt > 1 {
                    log::info!("[embedded-ble] status capabilities recovered on attempt {attempt}");
                }
            }
            if readiness.is_some() || capabilities.is_some() {
                return (readiness, capabilities);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= Duration::from_millis(200) {
                return (readiness, capabilities);
            }
            let retry_delay = match attempt {
                1 => Duration::from_millis(150),
                2 => Duration::from_millis(350),
                _ => Duration::from_millis(750),
            }
            .min(remaining.saturating_sub(Duration::from_millis(50)));
            log::info!(
                "[embedded-ble] status read attempt {attempt} did not produce a fresh value; retrying in {} ms",
                retry_delay.as_millis()
            );
            std::thread::sleep(retry_delay);
            attempt += 1;
        }
    }

    fn read_embedded_audio_status_from_target_bounded(
        target: &OpenEmbeddedAudioStatusTarget,
        deadline: Instant,
    ) -> crate::embedded_ble::EmbeddedAudioBleStatus {
        let service = &target.service;
        let (readiness, capabilities) =
            read_embedded_audio_status_strings_with_recovery(service, deadline);
        let fresh_status_read = readiness.is_some() || capabilities.is_some();
        let windows_connected = target
            .device
            .as_ref()
            .and_then(|device| device.ConnectionStatus().ok())
            .is_some_and(|status| status == BluetoothConnectionStatus::Connected);
        let detail = if fresh_status_read {
            "embedded audio BLE status characteristic read succeeded"
        } else if windows_connected {
            "Windows reports Listener BLE connected, but fresh status characteristic reads timed out"
        } else {
            "Windows cached Listener BLE service is visible, but no fresh status read confirmed a live link"
        };

        crate::embedded_ble::EmbeddedAudioBleStatus {
            connected: fresh_status_read,
            readiness,
            capabilities,
            detail: Some(detail.to_string()),
        }
    }

    pub fn read_embedded_audio_status(
        timeout: Duration,
    ) -> Result<crate::embedded_ble::EmbeddedAudioBleStatus, String> {
        let _fresh_guard = BleFreshGattGuard::enter("embedded audio status")?;
        let deadline = Instant::now() + timeout.max(Duration::from_millis(250));
        let target = open_embedded_audio_status_target(timeout)?;
        Ok(read_embedded_audio_status_from_target_bounded(
            &target, deadline,
        ))
    }

    fn open_embedded_audio_status_target(
        timeout: Duration,
    ) -> Result<OpenEmbeddedAudioStatusTarget, String> {
        let deadline = Instant::now() + timeout.max(Duration::from_millis(250));
        let recent_pairing = recent_pairing_fast_gatt_active(Instant::now());
        if let Some(state) = recent_pairing.as_ref() {
            if let Some(address) = state.address {
                match open_embedded_audio_status_target_for_device(address, deadline) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected recent-pairing status GATT path target={:?}",
                            state.target_name
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        log::info!(
                            "[embedded-ble] recent-pairing status GATT path not ready target={:?}: {}",
                            state.target_name,
                            err.chars().take(240).collect::<String>()
                        );
                    }
                }
            }
        }

        let selector = GattDeviceService::GetDeviceSelectorFromUuid(SERVICE_UUID)
            .map_err(|err| format!("BLE status service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE status service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    remaining_ble_timeout(
                        deadline,
                        Duration::from_secs(3),
                        "status service discovery",
                    )?,
                    "status service discovery",
                )
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("BLE status service collection size failed: {err}"))?;

        let mut last_error = if count == 0 {
            Some(format!(
                "Embedded audio BLE service {SERVICE_UUID:?} not found by Windows status selector"
            ))
        } else {
            None
        };
        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read BLE status service info failed: {err}"));
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
                    last_error = Some(format!("read BLE status service id failed: {err}"));
                    continue;
                }
            };
            let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
            if !ble_candidate_allowed("audio status", index, &name, address) {
                continue;
            }

            let mut candidate_error = None;
            if let Some(address) = address {
                match open_embedded_audio_status_target_for_device(address, deadline) {
                    Ok(target) => {
                        remember_runtime_bluetooth_target_address_for_candidate(
                            address,
                            &name,
                            "audio status device path",
                        );
                        log::info!(
                            "[embedded-ble] selected status device path index={index} name={name} address={address:012X}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        candidate_error = Some(format!(
                            "{name}: BLE status device path {address:012X} failed: {err}"
                        ));
                    }
                }
            }

            match open_embedded_audio_status_target_for_service(&id, deadline) {
                Ok(target) => {
                    if let Some(address) =
                        parse_bluetooth_address_from_device_id(&id.to_string_lossy())
                    {
                        remember_runtime_bluetooth_target_address_for_candidate(
                            address,
                            &name,
                            "audio status service-id fallback",
                        );
                    }
                    log::info!(
                        "[embedded-ble] selected status service-id fallback index={index} name={name}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(match candidate_error {
                        Some(previous) => {
                            format!("{previous}; status service-id fallback failed: {err}")
                        }
                        None => format!("{name}: {err}"),
                    });
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| "No reachable embedded audio BLE status service found".to_string()))
    }

    pub(super) fn is_transient_notify_target_open_error(err: &str) -> bool {
        if err.contains("No paired BLE device found in Windows Bluetooth pairing store") {
            return false;
        }
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
            || err.contains(RECENT_PAIRING_CACHED_GATT_PROBE_ERROR)
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
                            remember_runtime_bluetooth_target_address_for_candidate(
                                address,
                                &name,
                                "audio control device path",
                            );
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
                        if let Some(address) =
                            parse_bluetooth_address_from_device_id(&id.to_string_lossy())
                        {
                            remember_runtime_bluetooth_target_address_for_candidate(
                                address,
                                &name,
                                "audio control service-id fallback",
                            );
                        }
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

    fn ensure_paired_listener_for_advertisement_gatt(
        context: &str,
        addresses: &[u64],
        allow_pnp_service_signature_cache: bool,
    ) -> Result<(), String> {
        if addresses.is_empty() {
            return Err(format!(
                "{context} advertisement scan returned no Listener addresses before GATT fallback"
            ));
        }

        if paired_listener_device_visible_for_addresses(
            context,
            addresses,
            allow_pnp_service_signature_cache,
        )? {
            return Ok(());
        }

        let address_list = addresses
            .iter()
            .map(|address| format!("{address:012X}"))
            .collect::<Vec<_>>()
            .join(",");
        Err(format!(
            "No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es) {address_list}; skipping {context} advertisement GATT fallback until Windows pairing completes"
        ))
    }

    fn paired_listener_device_visible_for_addresses(
        context: &str,
        addresses: &[u64],
        allow_pnp_service_signature_cache: bool,
    ) -> Result<bool, String> {
        let target_name = effective_bluetooth_target_name(None);
        let target_addresses = listener_recovery_target_addresses();
        let selector =
            BluetoothLEDevice::GetDeviceSelectorFromPairingState(true).map_err(|err| {
                format!(
                    "paired BLE device selector failed before advertisement GATT fallback: {err}"
                )
            })?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| {
                format!("paired BLE device query failed before advertisement GATT fallback: {err}")
            })
            .and_then(|op| {
                wait_async_operation(
                    op,
                    BLE_DISCOVERY_TIMEOUT,
                    "paired BLE device query before advertisement GATT fallback",
                )
            })?;
        let count = devices.Size().map_err(|err| {
            format!("paired BLE device collection size failed before advertisement GATT fallback: {err}")
        })?;

        for index in 0..count {
            let info = devices.GetAt(index).map_err(|err| {
                format!(
                    "paired BLE device entry {index} read failed before advertisement GATT fallback: {err}"
                )
            })?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = info
                .Id()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let address = parse_bluetooth_address_from_device_id(&id);
            let address_matches = address.is_some_and(|value| {
                addresses.contains(&value) || target_addresses.contains(&value)
            });
            let name_matches = bluetooth_name_matches_expected(&name, &target_name);
            if !address_matches && !name_matches {
                continue;
            }
            let paired = info
                .Pairing()
                .and_then(|pairing| pairing.IsPaired())
                .unwrap_or(false);
            if paired {
                log::info!(
                    "[embedded-ble] {context}: paired Windows BLE device visible name={name:?} address={:?}; allowing advertised GATT fallback while service index refreshes",
                    address.map(crate::embedded_ble::format_bluetooth_address)
                );
                return Ok(true);
            }
        }

        if allow_pnp_service_signature_cache {
            match listener_pnp_service_signature_addresses() {
                Ok(pnp_addresses)
                    if pnp_addresses
                        .iter()
                        .any(|address| addresses.contains(address)) =>
                {
                    let labels = pnp_addresses
                        .iter()
                        .map(|address| format!("{address:012X}"))
                        .collect::<Vec<_>>();
                    log::info!(
                        "[embedded-ble] {context}: Windows PnP Listener service node visible for advertised address(es) {labels:?}; allowing advertised GATT fallback while AEP pairing cache refreshes"
                    );
                    return Ok(true);
                }
                Ok(_) => {}
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] {context}: Windows PnP Listener service-signature pairing check failed before advertisement GATT fallback: {err}"
                    );
                }
            }
        }

        Ok(false)
    }

    fn open_notify_target_for_known_addresses(
        context: &str,
        preferred_address: Option<u64>,
    ) -> Result<OpenNotifyTarget, String> {
        let mut addresses = Vec::new();
        if let Some(address) = preferred_address {
            push_unique_address(&mut addresses, address);
        }
        for address in listener_recovery_target_addresses() {
            push_unique_address(&mut addresses, address);
        }
        if let Some(address) = configured_bluetooth_address() {
            push_unique_address(&mut addresses, address);
        }
        if addresses.is_empty() {
            return Err(format!("{context}: no known Listener BLE address"));
        }

        let mut last_error = None;
        for address in addresses {
            match open_notify_target_for_device(address) {
                Ok(target) => {
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "known address audio notify",
                    );
                    log::info!(
                        "[embedded-ble] {context}: selected known Listener address={address:012X}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(format!(
                        "{context}: known Listener address {address:012X} failed: {err}"
                    ));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| format!("{context}: no usable known Listener address")))
    }

    fn recovery_swift_pair_advertisement_visible_for_persisted_address(address: u64) -> bool {
        let target_name = effective_bluetooth_target_name(None);
        match listener_swift_pair_advertisement_visible_for_address(
            "persisted audio notify recovery guard",
            &target_name,
            address,
            TYPE_READY_RECOVERY_PAIRING_ADV_PROBE_TIMEOUT,
        ) {
            Ok(visible) => visible,
            Err(err) => {
                log::info!(
                    "[embedded-ble] persisted audio notify recovery guard scan skipped target={target_name:?} address={address:012X}: {err}"
                );
                false
            }
        }
    }

    fn active_capture_disconnect_recovery_pairing_error(reason: &str) -> Option<String> {
        let address = persisted_successful_notify_target_address_for_current()?;
        if !recovery_swift_pair_advertisement_visible_for_persisted_address(address) {
            return None;
        }
        Some(format!(
            "Listener recovery Swift Pair advertisement visible for active capture address {address:012X} after {reason}; missing pairing must use Type automatic PairAsync recovery before declaring notify ready"
        ))
    }

    fn open_notify_target_from_advertisement() -> Result<OpenNotifyTarget, String> {
        let addresses = audio_target_advertisement_addresses("audio notify")?;
        ensure_paired_listener_for_advertisement_gatt("audio notify", &addresses, false)?;
        let mut last_error = None;
        for address in addresses {
            match open_notify_target_for_device(address) {
                Ok(target) => {
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "advertised audio notify",
                    );
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

    fn open_notify_target_from_recent_pairing_advertisement(
        state: &RecentPairingFastGattState,
    ) -> Result<OpenNotifyTarget, String> {
        let mut addresses = Vec::new();
        if let Some(address) = state.address {
            push_unique_address(&mut addresses, address);
        }
        match scan_ble_advertisements_by_name(
            "recent pairing audio notify",
            &state.target_name,
            AUDIO_ADVERTISEMENT_SCAN_TIMEOUT,
        ) {
            Ok(scanned_addresses) => {
                for address in scanned_addresses {
                    push_unique_address(&mut addresses, address);
                }
            }
            Err(err) => {
                log::info!(
                    "[embedded-ble] recent pairing audio notify advertisement refresh target={:?} failed: {}",
                    state.target_name,
                    err.chars().take(240).collect::<String>()
                );
            }
        }
        if addresses.is_empty() {
            return Err(format!(
                "recent pairing audio notify has no fresh address target={:?}",
                state.target_name
            ));
        }
        ensure_paired_listener_for_advertisement_gatt(
            "recent pairing audio notify",
            &addresses,
            true,
        )?;
        let mut last_error = None;
        for address in addresses {
            match open_notify_target_for_device(address) {
                Ok(target) => {
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "recent pairing advertised audio notify",
                    );
                    log::info!(
                        "[embedded-ble] selected recent-pairing advertised audio notify address={address:012X} target={:?}",
                        state.target_name
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(format!(
                        "recent pairing advertised audio notify address {address:012X} failed: {err}"
                    ));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "recent pairing audio notify advertisement scan returned no usable addresses"
                .to_string()
        }))
    }

    fn open_audio_control_target_from_advertisement() -> Result<OpenAudioControlTarget, String> {
        let addresses = audio_target_advertisement_addresses("audio control")?;
        ensure_paired_listener_for_advertisement_gatt("audio control", &addresses, false)?;
        let mut last_error = None;
        for address in addresses {
            match open_audio_control_target_for_device(address) {
                Ok(target) => {
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "advertised audio control",
                    );
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
        if let Some(address) = configured_bluetooth_address_from_env() {
            log::info!(
                "[embedded-ble] using pinned env-configured {kind} BLE address {}",
                crate::embedded_ble::format_bluetooth_address(address)
            );
            return Ok(vec![address]);
        }

        let mut addresses = Vec::new();
        let scan_result = if let Some(expected_name) = configured_bluetooth_target_name() {
            scan_ble_advertisements_by_name(kind, &expected_name, AUDIO_ADVERTISEMENT_SCAN_TIMEOUT)
        } else {
            scan_listener_audio_advertisements(kind, AUDIO_ADVERTISEMENT_SCAN_TIMEOUT)
        };
        let mut errors = Vec::new();
        match scan_result {
            Ok(scanned_addresses) => {
                for address in scanned_addresses {
                    push_unique_address(&mut addresses, address);
                }
            }
            Err(err) => errors.push(err),
        }

        match listener_pnp_service_signature_addresses() {
            Ok(pnp_addresses) => {
                for address in pnp_addresses {
                    push_unique_address(&mut addresses, address);
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] {kind}: Windows PnP address refresh failed during advertisement fallback: {err}"
                );
            }
        }

        if let Some(address) = runtime_bluetooth_target_address() {
            let was_empty = addresses.is_empty();
            push_unique_address(&mut addresses, address);
            log::info!(
                "[embedded-ble] appended cached runtime {kind} BLE address {} after fresh Windows candidates",
                crate::embedded_ble::format_bluetooth_address(address)
            );
            if was_empty {
                log::warn!(
                    "[embedded-ble] using cached runtime {kind} BLE address only because fresh advertisement/PnP discovery returned no address"
                );
            }
        }

        if addresses.is_empty() {
            return Err(errors.pop().unwrap_or_else(|| {
                format!("{kind} address discovery returned no usable address")
            }));
        }
        Ok(addresses)
    }

    pub(super) fn remember_current_bluetooth_target_address_for_name(
        next_target_name: &str,
        timeout: Duration,
        context: &str,
    ) -> Option<u64> {
        let next_target_name = normalize_bluetooth_target_name(next_target_name)?;
        if let Some(address) = configured_bluetooth_address_from_env() {
            remember_runtime_bluetooth_target_address_for_name(
                address,
                &next_target_name,
                BLE_RENAME_ADDRESS_GRACE_WINDOW,
                context,
            );
            return Some(address);
        }

        let current_target_name = effective_bluetooth_target_name(None);
        let discovery_timeout = timeout.clamp(Duration::from_millis(300), Duration::from_secs(2));
        let address = find_bluetooth_target_service_address(
            SERVICE_UUID,
            &current_target_name,
            discovery_timeout,
            context,
        )
        .or_else(|| {
            find_bluetooth_target_service_address(
                OTA_SERVICE_UUID,
                &current_target_name,
                discovery_timeout,
                context,
            )
        })
        .or_else(|| {
            find_paired_bluetooth_target_address(&current_target_name, discovery_timeout, context)
        })
        .or_else(|| {
            scan_ble_advertisements_by_name(context, &current_target_name, discovery_timeout)
                .ok()
                .and_then(|addresses| addresses.into_iter().next())
        });

        if let Some(address) = address {
            remember_runtime_bluetooth_target_address_for_name(
                address,
                &next_target_name,
                BLE_RENAME_ADDRESS_GRACE_WINDOW,
                context,
            );
        } else {
            log::info!(
                "[embedded-ble] {context}: no current Listener address learned before target transition current={current_target_name:?} next={next_target_name:?}"
            );
        }
        address
    }

    pub(super) fn wait_for_bluetooth_target_advertisement_by_name(
        target_name: &str,
        timeout: Duration,
        context: &str,
    ) -> Result<u64, String> {
        let target_name = normalize_bluetooth_target_name(target_name)
            .ok_or_else(|| format!("{context}: empty BLE target name"))?;
        if let Some(address) = configured_bluetooth_address_from_env() {
            remember_runtime_bluetooth_target_address_for_name(
                address,
                &target_name,
                BLE_RENAME_ADDRESS_GRACE_WINDOW,
                context,
            );
            return Ok(address);
        }
        let addresses = scan_ble_advertisements_by_name(context, &target_name, timeout)?;
        let address = addresses
            .into_iter()
            .next()
            .ok_or_else(|| format!("{context}: advertisement scan returned no address"))?;
        remember_runtime_bluetooth_target_address_for_name(
            address,
            &target_name,
            BLE_RENAME_ADDRESS_GRACE_WINDOW,
            context,
        );
        Ok(address)
    }

    fn find_bluetooth_target_service_address(
        service_uuid: GUID,
        target_name: &str,
        timeout: Duration,
        context: &str,
    ) -> Option<u64> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(service_uuid).ok()?;
        let services = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .ok()
            .and_then(|op| wait_async_operation(op, timeout, context).ok())?;
        let mut service_addresses = Vec::new();
        for index in 0..services.Size().ok()? {
            let info = services.GetAt(index).ok()?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            let id = info.Id().ok()?.to_string_lossy();
            if let Some(address) = parse_bluetooth_address_from_device_id(&id) {
                push_unique_address(&mut service_addresses, address);
            }
            if !bluetooth_name_matches_expected(&name, target_name) {
                continue;
            }
            if let Some(address) = parse_bluetooth_address_from_device_id(&id) {
                log::info!(
                    "[embedded-ble] {context}: learned Listener address from service target={target_name:?} name={name:?} address={address:012X}"
                );
                return Some(address);
            }
        }
        if service_addresses.len() == 1 {
            let address = service_addresses[0];
            log::info!(
                "[embedded-ble] {context}: learned sole Listener service address despite Windows name cache mismatch target={target_name:?} address={address:012X}"
            );
            return Some(address);
        }
        None
    }

    fn find_paired_bluetooth_target_address(
        target_name: &str,
        timeout: Duration,
        context: &str,
    ) -> Option<u64> {
        let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true).ok()?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .ok()
            .and_then(|op| wait_async_operation(op, timeout, context).ok())?;
        for index in 0..devices.Size().ok()? {
            let info = devices.GetAt(index).ok()?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if !bluetooth_name_matches_expected(&name, target_name) {
                continue;
            }
            let id = info.Id().ok()?.to_string_lossy();
            if let Some(address) = parse_bluetooth_address_from_device_id(&id) {
                log::info!(
                    "[embedded-ble] {context}: learned Listener address from paired device target={target_name:?} name={name:?} address={address:012X}"
                );
                return Some(address);
            }
        }
        None
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

    fn read_optional_string_characteristic_from_service_with_timeout(
        service: &GattDeviceService,
        characteristic_uuid: GUID,
        cache_mode: BluetoothCacheMode,
        timeout: Duration,
    ) -> Option<String> {
        read_optional_characteristic_from_service_with_timeout(
            service,
            characteristic_uuid,
            cache_mode,
            timeout,
        )
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|value| value.trim_matches(char::from(0)).trim().to_string())
        .filter(|value| !value.is_empty())
    }

    fn read_optional_string_characteristic_from_service(
        service: &GattDeviceService,
        characteristic_uuid: GUID,
        cache_mode: BluetoothCacheMode,
    ) -> Option<String> {
        read_optional_string_characteristic_from_service_with_timeout(
            service,
            characteristic_uuid,
            cache_mode,
            BLE_DISCOVERY_TIMEOUT.min(Duration::from_secs(2)),
        )
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
        let optional_timeout = BLE_DISCOVERY_TIMEOUT.min(Duration::from_secs(2));
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(service_uuid).ok()?;
        let services = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .ok()?
            .wait_ble_result(optional_timeout, "optional service discovery")
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
            let service = GattDeviceService::FromIdAsync(&id)
                .ok()?
                .wait_ble_result(optional_timeout, "optional service open")
                .ok()?;
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
        let optional_timeout = BLE_DISCOVERY_TIMEOUT.min(Duration::from_secs(2));
        for cache_mode in [BluetoothCacheMode::Uncached] {
            let services_result = device
                .GetGattServicesForUuidWithCacheModeAsync(service_uuid, cache_mode)
                .ok()?
                .wait_ble_result(optional_timeout, "optional GATT service discovery")
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

    fn read_optional_characteristic_from_service_with_timeout(
        service: &GattDeviceService,
        characteristic_uuid: GUID,
        cache_mode: BluetoothCacheMode,
        timeout: Duration,
    ) -> Option<Vec<u8>> {
        let timeout = timeout.min(BLE_DISCOVERY_TIMEOUT);
        let result = match service
            .GetCharacteristicsForUuidWithCacheModeAsync(characteristic_uuid, cache_mode)
            .ok()?
            .wait_ble_result(timeout, "optional characteristic")
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
            .wait_ble_result(timeout, "optional characteristic read")
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

    fn read_optional_characteristic_from_service(
        service: &GattDeviceService,
        characteristic_uuid: GUID,
        cache_mode: BluetoothCacheMode,
    ) -> Option<Vec<u8>> {
        read_optional_characteristic_from_service_with_timeout(
            service,
            characteristic_uuid,
            cache_mode,
            BLE_DISCOVERY_TIMEOUT.min(Duration::from_secs(2)),
        )
    }

    fn open_listener_ota_v1_target() -> Result<OpenListenerOtaV1Target, String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(LISTENER_OTA_V1_SERVICE_UUID)
            .map_err(|err| format!("Listener OTA v1 service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("Listener OTA v1 service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    BLE_DISCOVERY_TIMEOUT,
                    "Listener OTA v1 service discovery",
                )
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("Listener OTA v1 service collection size failed: {err}"))?;
        if count == 0 {
            return open_listener_ota_v1_target_from_cached_address(format!(
                "Listener OTA v1 service {LISTENER_OTA_V1_SERVICE_UUID:?} not found in Windows service index"
            ));
        }

        let mut last_error = None;
        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read Listener OTA v1 service info failed: {err}"));
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
                    last_error = Some(format!("read Listener OTA v1 service id failed: {err}"));
                    continue;
                }
            };
            let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
            if !ble_candidate_allowed("Listener OTA v1", index, &name, address) {
                continue;
            }

            let mut candidate_error = None;
            if let Some(address) = address {
                match open_listener_ota_v1_target_for_device(address) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected Listener OTA v1 device index={index} name={name} address={address:012X}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        candidate_error = Some(format!(
                            "{name}: Listener OTA v1 device path {address:012X} failed: {err}"
                        ));
                    }
                }
            }

            match open_listener_ota_v1_target_for_service(&id) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected Listener OTA v1 service-id fallback index={index} name={name}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(match candidate_error {
                        Some(previous) => {
                            format!("{previous}; Listener OTA v1 service-id fallback failed: {err}")
                        }
                        None => format!("{name}: {err}"),
                    });
                }
            }
        }

        match open_listener_ota_v1_target_from_cached_address(
            last_error.unwrap_or_else(|| "No writable Listener OTA v1 service found".to_string()),
        ) {
            Ok(target) => Ok(target),
            Err(err) => Err(err),
        }
    }

    fn open_listener_ota_v1_target_with_deadline(
        deadline: Instant,
    ) -> Result<OpenListenerOtaV1Target, String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(LISTENER_OTA_V1_SERVICE_UUID)
            .map_err(|err| format!("Listener OTA v1 service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("Listener OTA v1 service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    remaining_ble_timeout(
                        deadline,
                        BLE_DISCOVERY_TIMEOUT,
                        "Listener OTA v1 service discovery",
                    )?,
                    "Listener OTA v1 service discovery",
                )
            })?;
        let count = devices
            .Size()
            .map_err(|err| format!("Listener OTA v1 service collection size failed: {err}"))?;
        if count == 0 {
            return Err(format!(
                "Listener OTA v1 service {LISTENER_OTA_V1_SERVICE_UUID:?} not found in Windows service index"
            ));
        }

        let mut last_error = None;
        for index in 0..count {
            let info = match devices.GetAt(index) {
                Ok(info) => info,
                Err(err) => {
                    last_error = Some(format!("read Listener OTA v1 service info failed: {err}"));
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
                    last_error = Some(format!("read Listener OTA v1 service id failed: {err}"));
                    continue;
                }
            };
            let address = parse_bluetooth_address_from_device_id(&id.to_string_lossy());
            if !ble_candidate_allowed("Listener OTA v1", index, &name, address) {
                continue;
            }

            let mut candidate_error = None;
            if let Some(address) = address {
                match open_listener_ota_v1_target_for_device_with_deadline(address, deadline) {
                    Ok(target) => {
                        log::info!(
                            "[embedded-ble] selected Listener OTA v1 device index={index} name={name} address={address:012X}"
                        );
                        return Ok(target);
                    }
                    Err(err) => {
                        candidate_error = Some(format!(
                            "{name}: Listener OTA v1 device path {address:012X} failed: {err}"
                        ));
                    }
                }
            }

            match open_listener_ota_v1_target_for_service_with_deadline(&id, deadline) {
                Ok(target) => {
                    log::info!(
                        "[embedded-ble] selected Listener OTA v1 service-id fallback index={index} name={name}"
                    );
                    return Ok(target);
                }
                Err(err) => {
                    last_error = Some(match candidate_error {
                        Some(previous) => {
                            format!("{previous}; Listener OTA v1 service-id fallback failed: {err}")
                        }
                        None => format!("{name}: {err}"),
                    });
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "No writable Listener OTA v1 service found".to_string()))
    }

    fn open_listener_ota_v1_target_from_cached_address(
        previous_error: String,
    ) -> Result<OpenListenerOtaV1Target, String> {
        log::warn!(
            "[embedded-ble] Denzic OTA v1 direct discovery failed; trying cached Listener address: {previous_error}"
        );
        let address = runtime_bluetooth_target_address().ok_or_else(|| {
            format!("{previous_error}; no current Listener Bluetooth address is cached")
        })?;
        open_listener_ota_v1_target_for_device(address).map_err(|err| {
            format!(
                "{previous_error}; Denzic OTA v1 discovery via cached Listener address {} failed: {err}",
                crate::embedded_ble::format_bluetooth_address(address)
            )
        })
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

    pub fn listener_recovery_pairing_advertisement_visible(
        expected_name: Option<&str>,
        timeout: Duration,
    ) -> bool {
        listener_recovery_pairing_advertisement_probe(expected_name, timeout).visible
    }

    pub fn listener_recovery_pairing_advertisement_probe(
        expected_name: Option<&str>,
        timeout: Duration,
    ) -> crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
        let expected_name = effective_bluetooth_target_name(expected_name);
        match scan_listener_advertisements(
            "Listener recovery pairing",
            Some(&expected_name),
            timeout,
        ) {
            Ok(candidates) if candidates.is_empty() => {
                log::info!(
                    "[embedded-ble] no recovery pairing advertisement visible target={expected_name:?} timeout_ms={}",
                    timeout.as_millis()
                );
                crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default()
            }
            Ok(candidates) => {
                if let Some((address, address_type, name)) = candidates.first() {
                    log::info!(
                        "[embedded-ble] recovery pairing advertisement visible target={expected_name:?} name={name:?} address={address:012X} address_type={address_type:?}"
                    );
                }
                crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
                    visible: true,
                    has_random_identity: candidates
                        .iter()
                        .any(|(_, address_type, _)| *address_type == BluetoothAddressType::Random),
                }
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] recovery pairing advertisement probe failed target={expected_name:?}: {err}"
                );
                crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default()
            }
        }
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

        let (tx, rx) = mpsc::channel::<(u64, BluetoothAddressType, String, i16, String)>();
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
            let swift_pair_name = advertisement_swift_pair_display_name(&advertisement);
            let candidate_name =
                if listener_pairing_name_matches(&name, expected_name_for_handler.as_deref()) {
                    name
                } else if let Some(swift_pair_name) = swift_pair_name.filter(|swift_pair_name| {
                    listener_pairing_name_matches(
                        swift_pair_name,
                        expected_name_for_handler.as_deref(),
                    )
                }) {
                    swift_pair_name
                } else {
                    return Ok(());
                };
            if candidate_name.trim().is_empty() {
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
            let manufacturer_data = advertisement_manufacturer_data_summary(&advertisement);
            let _ = tx.send((
                address,
                address_type,
                candidate_name,
                rssi,
                manufacturer_data,
            ));
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
                Ok((address, address_type, name, rssi, manufacturer_data)) => {
                    if addresses.iter().any(|(seen, _, _)| *seen == address) {
                        continue;
                    }
                    log::info!(
                        "[embedded-ble] {context} advertisement candidate name={name} address={address:012X} address_type={address_type:?} rssi={rssi} {manufacturer_data}"
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

    fn listener_swift_pair_advertisement_visible_for_address(
        context: &str,
        expected_name: &str,
        target_address: u64,
        timeout: Duration,
    ) -> Result<bool, String> {
        let watcher = BluetoothLEAdvertisementWatcher::new()
            .map_err(|err| format!("{context} advertisement watcher create failed: {err}"))?;
        watcher
            .SetScanningMode(BluetoothLEScanningMode::Active)
            .map_err(|err| format!("{context} advertisement active scan failed: {err}"))?;

        let (tx, rx) = mpsc::channel::<(u64, BluetoothAddressType, String, i16, String)>();
        let expected_name = expected_name.to_string();
        let expected_name_for_handler = expected_name.clone();
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
            let Some(swift_pair_name) = advertisement_swift_pair_display_name(&advertisement)
            else {
                return Ok(());
            };
            if !listener_pairing_name_matches(&swift_pair_name, Some(&expected_name_for_handler)) {
                return Ok(());
            }
            let address = args.BluetoothAddress().unwrap_or_default();
            if address == 0 || address != target_address {
                return Ok(());
            }
            let address_type = args
                .BluetoothAddressType()
                .unwrap_or(BluetoothAddressType::Unspecified);
            let rssi = args.RawSignalStrengthInDBm().unwrap_or_default();
            let manufacturer_data = advertisement_manufacturer_data_summary(&advertisement);
            let _ = tx.send((
                address,
                address_type,
                swift_pair_name,
                rssi,
                manufacturer_data,
            ));
            Ok(())
        });

        let token = watcher
            .Received(&handler)
            .map_err(|err| format!("{context} advertisement handler failed: {err}"))?;
        watcher
            .Start()
            .map_err(|err| format!("{context} advertisement scan start failed: {err}"))?;

        let deadline = Instant::now() + timeout;
        let mut visible = false;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = remaining.min(Duration::from_millis(250));
            match rx.recv_timeout(timeout) {
                Ok((address, address_type, name, rssi, manufacturer_data)) => {
                    log::info!(
                        "[embedded-ble] {context} Swift Pair advertisement visible name={name} address={address:012X} address_type={address_type:?} rssi={rssi} {manufacturer_data}"
                    );
                    visible = true;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        let _ = watcher.Stop();
        let _ = watcher.RemoveReceived(token);

        if !visible {
            log::debug!(
                "[embedded-ble] {context} Swift Pair advertisement not visible for target={expected_name:?} address={target_address:012X} timeout_ms={}",
                timeout.as_millis()
            );
        }
        Ok(visible)
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

    fn open_listener_ota_v1_target_for_device(
        address: u64,
    ) -> Result<OpenListenerOtaV1Target, String> {
        let device = open_ble_device(address)?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Listener OTA v1 device access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Listener OTA v1 device access denied status={access:?}"
                ));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(LISTENER_OTA_V1_SERVICE_UUID, cache_mode)
                .map_err(|err| {
                    format!("Listener OTA v1 {cache_mode:?} service discovery failed: {err}")
                })
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        &format!("Listener OTA v1 {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!(
                            "Listener OTA v1 {cache_mode:?} service discovery wait failed: {err}"
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
                format!("Listener OTA v1 {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "Listener OTA v1 {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("Listener OTA v1 {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("Listener OTA v1 {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "Listener OTA v1 service {LISTENER_OTA_V1_SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read Listener OTA v1 {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                match open_listener_ota_v1_characteristics_from_service_with_retry(
                    &service, cache_mode,
                ) {
                    Ok(prepared) => {
                        log::info!(
                            "[embedded-ble] Listener OTA v1 selected {cache_mode:?} GATT characteristics"
                        );
                        return Ok(OpenListenerOtaV1Target {
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
            "No writable Listener OTA v1 characteristics found on device".to_string()
        }))
    }

    fn open_listener_ota_v1_target_for_device_with_deadline(
        address: u64,
        deadline: Instant,
    ) -> Result<OpenListenerOtaV1Target, String> {
        let device = open_ble_device_with_timeout(
            address,
            remaining_ble_timeout(
                deadline,
                BLE_DISCOVERY_TIMEOUT,
                "Listener OTA v1 device open",
            )?,
        )?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(
                op,
                remaining_ble_timeout(
                    deadline,
                    BLE_DISCOVERY_TIMEOUT,
                    "Listener OTA v1 device access",
                )
                .ok()?,
                "Listener OTA v1 device access",
            )
            .ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Listener OTA v1 device access denied status={access:?}"
                ));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(LISTENER_OTA_V1_SERVICE_UUID, cache_mode)
                .map_err(|err| {
                    format!("Listener OTA v1 {cache_mode:?} service discovery failed: {err}")
                })
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        remaining_ble_timeout(
                            deadline,
                            BLE_DISCOVERY_TIMEOUT,
                            "Listener OTA v1 service",
                        )?,
                        &format!("Listener OTA v1 {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!(
                            "Listener OTA v1 {cache_mode:?} service discovery wait failed: {err}"
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
                format!("Listener OTA v1 {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "Listener OTA v1 {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("Listener OTA v1 {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("Listener OTA v1 {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "Listener OTA v1 service {LISTENER_OTA_V1_SERVICE_UUID:?} not found from BLE device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read Listener OTA v1 {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                match open_listener_ota_v1_characteristics_from_service_with_retry_deadline(
                    &service, cache_mode, deadline,
                ) {
                    Ok(prepared) => {
                        log::info!(
                            "[embedded-ble] Listener OTA v1 selected {cache_mode:?} GATT characteristics"
                        );
                        return Ok(OpenListenerOtaV1Target {
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
            "No writable Listener OTA v1 characteristics found on device".to_string()
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

    fn open_embedded_audio_status_target_for_device(
        address: u64,
        deadline: Instant,
    ) -> Result<OpenEmbeddedAudioStatusTarget, String> {
        let device = open_ble_device_with_timeout(
            address,
            remaining_ble_timeout(deadline, Duration::from_secs(3), "status device open")?,
        )?;
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            remaining_ble_timeout(deadline, Duration::from_secs(1), "status device access")
                .ok()
                .and_then(|timeout| wait_async_operation(op, timeout, "status device access").ok())
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!("BLE status device access denied status={access:?}"));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(SERVICE_UUID, cache_mode)
                .map_err(|err| format!("BLE status {cache_mode:?} service discovery failed: {err}"))
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        remaining_ble_timeout(deadline, Duration::from_secs(3), "status service")?,
                        &format!("status {cache_mode:?} service"),
                    )
                    .map_err(|err| {
                        format!("BLE status {cache_mode:?} service discovery wait failed: {err}")
                    })
                }) {
                Ok(result) => result,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            let status = services_result.Status().map_err(|err| {
                format!("BLE status {cache_mode:?} service status read failed: {err}")
            })?;
            if status != GattCommunicationStatus::Success {
                last_error = Some(format!(
                    "BLE status {cache_mode:?} service discovery returned status={status:?}"
                ));
                continue;
            }

            let services = services_result.Services().map_err(|err| {
                format!("BLE status {cache_mode:?} service list read failed: {err}")
            })?;
            let count = services.Size().map_err(|err| {
                format!("BLE status {cache_mode:?} service list size failed: {err}")
            })?;
            if count == 0 {
                last_error = Some(format!(
                    "service {SERVICE_UUID:?} not found from BLE status device via {cache_mode:?}"
                ));
                continue;
            }

            for index in 0..count {
                let service = match services.GetAt(index) {
                    Ok(service) => service,
                    Err(err) => {
                        last_error = Some(format!(
                            "read BLE status {cache_mode:?} service failed: {err}"
                        ));
                        continue;
                    }
                };
                return Ok(OpenEmbeddedAudioStatusTarget {
                    service,
                    device: Some(device),
                });
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No reachable embedded audio BLE status service found on device".to_string()
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
        open_ble_device_with_timeout(address, BLE_DISCOVERY_TIMEOUT)
    }

    fn open_ble_device_with_timeout(
        address: u64,
        timeout: Duration,
    ) -> Result<BluetoothLEDevice, String> {
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(address)
            .map_err(|err| format!("BLE device open by address failed: {err}"))
            .and_then(|op| wait_async_operation(op, timeout, "device open by address"))?;

        let device_id = device
            .DeviceId()
            .map(|id| id.to_string_lossy())
            .unwrap_or_default();
        if device_id.is_empty() {
            return Ok(device);
        }

        match BluetoothLEDevice::FromIdAsync(&HSTRING::from(device_id.as_str()))
            .ok()
            .and_then(|op| wait_async_operation(op, timeout, "device open by id").ok())
        {
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

    fn open_listener_ota_v1_target_for_service(
        service_id: &HSTRING,
    ) -> Result<OpenListenerOtaV1Target, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("Listener OTA v1 service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Listener OTA v1 service open")
            })?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        BLE_DISCOVERY_TIMEOUT,
                        "Listener OTA v1 service device",
                    )
                    .ok()
                })
        });

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached] {
            match open_listener_ota_v1_characteristics_from_service_with_retry(&service, cache_mode)
            {
                Ok(prepared) => {
                    return Ok(OpenListenerOtaV1Target {
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
            "No writable Listener OTA v1 characteristics found from service id".to_string()
        }))
    }

    fn open_listener_ota_v1_target_for_service_with_deadline(
        service_id: &HSTRING,
        deadline: Instant,
    ) -> Result<OpenListenerOtaV1Target, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("Listener OTA v1 service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    remaining_ble_timeout(
                        deadline,
                        BLE_DISCOVERY_TIMEOUT,
                        "Listener OTA v1 service open",
                    )?,
                    "Listener OTA v1 service open",
                )
            })?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    wait_async_operation(
                        op,
                        remaining_ble_timeout(
                            deadline,
                            BLE_DISCOVERY_TIMEOUT,
                            "Listener OTA v1 service device",
                        )
                        .ok()?,
                        "Listener OTA v1 service device",
                    )
                    .ok()
                })
        });

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached] {
            match open_listener_ota_v1_characteristics_from_service_with_retry_deadline(
                &service, cache_mode, deadline,
            ) {
                Ok(prepared) => {
                    return Ok(OpenListenerOtaV1Target {
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
            "No writable Listener OTA v1 characteristics found from service id".to_string()
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
            bluetooth_address: parse_bluetooth_address_from_device_id(
                &service_id.to_string_lossy(),
            ),
        })
    }

    fn open_embedded_audio_status_target_for_service(
        service_id: &HSTRING,
        deadline: Instant,
    ) -> Result<OpenEmbeddedAudioStatusTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE status service open failed: {err}"))
            .and_then(|op| {
                wait_async_operation(
                    op,
                    remaining_ble_timeout(deadline, Duration::from_secs(3), "status service open")?,
                    "status service open",
                )
            })?;
        let device = service.DeviceId().ok().and_then(|device_id| {
            BluetoothLEDevice::FromIdAsync(&device_id)
                .ok()
                .and_then(|op| {
                    remaining_ble_timeout(deadline, Duration::from_secs(1), "status service device")
                        .ok()
                        .and_then(|timeout| {
                            wait_async_operation(op, timeout, "status service device").ok()
                        })
                })
        });

        Ok(OpenEmbeddedAudioStatusTarget { service, device })
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

    fn open_listener_ota_v1_characteristics_from_service(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedListenerOtaV1Characteristics, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Listener OTA v1 service access").ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Listener OTA v1 service access denied status={access:?}"
                ));
            }
        }
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT)?;
        let control = open_write_characteristic_from_service(
            service,
            LISTENER_OTA_V1_CONTROL_UUID,
            "Listener OTA v1 control",
            cache_mode,
        )?;
        let data = open_write_characteristic_from_service(
            service,
            LISTENER_OTA_V1_DATA_UUID,
            "Listener OTA v1 data",
            cache_mode,
        )?;
        let status = if LISTENER_OTA_V1_STATUS_UUID == LISTENER_OTA_V1_CONTROL_UUID {
            control.clone()
        } else {
            open_read_characteristic_from_service(
                service,
                LISTENER_OTA_V1_STATUS_UUID,
                "Listener OTA v1 status",
                cache_mode,
            )?
        };
        let data_properties = data.CharacteristicProperties().map_err(|err| {
            format!("Listener OTA v1 data characteristic properties read failed: {err}")
        })?;
        let data_write_option = listener_ota_v1_data_write_option(data_properties)?;
        let payload_bytes =
            listener_ota_v1_data_chunk_payload_bytes(session.as_ref(), data_write_option);
        log::info!(
            "[embedded-ble] Listener OTA v1 data write option={data_write_option:?} chunk_payload_bytes={payload_bytes}"
        );
        Ok(PreparedListenerOtaV1Characteristics {
            control,
            data,
            status,
            data_write_option,
            data_chunk_payload_bytes: payload_bytes,
            session,
        })
    }

    fn open_listener_ota_v1_characteristics_from_service_with_deadline(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
        deadline: Instant,
    ) -> Result<PreparedListenerOtaV1Characteristics, String> {
        if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(
                op,
                remaining_ble_timeout(
                    deadline,
                    BLE_DISCOVERY_TIMEOUT,
                    "Listener OTA v1 service access",
                )
                .ok()?,
                "Listener OTA v1 service access",
            )
            .ok()
        }) {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!(
                    "Listener OTA v1 service access denied status={access:?}"
                ));
            }
        }
        let session = prepare_gatt_session(
            service,
            remaining_ble_timeout(deadline, GATT_READY_TIMEOUT, "Listener OTA v1 GATT session")?,
        )?;
        let control = open_write_characteristic_from_service_with_timeout(
            service,
            LISTENER_OTA_V1_CONTROL_UUID,
            "Listener OTA v1 control",
            cache_mode,
            remaining_ble_timeout(
                deadline,
                BLE_DISCOVERY_TIMEOUT,
                "Listener OTA v1 control characteristic",
            )?,
        )?;
        let data = open_write_characteristic_from_service_with_timeout(
            service,
            LISTENER_OTA_V1_DATA_UUID,
            "Listener OTA v1 data",
            cache_mode,
            remaining_ble_timeout(
                deadline,
                BLE_DISCOVERY_TIMEOUT,
                "Listener OTA v1 data characteristic",
            )?,
        )?;
        let status = if LISTENER_OTA_V1_STATUS_UUID == LISTENER_OTA_V1_CONTROL_UUID {
            control.clone()
        } else {
            open_read_characteristic_from_service_with_timeout(
                service,
                LISTENER_OTA_V1_STATUS_UUID,
                "Listener OTA v1 status",
                cache_mode,
                remaining_ble_timeout(
                    deadline,
                    BLE_DISCOVERY_TIMEOUT,
                    "Listener OTA v1 status characteristic",
                )?,
            )?
        };
        let data_properties = data.CharacteristicProperties().map_err(|err| {
            format!("Listener OTA v1 data characteristic properties read failed: {err}")
        })?;
        let data_write_option = listener_ota_v1_data_write_option(data_properties)?;
        let payload_bytes =
            listener_ota_v1_data_chunk_payload_bytes(session.as_ref(), data_write_option);
        log::info!(
            "[embedded-ble] Listener OTA v1 data write option={data_write_option:?} chunk_payload_bytes={payload_bytes}"
        );
        Ok(PreparedListenerOtaV1Characteristics {
            control,
            data,
            status,
            data_write_option,
            data_chunk_payload_bytes: payload_bytes,
            session,
        })
    }

    fn open_listener_ota_v1_characteristics_from_service_with_retry(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
    ) -> Result<PreparedListenerOtaV1Characteristics, String> {
        let mut last_error = None;
        for attempt in 1..=AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS.len() + 1 {
            match open_listener_ota_v1_characteristics_from_service(service, cache_mode) {
                Ok(prepared) => {
                    if attempt > 1 {
                        log::info!(
                            "[embedded-ble] Listener OTA v1 characteristics recovered via {cache_mode:?} on attempt {attempt}"
                        );
                    }
                    return Ok(prepared);
                }
                Err(err) => {
                    let transient = is_transient_listener_ota_v1_discovery_error(&err);
                    if attempt > AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS.len() || !transient {
                        return Err(err);
                    }
                    let delay = AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS[attempt - 1];
                    log::warn!(
                        "[embedded-ble] Listener OTA v1 characteristic discovery attempt {attempt} via {cache_mode:?} returned transient error: {err}; retrying in {} ms",
                        delay.as_millis()
                    );
                    last_error = Some(err);
                    std::thread::sleep(delay);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "Listener OTA v1 characteristic discovery did not complete".to_string()
        }))
    }

    fn open_listener_ota_v1_characteristics_from_service_with_retry_deadline(
        service: &GattDeviceService,
        cache_mode: BluetoothCacheMode,
        deadline: Instant,
    ) -> Result<PreparedListenerOtaV1Characteristics, String> {
        let mut last_error = None;
        for attempt in 1..=AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS.len() + 1 {
            match open_listener_ota_v1_characteristics_from_service_with_deadline(
                service, cache_mode, deadline,
            ) {
                Ok(prepared) => {
                    if attempt > 1 {
                        log::info!(
                            "[embedded-ble] Listener OTA v1 characteristics recovered via {cache_mode:?} on attempt {attempt}"
                        );
                    }
                    return Ok(prepared);
                }
                Err(err) => {
                    let transient = is_transient_listener_ota_v1_discovery_error(&err);
                    if attempt > AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS.len() || !transient {
                        return Err(err);
                    }
                    let delay = AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS[attempt - 1];
                    let sleep_for = match remaining_ble_timeout(
                        deadline,
                        delay,
                        "Listener OTA v1 characteristic retry delay",
                    ) {
                        Ok(value) => value,
                        Err(_) => return Err(err),
                    };
                    log::warn!(
                        "[embedded-ble] Listener OTA v1 characteristic discovery attempt {attempt} via {cache_mode:?} returned transient error: {err}; retrying in {} ms",
                        sleep_for.as_millis()
                    );
                    last_error = Some(err);
                    std::thread::sleep(sleep_for);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "Listener OTA v1 characteristic discovery did not complete".to_string()
        }))
    }

    fn is_transient_listener_ota_v1_discovery_error(err: &str) -> bool {
        err.contains("GattCommunicationStatus(1)")
            || err.contains("GattCommunicationStatus(3)")
            || err.contains("Unreachable")
            || err.contains("unreachable")
            || err.contains("timed out")
            || err.contains("timeout")
            || err.contains("GATT session did not become active")
            || err.contains("characteristic discovery returned status")
            || err.contains("characteristic discovery wait failed")
            || err.contains("service access")
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

    fn listener_ota_v1_data_write_option(
        properties: GattCharacteristicProperties,
    ) -> Result<GattWriteOption, String> {
        if properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
            Ok(GattWriteOption::WriteWithoutResponse)
        } else if properties.contains(GattCharacteristicProperties::Write) {
            Ok(GattWriteOption::WriteWithResponse)
        } else {
            Err("Listener OTA v1 data characteristic is not writable".to_string())
        }
    }

    fn listener_ota_v1_data_chunk_payload_bytes(
        session: Option<&GattSession>,
        write_option: GattWriteOption,
    ) -> usize {
        let desired_payload =
            LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES + denzic_ota_core::DATA_HEADER_BYTES;
        let mut payload_bytes = session
            .and_then(|session| session.MaxPduSize().ok())
            .map(|max_pdu_size| usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES))
            .filter(|payload_bytes| *payload_bytes > denzic_ota_core::DATA_HEADER_BYTES)
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
                    .filter(|payload_bytes| *payload_bytes > denzic_ota_core::DATA_HEADER_BYTES)
                {
                    payload_bytes = payload_bytes.max(next_payload);
                }
            }
            if payload_bytes < desired_payload {
                log::warn!(
                    "[embedded-ble] Listener OTA v1 MaxPduSize stayed at payload_bytes={payload_bytes}; using {LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES} byte payload for WriteWithoutResponse and relying on WinRT write status"
                );
                return LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES;
            }
        }

        payload_bytes
            .saturating_sub(denzic_ota_core::DATA_HEADER_BYTES)
            .min(LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES)
            .max(1)
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
        open_write_characteristic_from_service_with_timeout(
            service,
            uuid,
            label,
            cache_mode,
            BLE_DISCOVERY_TIMEOUT,
        )
    }

    fn open_write_characteristic_from_service_with_timeout(
        service: &GattDeviceService,
        uuid: GUID,
        label: &str,
        cache_mode: BluetoothCacheMode,
        timeout: Duration,
    ) -> Result<GattCharacteristic, String> {
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(uuid, cache_mode)
            .map_err(|err| format!("BLE {label} characteristic discovery failed: {err}"))?
            .wait_ble_result(timeout, &format!("{label} characteristic"))?;
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
            .wait_ble_result(BLE_DISCOVERY_TIMEOUT, &format!("{label} characteristic"))?;
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
        open_read_characteristic_from_service_with_timeout(
            service,
            uuid,
            label,
            cache_mode,
            BLE_DISCOVERY_TIMEOUT,
        )
    }

    fn open_read_characteristic_from_service_with_timeout(
        service: &GattDeviceService,
        uuid: GUID,
        label: &str,
        cache_mode: BluetoothCacheMode,
        timeout: Duration,
    ) -> Result<GattCharacteristic, String> {
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(uuid, cache_mode)
            .map_err(|err| format!("BLE {label} characteristic discovery failed: {err}"))?
            .wait_ble_result(timeout, &format!("{label} characteristic"))?;
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
        open_notify_characteristic_by_uuid_from_service_with_timeout(
            service,
            uuid,
            label,
            cache_mode,
            BLE_DISCOVERY_TIMEOUT,
        )
    }

    fn open_notify_characteristic_by_uuid_from_service_with_timeout(
        service: &GattDeviceService,
        uuid: GUID,
        label: &str,
        cache_mode: BluetoothCacheMode,
        timeout: Duration,
    ) -> Result<GattCharacteristic, String> {
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(uuid, cache_mode)
            .map_err(|err| format!("BLE {label} characteristic discovery failed: {err}"))?
            .wait_ble_result(timeout, &format!("{label} characteristic"))?;
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
            .wait_ble_result(BLE_DISCOVERY_TIMEOUT, "notify characteristic")?;
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

    fn open_notify_characteristic_from_service_for_startup_fast_path(
        service: &GattDeviceService,
        deadline: Instant,
    ) -> Result<PreparedNotifyCharacteristic, String> {
        if let Ok(operation) = service.RequestAccessAsync() {
            if let Some(access) = wait_async_operation(
                operation,
                remaining_ble_timeout(
                    deadline,
                    STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT,
                    "persisted startup service access",
                )?,
                "persisted startup service access",
            )
            .ok()
            {
                if access != DeviceAccessStatus::Allowed
                    && access != DeviceAccessStatus::Unspecified
                {
                    return Err(format!("BLE service access denied status={access:?}"));
                }
            }
        }
        let session = prepare_gatt_session(
            service,
            remaining_ble_timeout(
                deadline,
                STARTUP_NOTIFY_FAST_PATH_GATT_TIMEOUT,
                "persisted startup GATT ready",
            )?,
        )?;
        let control = open_write_characteristic_from_service_with_timeout(
            service,
            AUDIO_CONTROL_UUID,
            "persisted startup audio control",
            BluetoothCacheMode::Cached,
            remaining_ble_timeout(
                deadline,
                STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT,
                "persisted startup audio control characteristic",
            )?,
        )?;
        let characteristic = open_notify_characteristic_by_uuid_from_service_with_timeout(
            service,
            NOTIFY_UUID,
            "persisted startup notify",
            BluetoothCacheMode::Cached,
            remaining_ble_timeout(
                deadline,
                STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT,
                "persisted startup notify characteristic",
            )?,
        )?;
        Ok(PreparedNotifyCharacteristic {
            characteristic,
            control: Some(control),
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

    fn configured_bluetooth_address_from_env() -> Option<u64> {
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

    fn configured_bluetooth_address() -> Option<u64> {
        configured_bluetooth_address_from_env().or_else(runtime_bluetooth_target_address)
    }

    fn runtime_bluetooth_target_address() -> Option<u64> {
        let target_name = effective_bluetooth_target_name(None);
        let now = Instant::now();
        let mut slot = RUNTIME_BLUETOOTH_TARGET_ADDRESS
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()?;
        let state = slot.as_ref()?.clone();
        if now.saturating_duration_since(state.learned_at) >= state.valid_for {
            *slot = None;
            return None;
        }
        if !bluetooth_name_matches_expected(&state.target_name, &target_name) {
            return None;
        }
        Some(state.address)
    }

    pub(super) fn verified_bluetooth_target_rename_handoff_address() -> Option<u64> {
        let target_name = effective_bluetooth_target_name(None);
        let now = Instant::now();
        let mut slot = RUNTIME_BLUETOOTH_TARGET_ADDRESS
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()?;
        let state = slot.as_ref()?.clone();
        if now.saturating_duration_since(state.learned_at) >= state.valid_for {
            *slot = None;
            return None;
        }
        if state.valid_for != BLE_RENAME_ADDRESS_GRACE_WINDOW
            || !bluetooth_name_matches_expected(&state.target_name, &target_name)
        {
            return None;
        }
        log::info!(
            "[embedded-ble] using verified Listener BLE rename handoff address {} target={target_name:?}",
            crate::embedded_ble::format_bluetooth_address(state.address)
        );
        Some(state.address)
    }

    fn ble_device_state_path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        Some(
            PathBuf::from(appdata)
                .join("Listener Type")
                .join(BLE_DEVICE_STATE_FILE),
        )
    }

    fn persisted_ble_device_state_address_for_target(
        state: &PersistedBleDeviceState,
        expected_target_name: &str,
    ) -> Option<u64> {
        let stored_target = state
            .target_name
            .as_deref()
            .and_then(normalize_bluetooth_target_name)?;
        if !bluetooth_name_matches_expected(&stored_target, expected_target_name) {
            return None;
        }
        state
            .last_successful_address
            .as_deref()
            .and_then(parse_bluetooth_address_hex)
    }

    fn persisted_successful_notify_target_address_for_current() -> Option<u64> {
        if configured_bluetooth_address_from_env().is_some() {
            return None;
        }
        let path = ble_device_state_path()?;
        let text = fs::read_to_string(&path).ok()?;
        let state: PersistedBleDeviceState = serde_json::from_str(&text).ok()?;
        let target_name = effective_bluetooth_target_name(None);
        let address = persisted_ble_device_state_address_for_target(&state, &target_name)?;
        log::info!(
            "[embedded-ble] persisted Listener BLE notify target candidate address={} target={target_name:?}",
            crate::embedded_ble::format_bluetooth_address(address)
        );
        Some(address)
    }

    fn persist_successful_notify_target_address(address: u64, context: &str) {
        let target_name = effective_bluetooth_target_name(None);
        let Some(path) = ble_device_state_path() else {
            return;
        };
        let state = PersistedBleDeviceState {
            last_successful_address: Some(crate::embedded_ble::format_bluetooth_address(address)),
            target_name: Some(target_name.clone()),
            updated_at: Some(super::utc_now_rfc3339()),
        };
        let Some(parent) = path.parent() else {
            return;
        };
        if let Err(err) = fs::create_dir_all(parent) {
            log::warn!(
                "[embedded-ble] failed to create BLE device state dir {}: {err}",
                parent.display()
            );
            return;
        }
        let bytes = match serde_json::to_vec_pretty(&state) {
            Ok(bytes) => bytes,
            Err(err) => {
                log::warn!("[embedded-ble] failed to encode BLE device state: {err}");
                return;
            }
        };
        if let Err(err) = fs::write(&path, bytes) {
            log::warn!(
                "[embedded-ble] failed to persist BLE notify target address to {}: {err}",
                path.display()
            );
            return;
        }
        log::info!(
            "[embedded-ble] persisted Listener BLE notify target address={} target={target_name:?} context={context}",
            crate::embedded_ble::format_bluetooth_address(address)
        );
    }

    fn has_active_runtime_bluetooth_target_address(now: Instant) -> bool {
        let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_ADDRESS
            .get_or_init(|| Mutex::new(None))
            .lock()
        else {
            return false;
        };
        let Some(state) = slot.as_ref() else {
            return false;
        };
        if now.saturating_duration_since(state.learned_at) >= state.valid_for {
            *slot = None;
            return false;
        }
        true
    }

    fn remember_runtime_bluetooth_target_address_for_name(
        address: u64,
        target_name: &str,
        valid_for: Duration,
        context: &str,
    ) {
        let Some(target_name) = normalize_bluetooth_target_name(target_name) else {
            return;
        };
        let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_ADDRESS
            .get_or_init(|| Mutex::new(None))
            .lock()
        else {
            log::warn!("[embedded-ble] target Bluetooth address slot is poisoned");
            return;
        };
        let unchanged = slot
            .as_ref()
            .is_some_and(|state| state.address == address && state.target_name == target_name);
        *slot = Some(RuntimeBluetoothTargetAddress {
            address,
            target_name: target_name.clone(),
            learned_at: Instant::now(),
            valid_for,
        });
        if !unchanged {
            log::info!(
                "[embedded-ble] remembered Listener BLE address {} for target={target_name:?} context={context}",
                crate::embedded_ble::format_bluetooth_address(address)
            );
        }
    }

    fn remember_runtime_bluetooth_target_address_for_current(address: u64, context: &str) {
        let target_name = effective_bluetooth_target_name(None);
        remember_runtime_bluetooth_target_address_for_name(
            address,
            &target_name,
            BLE_TARGET_ADDRESS_CACHE_WINDOW,
            context,
        );
    }

    fn remember_runtime_bluetooth_target_address_for_candidate(
        address: u64,
        candidate_name: &str,
        context: &str,
    ) {
        let target_name = match (
            configured_bluetooth_target_name(),
            normalize_bluetooth_target_name(candidate_name),
        ) {
            (Some(configured), Some(candidate))
                if bluetooth_name_matches_expected(&configured, DEFAULT_BLUETOOTH_TARGET_NAME)
                    && !bluetooth_name_matches_expected(
                        &candidate,
                        DEFAULT_BLUETOOTH_TARGET_NAME,
                    ) =>
            {
                set_configured_bluetooth_target_name(&candidate);
                candidate
            }
            (Some(configured), _) => configured,
            (None, Some(candidate)) => {
                set_configured_bluetooth_target_name(&candidate);
                candidate
            }
            (None, None) => DEFAULT_BLUETOOTH_TARGET_NAME.to_string(),
        };
        remember_runtime_bluetooth_target_address_for_name(
            address,
            &target_name,
            BLE_TARGET_ADDRESS_CACHE_WINDOW,
            context,
        );
    }

    pub fn set_configured_bluetooth_target_name(name: &str) {
        let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_NAME
            .get_or_init(|| Mutex::new(None))
            .lock()
        else {
            log::warn!("[embedded-ble] target Bluetooth name slot is poisoned");
            return;
        };
        *slot = normalize_bluetooth_target_name(name);
    }

    fn configured_bluetooth_target_name() -> Option<String> {
        for key in [
            "LISTENER_TYPE_BLE_TARGET_NAME",
            "LISTENER_TYPE_BLUETOOTH_TARGET_NAME",
        ] {
            let Ok(value) = std::env::var(key) else {
                continue;
            };
            if let Some(name) = normalize_bluetooth_target_name(&value) {
                return Some(name);
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

    fn normalize_bluetooth_target_name(name: &str) -> Option<String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
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

    fn default_target_name_can_accept_candidate(
        expected_name: &str,
        candidate_name: &str,
        address: Option<u64>,
    ) -> bool {
        address.is_some()
            && bluetooth_name_matches_expected(expected_name, DEFAULT_BLUETOOTH_TARGET_NAME)
            && normalize_bluetooth_target_name(candidate_name).is_some()
            && !bluetooth_name_matches_expected(candidate_name, DEFAULT_BLUETOOTH_TARGET_NAME)
    }

    fn ble_candidate_allowed(kind: &str, index: u32, name: &str, address: Option<u64>) -> bool {
        let mut address_matched = false;
        if let Some(expected_address) = configured_bluetooth_address_from_env() {
            if address != Some(expected_address) {
                log::info!(
                    "[embedded-ble] skipping {kind} candidate index={index} name={name} address={}: pinned env-configured address is {}",
                    address
                        .map(crate::embedded_ble::format_bluetooth_address)
                        .unwrap_or_else(|| "-".to_string()),
                    crate::embedded_ble::format_bluetooth_address(expected_address)
                );
                return false;
            }
            address_matched = true;
        }

        if !address_matched {
            if let Some(expected_name) = configured_bluetooth_target_name() {
                if !name.trim().eq_ignore_ascii_case(&expected_name) {
                    if default_target_name_can_accept_candidate(&expected_name, name, address) {
                        log::warn!(
                            "[embedded-ble] allowing {kind} candidate index={index} name={name:?} address={} because local target is default {expected_name:?}; accepting discovered Listener service name",
                            address
                                .map(crate::embedded_ble::format_bluetooth_address)
                                .unwrap_or_else(|| "-".to_string())
                        );
                        return true;
                    }
                    if let Some(address) = address {
                        let trusted_addresses = listener_recovery_target_addresses();
                        if trusted_addresses.contains(&address) {
                            log::warn!(
                                "[embedded-ble] allowing {kind} candidate index={index} name={name:?} address={} despite target name {expected_name:?} because Windows PnP/service signature still points to the same Listener address",
                                crate::embedded_ble::format_bluetooth_address(address)
                            );
                            return true;
                        }
                    }
                    log::info!(
                    "[embedded-ble] skipping {kind} candidate index={index} name={name:?} address={}: target name is {expected_name:?}",
                    address
                        .map(crate::embedded_ble::format_bluetooth_address)
                        .unwrap_or_else(|| "-".to_string())
                );
                    return false;
                }
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

    fn advertisement_manufacturer_data_summary(advertisement: &BluetoothLEAdvertisement) -> String {
        let Ok(manufacturer_data) = advertisement.ManufacturerData() else {
            return "mfg=unavailable".to_string();
        };
        let Ok(count) = manufacturer_data.Size() else {
            return "mfg=size-unavailable".to_string();
        };
        if count == 0 {
            return "mfg=none".to_string();
        }

        let mut entries = Vec::new();
        for index in 0..count {
            let Ok(entry) = manufacturer_data.GetAt(index) else {
                entries.push(format!("index={index}:unreadable"));
                continue;
            };
            let company_id = entry.CompanyId().unwrap_or_default();
            let bytes = entry
                .Data()
                .ok()
                .and_then(|buffer| buffer_to_vec(&buffer).ok())
                .unwrap_or_default();
            entries.push(format!(
                "company=0x{company_id:04X} data={}",
                hex_bytes(&bytes)
            ));
        }
        format!("mfg=[{}]", entries.join(";"))
    }

    fn advertisement_swift_pair_display_name(
        advertisement: &BluetoothLEAdvertisement,
    ) -> Option<String> {
        let manufacturer_data = advertisement.ManufacturerData().ok()?;
        let count = manufacturer_data.Size().ok()?;
        for index in 0..count {
            let entry = manufacturer_data.GetAt(index).ok()?;
            let company_id = entry.CompanyId().ok()?;
            let bytes = entry
                .Data()
                .ok()
                .and_then(|buffer| buffer_to_vec(&buffer).ok())
                .unwrap_or_default();
            if let Some(name) = swift_pair_display_name_from_manufacturer_entry(company_id, &bytes)
            {
                return Some(name);
            }
        }
        None
    }

    fn swift_pair_display_name_from_manufacturer_entry(
        company_id: u16,
        bytes: &[u8],
    ) -> Option<String> {
        if company_id != 0x0006 {
            return None;
        }

        let payload = if bytes.len() >= 5 && bytes[0] == 0x06 && bytes[1] == 0x00 {
            &bytes[2..]
        } else {
            bytes
        };
        if payload.len() <= 3 || payload[0] != 0x03 {
            return None;
        }

        let name = String::from_utf8_lossy(&payload[3..])
            .trim_matches(char::from(0))
            .trim()
            .to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return "-".to_string();
        }
        bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join("")
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
        read_characteristic_bytes_with_timeout(
            characteristic,
            cache_mode,
            label,
            BLE_DISCOVERY_TIMEOUT,
        )
    }

    fn read_characteristic_bytes_with_timeout(
        characteristic: &GattCharacteristic,
        cache_mode: BluetoothCacheMode,
        label: &str,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let read = characteristic
            .ReadValueWithCacheModeAsync(cache_mode)
            .map_err(|err| format!("BLE {label} read failed: {err}"))?
            .wait_ble_result(timeout, &format!("{label} read"))
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
        recovery_probe_address: Option<u64>,
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
                    if let Some(recovery_error) =
                        cccd_notify_recovery_pairing_error(&err, recovery_probe_address)
                    {
                        log::warn!(
                            "[embedded-ble] {label} #{capture_id}: notify CCCD enable attempt {attempt} hit recovery pairing window; entering Type PairAsync recovery instead of retrying CCCD: {err}"
                        );
                        return Err(recovery_error);
                    }
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

    fn cccd_notify_recovery_pairing_error(
        err: &str,
        recovery_probe_address: Option<u64>,
    ) -> Option<String> {
        if !err.contains("0x800704C7") {
            return None;
        }
        let address = recovery_probe_address?;
        if !recovery_swift_pair_advertisement_visible_for_persisted_address(address) {
            return None;
        }
        Some(format!(
            "Listener recovery Swift Pair advertisement visible for notify CCCD address {address:012X} after {err}; missing pairing must use Type automatic PairAsync recovery before declaring notify ready"
        ))
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

    trait AsyncOperationTimeoutExt<T: windows::core::RuntimeType> {
        fn wait_ble_result(self, timeout: Duration, label: &str) -> Result<T, String>;
    }

    impl<T: windows::core::RuntimeType> AsyncOperationTimeoutExt<T> for IAsyncOperation<T> {
        fn wait_ble_result(self, timeout: Duration, label: &str) -> Result<T, String> {
            wait_async_operation(self, timeout, label)
        }
    }

    fn wait_async_operation<T: windows::core::RuntimeType>(
        operation: IAsyncOperation<T>,
        timeout: Duration,
        label: &str,
    ) -> Result<T, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if notify_capture_cancel_requested() {
                let _ = operation.Cancel();
                let _ = operation.Close();
                return Err(notify_capture_cancelled_error(label));
            }
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
            NOTIFY_CAPTURE_SESSION_ACTIVE.store(true, Ordering::SeqCst);
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
            NOTIFY_CAPTURE_SESSION_ACTIVE.store(false, Ordering::SeqCst);
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
        bluetooth_address: Option<u64>,
    }

    struct OpenAudioControlTarget {
        control: GattCharacteristic,
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
    }

    struct OpenEmbeddedAudioStatusTarget {
        service: GattDeviceService,
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

    struct PreparedDiagnosticCharacteristics {
        control: GattCharacteristic,
        data: GattCharacteristic,
        count: GattCharacteristic,
        session: Option<GattSession>,
    }

    impl Drop for OpenListenerOtaV1Target {
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

        fn finish_after_caller_cancel(
            &mut self,
            terminal_behavior: CaptureTerminalBehavior,
            controlled_connection_handoff: bool,
        ) {
            let teardown = NotifyCccdTeardown::for_capture_cancel(
                terminal_behavior,
                ble_ota_process_mutex_busy(),
                controlled_connection_handoff,
            );
            self.finish(teardown);
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
                        "[embedded-ble] capture #{}: leaving notify CCCD enabled for controlled connection handoff",
                        self.capture_id
                    );
                }
            }
            let _ = self.audio_control_registration.take();
            self.notify_disabled = true;
        }

        fn handle_audio_control_request(&self, request: AudioControlRequest) {
            let queued_ms = request.queued_at.elapsed().as_millis();
            if request.label == "audio control stop" || queued_ms >= 50 {
                log::info!(
                    "[embedded-ble] active audio control dispatch label={} queued_ms={}",
                    request.label,
                    queued_ms
                );
            }
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

        fn drain_audio_control_requests(&self, rx: &mpsc::Receiver<AudioControlRequest>) {
            while let Ok(request) = rx.try_recv() {
                self.handle_audio_control_request(request);
            }
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
        let (primary, fallback) = audio_control_write_options(control, bytes, label)?;
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
        bytes: &[u8],
        label: &str,
    ) -> Result<(GattWriteOption, Option<GattWriteOption>), String> {
        let properties = control
            .CharacteristicProperties()
            .map_err(|err| format!("BLE {label} characteristic properties read failed: {err}"))?;
        audio_control_write_options_from_properties(properties, audio_control_write_policy(bytes))
            .ok_or_else(|| {
                "BLE audio control characteristic must support Write or WriteWithoutResponse."
                    .to_string()
            })
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum AudioControlWritePolicy {
        LowLatency,
        Reliable,
    }

    fn audio_control_write_policy(bytes: &[u8]) -> AudioControlWritePolicy {
        if bytes == b"VREC:TOGGLE\n" || bytes == b"VREC:STOP\n" {
            AudioControlWritePolicy::LowLatency
        } else {
            AudioControlWritePolicy::Reliable
        }
    }

    fn audio_control_write_options_from_properties(
        properties: GattCharacteristicProperties,
        policy: AudioControlWritePolicy,
    ) -> Option<(GattWriteOption, Option<GattWriteOption>)> {
        let supports_write = properties.contains(GattCharacteristicProperties::Write);
        let supports_without_response =
            properties.contains(GattCharacteristicProperties::WriteWithoutResponse);
        if policy == AudioControlWritePolicy::LowLatency && supports_without_response {
            return Some((
                GattWriteOption::WriteWithoutResponse,
                supports_write.then_some(GattWriteOption::WriteWithResponse),
            ));
        }
        if supports_write {
            return Some((
                GattWriteOption::WriteWithResponse,
                supports_without_response.then_some(GattWriteOption::WriteWithoutResponse),
            ));
        }
        if supports_without_response {
            return Some((GattWriteOption::WriteWithoutResponse, None));
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

        fn for_capture_cancel(
            terminal_behavior: CaptureTerminalBehavior,
            ota_process_busy: bool,
            controlled_connection_handoff: bool,
        ) -> Self {
            if terminal_behavior == CaptureTerminalBehavior::ContinueListening
                && (ota_process_busy || controlled_connection_handoff)
            {
                Self::LeaveEnabled
            } else {
                Self::Disable
            }
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

        fn bluetooth_target_name_test_lock() -> &'static Mutex<()> {
            static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            LOCK.get_or_init(|| Mutex::new(()))
        }

        #[test]
        fn background_capture_cancel_scope_is_visible_to_winrt_waits() {
            let cancel = Arc::new(AtomicBool::new(false));
            assert!(!notify_capture_cancel_requested());

            let scope = NotifyCaptureCancelScope::install(&cancel);
            assert!(!notify_capture_cancel_requested());
            cancel.store(true, Ordering::SeqCst);
            assert!(notify_capture_cancel_requested());

            drop(scope);
            assert!(!notify_capture_cancel_requested());
        }

        #[test]
        fn background_capture_cancellation_interrupts_target_open_before_fallback_scans() {
            let source = include_str!("embedded_ble.rs");
            let open_start = source
                .find("fn open_notify_target()")
                .expect("notify target helper should exist");
            let open_end = source[open_start..]
                .find("fn open_notify_target_with_retry")
                .map(|offset| open_start + offset)
                .expect("notify target helper boundary should exist");
            let open_body = &source[open_start..open_end];
            assert!(
                open_body.contains("if notify_capture_cancel_requested() {\n                        return Err(err);"),
                "a cancelled recent-pairing open must stop before service-selector or advertisement fallbacks"
            );

            let wait_start = source
                .find("fn wait_async_operation<T")
                .expect("WinRT async wait helper should exist");
            let wait_end = source[wait_start..]
                .find("fn wait_gatt_write_result")
                .map(|offset| wait_start + offset)
                .expect("WinRT async wait helper boundary should exist");
            let wait_body = &source[wait_start..wait_end];
            assert!(wait_body.contains("notify_capture_cancel_requested()"));
            assert!(wait_body.contains("operation.Cancel()"));
        }

        #[test]
        fn continuous_listener_skips_notify_cccd_prereset() {
            assert!(!notify_cccd_prereset_required(
                CaptureTerminalBehavior::ContinueListening
            ));
            assert!(notify_cccd_prereset_required(
                CaptureTerminalBehavior::StopCapture
            ));
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
        fn audio_control_toggle_prefers_no_response_when_available() {
            let both = GattCharacteristicProperties::Write
                | GattCharacteristicProperties::WriteWithoutResponse;
            assert_eq!(
                audio_control_write_options_from_properties(
                    both,
                    AudioControlWritePolicy::LowLatency
                ),
                Some((
                    GattWriteOption::WriteWithoutResponse,
                    Some(GattWriteOption::WriteWithResponse)
                ))
            );
            assert_eq!(
                audio_control_write_options_from_properties(
                    GattCharacteristicProperties::Write,
                    AudioControlWritePolicy::LowLatency
                ),
                Some((GattWriteOption::WriteWithResponse, None))
            );
            assert_eq!(
                audio_control_write_options_from_properties(
                    GattCharacteristicProperties::WriteWithoutResponse,
                    AudioControlWritePolicy::LowLatency
                ),
                Some((GattWriteOption::WriteWithoutResponse, None))
            );
        }

        #[test]
        fn recording_stop_prefers_no_response_when_available() {
            assert_eq!(
                audio_control_write_policy(b"VREC:STOP\n"),
                AudioControlWritePolicy::LowLatency
            );
        }

        #[test]
        fn reliable_audio_control_prefers_with_response_when_available() {
            let both = GattCharacteristicProperties::Write
                | GattCharacteristicProperties::WriteWithoutResponse;
            assert_eq!(
                audio_control_write_options_from_properties(
                    both,
                    AudioControlWritePolicy::Reliable
                ),
                Some((
                    GattWriteOption::WriteWithResponse,
                    Some(GattWriteOption::WriteWithoutResponse)
                ))
            );
            assert_eq!(
                audio_control_write_options_from_properties(
                    GattCharacteristicProperties::Write,
                    AudioControlWritePolicy::Reliable
                ),
                Some((GattWriteOption::WriteWithResponse, None))
            );
            assert_eq!(
                audio_control_write_options_from_properties(
                    GattCharacteristicProperties::WriteWithoutResponse,
                    AudioControlWritePolicy::Reliable
                ),
                Some((GattWriteOption::WriteWithoutResponse, None))
            );
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
        fn recovery_pairing_prefers_direct_address_before_slow_windows_aep_fallback() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_pairing_candidates")
                .expect("recovery pairing candidate function should exist");
            let body = &source[start..];
            let direct_index = body
                .find("listener_recovery_direct_pairing_candidates")
                .expect("recovery pairing should try direct address candidates");
            let direct_return_index = body
                .find("return Ok(candidates);")
                .expect("recovery pairing should immediately use direct address candidates");
            let selector_index = body
                .find("listener_pairing_candidates_from_unpaired_selector")
                .expect("recovery pairing must keep Windows AEP fallback");

            assert!(
                direct_index < direct_return_index && direct_return_index < selector_index,
                "recovery pairing must attempt fresh direct BLE address candidates immediately instead of waiting on slow Windows AEP fallback"
            );
        }

        #[test]
        fn recovery_pairing_passes_advertised_addresses_into_windows_selector() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_pairing_candidates")
                .expect("recovery pairing candidate function should exist");
            let body = &source[start..];
            let scan_index = body
                .find("scan_listener_pairing_advertisements")
                .expect("recovery pairing must scan advertisements");
            let selector_index = body
                .find(
                    "listener_pairing_candidates_from_unpaired_selector(expected_name, direct_addresses)",
                )
                .expect(
                    "recovery pairing must pass freshly advertised addresses into Windows selector",
                );

            assert!(
                body.contains("let direct_addresses = if fresh_advertised_addresses.is_empty()"),
                "recovery pairing must use fresh advertised addresses as the selector filter when present"
            );
            assert!(
                scan_index < selector_index,
                "recovery pairing must still pass freshly advertised BLE addresses into Windows selector filtering when direct lookup cannot produce a candidate"
            );
        }

        #[test]
        fn recovery_pairing_limits_direct_candidates_to_fresh_advertisement_when_available() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_pairing_candidates_for_addresses")
                .expect("recovery pairing candidate function should exist");
            let end = source[start..]
                .find("fn listener_recovery_pairing_selector_fallback_candidates")
                .map(|offset| start + offset)
                .expect("recovery pairing selector fallback boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("fresh_advertised_addresses.as_slice()"));
            assert!(body.contains("recovery pairing using fresh advertised address(es)"));
            assert!(
                body.contains(
                    "listener_recovery_direct_pairing_candidates(\n            direct_addresses,"
                ),
                "direct pairing must not try stale configured/PnP addresses ahead of the current recovery advertisement"
            );
        }

        #[test]
        fn type_observed_recovery_address_skips_duplicate_pairing_advertisement_scan() {
            let source = include_str!("embedded_ble.rs");
            let helper_start = source
                .find("fn listener_recovery_pairing_candidates_for_addresses")
                .expect("recovery pairing candidate helper should exist");
            let helper_end = source[helper_start..]
                .find("fn listener_recovery_pairing_selector_fallback_candidates")
                .map(|offset| helper_start + offset)
                .expect("recovery pairing helper boundary should exist");
            let helper = &source[helper_start..helper_end];
            let observed_index = helper
                .find("observed_recovery_addresses")
                .expect("Type-observed recovery addresses must enter candidate selection");
            let direct_index = helper
                .find("recovery pairing using Type-observed Swift Pair address(es)")
                .expect("Type-observed addresses should be logged as the first candidate source");
            let scan_index = helper
                .find("scan_listener_pairing_advertisements")
                .expect("ordinary fallback advertisement scan should remain");
            assert!(
                observed_index < direct_index && direct_index < scan_index,
                "once Type has observed the recovery advertisement address, PairAsync must use that address before any duplicate scan"
            );
            assert!(
                helper.contains(
                    "let mut addresses = if observed_recovery_addresses.is_empty() {\n            listener_recovery_target_addresses()\n        } else {\n            Vec::new()\n        };"
                ),
                "a current recovery advertisement address must bypass the redundant PnP address enumeration before direct PairAsync"
            );
            assert!(helper.contains(
                "recovery pairing using {} Type-observed direct address candidate(s) before slow advertisement/AEP discovery"
            ));

            let prompt_start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("pairing prompt helper should exist");
            let prompt_end = source[prompt_start..]
                .find("let mut result = crate::embedded_ble::BleDevicePairingPromptResult")
                .map(|offset| prompt_start + offset)
                .expect("pairing prompt candidate boundary should exist");
            let prompt = &source[prompt_start..prompt_end];
            assert!(
                prompt.contains("if candidate.fresh_pairing_advertisement {\n                    continue;\n                }")
                    && prompt.contains(
                        "trusted_addresses.get_or_insert_with(listener_recovery_target_addresses)"
                    ),
                "a freshly advertised candidate must not pay another PnP trust lookup; stale candidates still require the existing trusted-address check"
            );

            let fallback_start = source
                .find("fn listener_recovery_pairing_selector_fallback_candidates_for_addresses")
                .expect("recovery AEP fallback helper should exist");
            let fallback_end = source[fallback_start..]
                .find("fn listener_recovery_direct_pairing_candidates")
                .map(|offset| fallback_start + offset)
                .expect("recovery AEP fallback boundary should exist");
            let fallback = &source[fallback_start..fallback_end];
            assert!(fallback.contains(
                "recovery AEP fallback using Type-observed Swift Pair address(es) without duplicate advertisement scan"
            ));
            assert!(fallback.contains("if fresh_advertised_addresses.is_empty()"));
        }

        #[test]
        fn recovery_pairing_fallback_does_not_use_stale_same_name_cache_when_fresh_address_exists()
        {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_pairing_selector_fallback_candidates")
                .expect("recovery pairing selector fallback should exist");
            let end = source[start..]
                .find("fn listener_recovery_direct_pairing_candidates")
                .map(|offset| start + offset)
                .expect("recovery direct pairing boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("let mut fresh_advertised_addresses = Vec::new();"));
            assert!(
                body.contains("let selector_addresses = if fresh_advertised_addresses.is_empty()")
            );
            assert!(body.contains("not falling back to stale same-name cache"));
        }

        #[test]
        fn recovery_pairing_fast_direct_failure_uses_slow_aep_fallback() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("pairing prompt helper should exist");
            let end = source[start..]
                .find("fn pair_listener_candidates_into_prompt_result")
                .map(|offset| start + offset)
                .expect("pairing prompt result helper should exist");
            let body = &source[start..end];

            assert!(source.contains("BLE_PAIRING_FAST_FAILURE_AEP_FALLBACK_THRESHOLD"));
            assert!(body.contains("fast_recovery_pairing_failure"));
            assert!(body.contains("listener_recovery_pairing_selector_fallback_candidates"));
            assert!(body.contains("failed_before_fallback"));
            assert!(
                body.contains("saturating_sub(failed_before_fallback)"),
                "if slow AEP fallback succeeds after an immediate direct PairAsync failure, the direct failure must not keep the final prompt result failed"
            );
        }

        #[test]
        fn recovery_pairing_fast_aep_fallback_is_bounded() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_pairing_selector_fallback_candidates")
                .expect("recovery pairing selector fallback should exist");
            let end = source[start..]
                .find("fn listener_recovery_direct_pairing_candidates")
                .map(|offset| start + offset)
                .expect("recovery direct pairing boundary should exist");
            let body = &source[start..end];

            assert!(source.contains("BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT"));
            assert!(
                body.contains("listener_pairing_candidates_from_unpaired_selector_with_timeout")
                    && body.contains("BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT"),
                "rename/Type automatic recovery must not wait the full Windows pairing discovery timeout after direct PairAsync fails"
            );
            assert!(
                source.contains("const BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6)"),
                "keep the no-user-prompt recovery fallback bounded so full BLE rename recovery does not regress toward 45s"
            );
        }

        #[test]
        fn recovery_pairing_marks_fresh_only_from_current_advertisement_scan() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_pairing_candidates")
                .expect("recovery pairing candidate function should exist");
            let end = source[start..]
                .find("fn listener_recovery_direct_pairing_candidates")
                .map(|offset| start + offset)
                .expect("direct pairing helper should exist");
            let body = &source[start..end];

            assert!(body.contains("let mut fresh_advertised_addresses = Vec::new();"));
            assert!(body.contains("push_unique_address(&mut fresh_advertised_addresses, address);"));
            assert!(body.contains("&fresh_advertised_addresses"));
        }

        #[test]
        fn direct_pairing_does_not_mark_pnp_address_as_fresh_advertisement() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_direct_pairing_candidates")
                .expect("direct pairing helper should exist");
            let end = source[start..]
                .find("fn push_listener_pairing_candidate_if_matching")
                .map(|offset| start + offset)
                .expect("direct pairing helper boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("fresh_advertised_addresses: &[u64]"));
            assert!(
                body.contains("fresh_pairing_advertisement: fresh_advertised_addresses.contains(&address)"),
                "PnP/configured address lookup alone must not be treated as a fresh recovery advertisement"
            );
            assert!(
                !body.contains("fresh_pairing_advertisement: true"),
                "direct address lookup must not blindly allow destructive stale-cache unpair"
            );
        }

        #[test]
        fn embedded_audio_status_probe_uses_bounded_status_path_not_notify_capture() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("pub fn read_embedded_audio_status(\n        timeout: Duration,")
                .expect("Windows status probe entry should exist");
            let end = source[start..]
                .find("pub(super) fn is_transient_notify_target_open_error")
                .map(|offset| start + offset)
                .expect("status probe body boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("open_embedded_audio_status_target(timeout)"),
                "status reads must use the bounded status-only target path"
            );
            assert!(
                source.contains("let fresh_status_read = readiness.is_some() || capabilities.is_some();")
                    && source.contains("connected: fresh_status_read"),
                "cached Windows service visibility must not be reported as a live BLE link without a fresh status read"
            );
            assert!(
                source.contains("fn read_embedded_audio_status_strings_with_recovery(")
                    && source.contains("status readiness recovered on attempt")
                    && source.contains("status capabilities recovered on attempt"),
                "status reads must retry transient Windows GATT discovery/read failures within the bounded status timeout"
            );
            assert!(
                !body.contains("open_notify_target()?"),
                "status reads must not open the full notify/capture target because Windows GATT discovery can queue for long periods"
            );
            assert!(source.contains("fn remaining_ble_timeout("));
            assert!(source.contains("fn open_embedded_audio_status_target("));
        }

        #[test]
        fn pairing_discovery_prefers_windows_ble_association_endpoint() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_pairing_candidates_from_unpaired_selector")
                .expect("pairing discovery helper should exist");
            let end = source[start..]
                .find("fn listener_pairing_candidates_from_windows_ble_aep")
                .map(|offset| start + offset)
                .expect("AEP pairing helper should exist");
            let body = &source[start..end];
            let aep_index = body
                .find("listener_pairing_candidates_from_windows_ble_aep")
                .expect("pairing discovery must query BLE AEP first");
            let legacy_index = body
                .find("GetDeviceSelectorFromPairingState(false)")
                .expect("legacy BluetoothLEDevice selector should remain as fallback");

            assert!(
                aep_index < legacy_index,
                "Windows BLE pairing discovery must prefer AssociationEndpoint before legacy BLE device interface selector"
            );
            assert!(source.contains("DeviceInformationKind::AssociationEndpoint"));
            assert!(source.contains("System.Devices.Aep.ProtocolId"));
        }

        #[test]
        fn pairing_discovery_requires_connectable_aep_before_full_cache() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_pairing_candidates_from_windows_ble_aep")
                .expect("AEP pairing helper should exist");
            let end = source[start..]
                .find("fn listener_pairing_candidates_from_windows_ble_aep_selector")
                .map(|offset| start + offset)
                .expect("AEP selector helper boundary should exist");
            let body = &source[start..end];
            let connectable_index = body
                .find("WINDOWS_BLE_AEP_CONNECTABLE_SELECTOR")
                .expect("AEP discovery must query connectable devices first");
            let full_cache_index = body
                .find("WINDOWS_BLE_AEP_SELECTOR")
                .expect("AEP discovery should keep full-cache fallback for diagnostics");

            assert!(
                connectable_index < full_cache_index,
                "Windows BLE pairing must prefer connectable AEPs before stale full-cache AEPs"
            );
            assert!(source.contains("System.Devices.Aep.Bluetooth.Le.IsConnectable"));
            assert!(source.contains("connectable_selector={connectable_selector}"));
        }

        #[test]
        fn usb_serial_device_settings_drain_waits_for_quiet_window() {
            let source = include_str!("embedded_ble.rs");
            let exchange_start = source
                .find("fn exchange_device_settings_via_serial_port")
                .expect("device settings serial exchange helper should exist");
            let control_start = source[exchange_start..]
                .find("fn send_control_command_via_serial_port")
                .map(|offset| exchange_start + offset)
                .expect("control serial helper should exist");
            let drain_start = source[control_start..]
                .find("fn drain_serial_input_until_quiet")
                .map(|offset| control_start + offset)
                .expect("quiet drain helper should exist");
            let response_tail_start = source[drain_start..]
                .find("fn response_tail")
                .map(|offset| drain_start + offset)
                .expect("response tail helper should follow quiet drain");
            let production_body = &source[exchange_start..response_tail_start];

            assert!(source.contains("DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION"));
            assert!(source.contains("DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION"));
            assert!(production_body.contains("fn drain_serial_input_until_quiet"));
            assert!(
                production_body.contains("quiet_since.elapsed() >= quiet_duration"),
                "USB serial fallback must drain stale diagnostic output until a quiet window before sending commands"
            );
            assert!(
                !production_body
                    .contains("drain_serial_input(&mut *port, Duration::from_millis(180))"),
                "USB serial fallback must not use the old fixed 180 ms drain"
            );
        }

        #[test]
        fn pairing_discovery_rejects_stale_same_name_aep_when_target_address_known() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn push_listener_pairing_candidate_if_matching")
                .expect("pairing candidate filter should exist");
            let end = source[start..]
                .find("fn device_information_display_name")
                .map(|offset| start + offset)
                .expect("pairing candidate filter boundary should exist");
            let body = &source[start..end];
            let stale_guard_index = body
                .find("!target_addresses.is_empty() && address.is_some() && !address_matches")
                .expect("known-address pairing must reject stale same-name Windows cache entries");
            let name_match_index = body
                .find("listener_pairing_name_matches")
                .expect("pairing candidate filter should still support name matching fallback");

            assert!(
                stale_guard_index < name_match_index,
                "known target address must reject stale Windows cache entries before name-only fallback"
            );
            assert!(body.contains("skipping stale Windows pairing candidate"));
        }

        #[test]
        fn already_paired_requires_trusted_address_not_same_name_cache_only() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn pair_listener_candidate(\n")
                .expect("pairing helper should exist");
            let end = source[start..]
                .find("fn pair_unpaired_listener_candidate")
                .map(|offset| start + offset)
                .expect("pairing helper boundary should exist");
            let body = &source[start..end];
            let guard_index = body
                .find("listener_pairing_candidate_has_trusted_address")
                .expect("cached AlreadyPaired path must require trusted address evidence");
            let already_index = body
                .find("DevicePairingOutcome::AlreadyPaired")
                .expect("cached AlreadyPaired branch should still exist");

            assert!(
                guard_index < already_index,
                "same-name Windows cache entries must not be reported as already paired unless their address is trusted"
            );
            assert!(body.contains(
                "same-name cached pairing without matching Listener address/service proof"
            ));
        }

        #[test]
        fn type_recovery_promotes_only_trusted_cached_addresses() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("if type_recovery_command_confirmed")
                .expect("Type recovery promotion block should exist");
            let end = source[start..]
                .find("let mut result = crate::embedded_ble::BleDevicePairingPromptResult")
                .map(|offset| start + offset)
                .expect("Type recovery promotion block boundary should exist");
            let body = &source[start..end];

            let pre_cleanup_index = body
                .find("unpair_listener_devices_inner(std::slice::from_ref(&target_name))")
                .expect(
                    "confirmed Type recovery must clean stale Windows pairing before PairAsync",
                );
            let candidate_scan_index = body
                .find("listener_recovery_pairing_candidates_for_addresses")
                .expect(
                    "confirmed Type recovery must use recovery-address-aware candidate discovery",
                );
            assert!(
                pre_cleanup_index < candidate_scan_index,
                "confirmed Type recovery must remove stale Windows PnP/bond cache before pairing the Type-observed or freshly scanned recovery address"
            );
            assert!(
                body.contains("get_or_insert_with(listener_recovery_target_addresses)")
                    && body.contains("listener_pairing_candidate_has_trusted_address"),
                "confirmed Type recovery may lazily resolve trusted addresses, but every stale cached candidate must still be checked against that trusted set"
            );
            assert!(
                body.contains("same-name cached candidate without trusted address proof"),
                "confirmed Type recovery must not promote arbitrary same-name stale AEP cache entries"
            );
        }

        #[test]
        fn recovery_target_addresses_include_windows_pnp_service_signature() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn listener_recovery_target_addresses")
                .expect("recovery target address helper should exist");
            let end = source[start..]
                .find("fn push_listener_recovery_target_addresses_from_candidates")
                .map(|offset| start + offset)
                .expect("recovery target address helper boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("configured_bluetooth_address"),
                "recovery address discovery must keep the fast in-process configured address"
            );
            assert!(
                body.contains("listener_pnp_service_signature_addresses"),
                "headless/new Type processes must recover the current Listener address from Windows PnP service UUIDs instead of accepting same-name stale AEP cache entries"
            );
            assert!(source.contains("Listener PnP service-signature addresses"));
        }

        #[test]
        fn windows_pairing_uses_standard_pairasync_by_default() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn run_default_pairing_once")
                .expect("default pairing helper should exist");
            let end = source[start..]
                .find("fn pairing_status_should_retry_after_settle")
                .map(|offset| start + offset)
                .expect("default pairing helper boundary should exist");
            let body = &source[start..end];

            assert!(body.contains(".PairAsync()"));
            assert!(
                !body.contains("PairWithProtectionLevelAsync(DevicePairingProtectionLevel::None)"),
                "Windows BLE HID pairing should not force protection level None without hardware evidence"
            );
        }

        #[test]
        fn windows_pairing_attempts_standard_pairasync_before_custom_fallback() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn pair_unpaired_listener_candidate")
                .expect("pairing helper should exist");
            let end = source[start..]
                .find("fn run_default_pairing_once")
                .map(|offset| start + offset)
                .expect("pairing helper boundary should exist");
            let body = &source[start..end];
            let standard_index = body
                .find("run_default_pairing_once")
                .expect("pairing helper must attempt standard PairAsync");
            let custom_index = body
                .find("custom_pair_listener_candidate")
                .expect("pairing helper should retain custom fallback");

            assert!(
                standard_index < custom_index,
                "Windows BLE HID pairing must keep the hardware-proven v1.0.2 path: standard PairAsync primes Windows, then custom ConfirmOnly recovers Failed(19)"
            );
            assert!(body.contains("pairing_status_should_try_custom_fallback"));
        }

        #[test]
        fn pairing_and_unpair_share_single_maintenance_gate() {
            let source = include_str!("embedded_ble.rs");
            assert!(
                source.contains("LISTENER_PAIRING_MAINTENANCE_TOKEN"),
                "BLE pairing/cache maintenance must have a process-wide owner token"
            );
            assert!(
                source.contains("CreateMutexW")
                    && source.contains("WaitForSingleObject")
                    && source.contains("BLE_PAIRING_MAINTENANCE_MUTEX_NAME"),
                "GUI tray and headless CLI processes must share a Windows named mutex before touching pairing/cache state"
            );
            assert!(
                source.contains("pub fn listener_pairing_maintenance_active"),
                "coordinator must be able to detect an active pairing/cache owner before cleanup"
            );

            let prompt_start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("pairing prompt helper should exist");
            let prompt_end = source[prompt_start..]
                .find("let candidates = if bypass_prompt_suppression")
                .map(|offset| prompt_start + offset)
                .expect("pairing prompt candidate boundary should exist");
            let prompt_setup = &source[prompt_start..prompt_end];
            assert!(
                prompt_setup.contains("try_begin_listener_pairing_maintenance(\"pair\""),
                "PairAsync recovery must acquire the same maintenance gate before touching Windows pairing state"
            );

            let unpair_start = source
                .find("pub fn unpair_listener_devices_for_names")
                .expect("unpair helper should exist");
            let unpair_end = source[unpair_start..]
                .find("fn unpair_listener_devices_inner")
                .map(|offset| unpair_start + offset)
                .expect("unpair helper boundary should exist");
            let unpair_body = &source[unpair_start..unpair_end];
            assert!(
                unpair_body.contains("try_begin_listener_pairing_maintenance(\"unpair\""),
                "Windows cache cleanup must not run while another pairing/cache operation is active"
            );
        }

        #[test]
        fn confirmed_type_recovery_command_allows_direct_stale_cache_cleanup() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("pairing prompt helper should exist");
            let end = source[start..]
                .find("let mut result = crate::embedded_ble::BleDevicePairingPromptResult")
                .map(|offset| start + offset)
                .expect("pairing prompt candidate boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("type_recovery_command_confirmed"));
            assert!(
                body.contains("candidate.fresh_pairing_advertisement = true"),
                "after Type has successfully commanded Listener into recovery pairing, Windows paired cache for the same address must be treated as stale even if advertisement scanning misses the short window"
            );
            assert!(
                source.contains("pub fn prompt_listener_pairing_after_type_recovery"),
                "the confirmed recovery-command path must stay separate from conservative pairing-only scans"
            );
        }

        #[test]
        fn cached_aep_pairing_candidate_never_unpairs_existing_link() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn pair_listener_candidate")
                .expect("pair listener helper should exist");
            let end = source[start..]
                .find("fn pair_unpaired_listener_candidate")
                .map(|offset| start + offset)
                .expect("pair listener helper boundary should exist");
            let body = &source[start..end];
            let cache_guard_index = body
                .find("!candidate.fresh_pairing_advertisement")
                .expect("cached/AEP pairing candidates must be guarded");
            let unpair_index = body
                .find("unpair_device_information_pairing")
                .expect("fresh recovery advertisement path may still clean stale pairing");

            assert!(
                cache_guard_index < unpair_index,
                "Windows cached/AEP candidates must not unpair an existing Listener link; only fresh recovery advertisements can trigger stale-cache cleanup"
            );
            assert!(body.contains("AlreadyPaired"));
            assert!(body.contains("leaving pairing intact"));
        }

        #[test]
        fn recovery_already_paired_cache_requires_fresh_gatt_before_success() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn pair_listener_candidate")
                .expect("pair listener helper should exist");
            let end = source[start..]
                .find("fn pair_unpaired_listener_candidate")
                .map(|offset| start + offset)
                .expect("pair listener helper boundary should exist");
            let body = &source[start..end];
            let verify_index = body
                .find("verify_already_paired_liveness")
                .expect("recovery path must have an already-paired liveness gate");
            let fresh_status_index = body
                .find("listener_trusted_paired_candidate_has_fresh_status")
                .expect("already-paired recovery must prove fresh GATT status before success");
            let stale_cleanup_index = body
                .find("fresh GATT status is not live during recovery; clearing stale host bond before PairAsync")
                .expect("stale host bond cleanup log should name the Windows cache failure mode");
            let force_unpair_index = body
                .find("candidate.fresh_pairing_advertisement = true")
                .expect("failed liveness must force the existing stale cleanup path");
            let unpair_index = body
                .find("unpair_device_information_pairing")
                .expect("recovery stale cache path must still unpair Windows first");

            assert!(verify_index < fresh_status_index);
            assert!(fresh_status_index < stale_cleanup_index);
            assert!(stale_cleanup_index < force_unpair_index);
            assert!(force_unpair_index < unpair_index);

            let helper_start = source
                .find("fn listener_trusted_paired_candidate_has_fresh_status")
                .expect("fresh GATT liveness helper should exist");
            let helper_end = source[helper_start..]
                .find("fn pair_unpaired_listener_candidate")
                .map(|offset| helper_start + offset)
                .expect("fresh GATT liveness helper boundary should exist");
            let helper = &source[helper_start..helper_end];
            assert!(helper.contains("open_embedded_audio_status_target_for_device"));
            assert!(helper.contains("read_embedded_audio_status_from_target_bounded"));
            assert!(helper.contains("status.connected"));
        }

        #[test]
        fn stale_pairing_refresh_uses_known_address_before_selector_query() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn refresh_listener_pairing_candidate")
                .expect("refresh helper should exist");
            let end = source[start..]
                .find("fn unpair_listener_candidate")
                .map(|offset| start + offset)
                .expect("refresh helper boundary should exist");
            let body = &source[start..end];
            let direct_index = body
                .find("pairing_device_information_from_bluetooth_address_handle")
                .expect("refresh should use direct Bluetooth address handle open");
            let selector_index = body
                .find("listener_pairing_candidates")
                .expect("refresh should still keep selector fallback");

            assert!(
                direct_index < selector_index,
                "after stale unpair, a known Listener address should be reopened directly before waiting on the slower Windows AEP selector"
            );
        }

        #[test]
        fn desktop_pairasync_failed_uses_custom_pairing_fallback() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn pairing_status_should_try_custom_fallback")
                .expect("custom fallback status helper should exist");
            let end = source[start..]
                .find("fn custom_pair_listener_candidate")
                .map(|offset| start + offset)
                .expect("custom pairing helper boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("DevicePairingResultStatus::Failed"),
                "desktop Windows PairAsync can return Failed(19) before the system dialog, so Type must try custom ConfirmOnly pairing"
            );
        }

        #[test]
        fn pairasync_failed_restarts_windows_bluetooth_adapter_only_when_allowed() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn pair_unpaired_listener_candidate_with_adapter_recovery")
                .expect("adapter recovery pairing helper should exist");
            let end = source[start..]
                .find("fn run_default_pairing_once")
                .map(|offset| start + offset)
                .expect("adapter recovery pairing helper boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("pairing_status_suggests_adapter_restart(status)"));
            assert!(body.contains("&& allow_adapter_restart"));
            assert!(body.contains("restart_windows_bluetooth_adapter_after_pairing_failure"));
            assert!(
                body.contains("refresh_listener_pairing_candidate(candidate, expected_name)"),
                "after restarting the local adapter, Type must reopen the BLE DeviceInformation before retrying PairAsync"
            );
            assert!(
                body.contains("!allow_adapter_restart")
                    && body.contains("passive in-progress PairAsync settle"),
                "Type automatic recovery should wait briefly for Windows in-progress pairing instead of bouncing the local Bluetooth adapter"
            );
            assert!(
                body.contains("false,"),
                "adapter restart retry must disable another restart to avoid a loop"
            );
            assert!(
                source.contains("fn pairing_status_suggests_adapter_restart")
                    && source.contains("DevicePairingResultStatus::Failed"),
                "only the observed Windows Failed(19) pairing result should trigger adapter restart"
            );
        }

        #[test]
        fn type_recovery_failed_pairasync_retries_once_without_adapter_restart() {
            let source = include_str!("embedded_ble.rs");
            let prompt_start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("prompt helper should exist");
            let prompt_end = source[prompt_start..]
                .find("fn pairing_prompt_suppression_remaining")
                .map(|offset| prompt_start + offset)
                .expect("prompt helper boundary should exist");
            let prompt_body = &source[prompt_start..prompt_end];
            assert!(prompt_body.contains("type_recovery_command_confirmed"));
            assert!(
                prompt_body.contains("retry_failed_pairasync_once")
                    && prompt_body.contains("type_recovery_command_confirmed,"),
                "Type-confirmed recovery should enable the bounded one-shot PairAsync retry"
            );
            assert!(
                prompt_body.contains("allow_adapter_restart,\n                        false,"),
                "slow AEP fallback should not recursively enable the direct PairAsync retry"
            );

            let retry_start = source
                .find("fn pair_unpaired_listener_candidate_with_adapter_recovery")
                .expect("pairing helper should exist");
            let retry_end = source[retry_start..]
                .find("fn run_default_pairing_once")
                .map(|offset| retry_start + offset)
                .expect("pairing helper boundary should exist");
            let retry_body = &source[retry_start..retry_end];
            assert!(
                retry_body.contains("&& retry_failed_pairasync_once")
                    && retry_body.contains("&& !allow_adapter_restart")
                    && retry_body.contains("pairing_status_suggests_adapter_restart(status)"),
                "the retry may only fire for Type recovery Failed(19) when local adapter restart is disabled"
            );
            assert!(
                retry_body.contains("&& !retry_failed_pairasync_once\n            && !allow_adapter_restart"),
                "when Type recovery has an explicit one-shot Failed(19) retry, it must not wait through the passive in-progress settle first"
            );
            let retry_index = retry_body
                .find("Type automatic recovery retrying direct PairAsync once after Windows Failed(19)")
                .expect("Type recovery retry log should exist");
            let adapter_restart_index = retry_body
                .find("restart_windows_bluetooth_adapter_after_pairing_failure")
                .expect("adapter restart branch should still exist");
            assert!(
                retry_index < adapter_restart_index,
                "Type recovery should retry the fresh direct PairAsync path before any adapter-restart branch"
            );
            assert!(
                retry_body.contains("&refreshed_pairing,\n                        expected_name,\n                        false,\n                        false,"),
                "the retry must disable both adapter restart and another retry to avoid a loop"
            );
        }

        #[test]
        fn type_recovery_pairasync_disables_adapter_restart() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("prompt helper should exist");
            let end = source[start..]
                .find("fn pairing_prompt_suppression_remaining")
                .map(|offset| start + offset)
                .expect("prompt helper boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("allow_user_pairing_prompt && !type_recovery_command_confirmed"),
                "Type-owned or user-prompt-suppressed recovery must not restart the whole Windows Bluetooth adapter"
            );
            assert!(
                body.contains("allow_adapter_restart,"),
                "the Type recovery adapter-restart policy must flow into every pairing candidate attempt"
            );
        }

        #[test]
        fn rename_recovery_can_skip_duplicate_pre_pair_cleanup_after_cache_refresh() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt")
                .expect("Type recovery prompt helper should exist");
            let end = source[start..]
                .find("pub fn query_listener_pairing")
                .map(|offset| start + offset)
                .expect("Type recovery prompt helper boundary should exist");
            let helpers = &source[start..end];
            let prompt_start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("prompt helper should exist");
            let prompt_end = source[prompt_start..]
                .find("let mut candidates = if bypass_prompt_suppression")
                .map(|offset| prompt_start + offset)
                .expect("prompt helper cleanup boundary should exist");
            let prompt_setup = &source[prompt_start..prompt_end];

            assert!(helpers.contains(
                "prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup"
            ));
            assert!(helpers.contains(
                "prompt_listener_pairing_inner(expected_name, true, true, false, true, &[])"
            ));
            assert!(helpers.contains(
                "prompt_listener_pairing_inner(\n            expected_name,\n            true,\n            true,\n            false,\n            false,\n            observed_recovery_addresses,"
            ));
            assert!(
                prompt_setup.contains("type_recovery_command_confirmed && pre_pair_stale_cleanup"),
                "double-click/one-click Type recovery must keep pre-pair stale cleanup, while BLE rename may skip it only after its explicit Windows cache cleanup already ran"
            );
        }

        #[test]
        fn windows_pairing_recovery_uses_hidden_pwsh_not_windows_powershell() {
            let source = include_str!("embedded_ble.rs");
            let production = &source[..source
                .find("    mod tests {")
                .expect("Windows BLE test module boundary should exist")];

            assert!(production.contains("fn hidden_pwsh_command() -> Command"));
            assert!(production.contains("hidden_command(\"pwsh\")"));
            assert!(production.contains("run_hidden_pwsh_script"));
            assert!(
                !production.contains("powershell.exe"),
                "workflow/product diagnostics must not spawn Windows PowerShell 5.1 or visible pwsh windows"
            );
        }

        #[test]
        fn custom_pairing_keeps_v1_confirm_ceremonies() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn custom_pair_listener_candidate")
                .expect("custom pairing helper should exist");
            let end = source[start..]
                .find("fn pairing_kind_accepts_without_user")
                .map(|offset| start + offset)
                .expect("custom pairing helper boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("DevicePairingKinds::ConfirmOnly"));
            assert!(
                body.contains("DevicePairingKinds::ConfirmPinMatch"),
                "Windows may request the SC confirm ceremony; keep the accepted v1.0.1 custom pairing kinds"
            );
            assert!(body.contains(".PairAsync(supported_pairing_kinds)"));
            assert!(
                !body.contains("DevicePairingProtectionLevel::None"),
                "do not silently request unprotected HID keyboard pairing"
            );
        }

        #[test]
        fn pairasync_failure_does_not_request_native_windows_pairing_window() {
            let source = include_str!("embedded_ble.rs");
            let production = &source[..source
                .find("    mod tests {")
                .expect("Windows BLE test module boundary should exist")];
            let start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("pairing prompt helper should exist");
            let end = source[start..]
                .find("fn pairing_prompt_suppression_remaining")
                .map(|offset| start + offset)
                .expect("pairing prompt helper boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("result.failed_devices > 0"));
            assert!(
                !body.contains("send_recording_control_native_pairing_recovery"),
                "failed Windows PairAsync during Type recovery must not request an extra Windows native pairing toast"
            );
            assert!(!production.contains("LAST_NATIVE_PAIRING_WINDOW"));
            assert!(!production.contains("BLE_NATIVE_PAIRING_WINDOW_SUPPRESS"));
        }

        #[test]
        fn pairing_prompt_syncs_expected_name_before_control_fallback() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn prompt_listener_pairing_inner")
                .expect("pairing prompt helper should exist");
            let end = source[start..]
                .find("let now = Instant::now();")
                .map(|offset| start + offset)
                .expect("pairing prompt setup boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("let target_name = effective_bluetooth_target_name"));
            assert!(
                body.contains("set_configured_bluetooth_target_name(&target_name);"),
                "expected BLE name must be applied before GATT control fallback scans advertisements"
            );
        }

        #[test]
        fn type_controlled_recovery_uses_explicit_type_commands() {
            let source = include_str!("embedded_ble.rs");
            let production = &source[..source
                .find("    mod tests {")
                .expect("Windows BLE test module boundary should exist")];
            let start = source
                .find("pub fn send_recording_control_recovery")
                .expect("type recovery command should exist");
            let end = source[start..]
                .find("pub fn send_recording_processing_state")
                .map(|offset| start + offset)
                .expect("type recovery boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("\"VREC:RECOVERY:TYPE\""));
            assert!(body.contains("b\"VREC:RECOVERY:TYPE\\n\""));
            assert!(
                !body.contains("b\"VREC:RECOVERY\\n\""),
                "normal Type-controlled recovery should use the explicit Swift-Pair-capable firmware command"
            );
            assert!(source.contains("pub fn send_recording_control_silent_recovery"));
            assert!(source.contains("\"VREC:RECOVERY:TYPE:SILENT\""));
            assert!(source.contains("b\"VREC:RECOVERY:TYPE:SILENT\\n\""));
            assert!(!production.contains("send_recording_control_native_pairing_recovery"));
        }

        #[test]
        fn type_bye_is_a_separate_shutdown_heartbeat_command() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("pub fn send_recording_control_type_bye")
                .expect("Type bye command should exist");
            let end = source[start..]
                .find("pub fn send_recording_processing_state")
                .map(|offset| start + offset)
                .expect("Type bye command boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("b\"TYPE:BYE\\n\""));
            assert!(body.contains("send_audio_control_via_active_capture"));
            assert!(
                !body.contains("send_recording_control_command"),
                "Type shutdown bye must not open a fresh GATT control target; tray quit should not run the long reconnect retry chain"
            );
            assert!(
                !body.contains("VREC:RECOVERY"),
                "Type shutdown must only clear Type-ready heartbeat, not open pairing recovery"
            );
        }

        #[test]
        fn audio_control_advertisement_fallback_requires_windows_pairing() {
            let source = include_str!("embedded_ble.rs");
            let production = &source[..source
                .find("    mod tests {")
                .expect("Windows BLE test module boundary should exist")];
            assert!(!production.contains("TryFreshGattAllowUnpairedAdvertisement"));
            assert!(
                !production.contains("allowing unpaired audio control advertisement GATT fallback")
            );
            assert!(
                production
                    .contains("ensure_paired_listener_for_advertisement_gatt(\"audio control\""),
                "audio control advertisement fallback must require Windows pairing"
            );
        }

        #[test]
        fn advertisement_gatt_pairing_check_accepts_pnp_service_signature_cache_only_after_pairasync(
        ) {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn paired_listener_device_visible_for_addresses")
                .expect("advertisement pairing visibility helper should exist");
            let end = source[start..]
                .find("fn open_notify_target_for_known_addresses")
                .map(|offset| start + offset)
                .expect("advertisement pairing visibility helper boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("allow_pnp_service_signature_cache"),
                "PnP service-signature cache is only pairing evidence after Type has just completed PairAsync"
            );
            assert!(
                body.contains("listener_pnp_service_signature_addresses()"),
                "recent PairAsync notify recovery may accept Windows PnP Listener service nodes while AEP pairing cache lags behind"
            );
            assert!(
                body.contains("addresses.contains(address)"),
                "PnP fallback must still match the advertised Bluetooth address"
            );
            assert!(
                body.contains("allowing advertised GATT fallback while AEP pairing cache refreshes"),
                "runtime logs should distinguish PnP-backed pairing evidence from unpaired advertisement access"
            );

            assert!(source.contains(
                "ensure_paired_listener_for_advertisement_gatt(\"audio notify\", &addresses, false)"
            ));
            assert!(source.contains(
                "ensure_paired_listener_for_advertisement_gatt(\"audio control\", &addresses, false)"
            ));
            assert!(source.contains(
                "ensure_paired_listener_for_advertisement_gatt(\n            \"recent pairing audio notify\",\n            &addresses,\n            true,\n        )"
            ));
        }

        #[test]
        fn persisted_notify_fast_path_blocks_type_ready_during_recovery_swift_pair_window() {
            let source = include_str!("embedded_ble.rs");
            let open_start = source
                .find("fn open_notify_target()")
                .expect("notify target helper should exist");
            let open_end = source[open_start..]
                .find("fn open_notify_target_with_retry")
                .map(|offset| open_start + offset)
                .expect("notify target helper boundary should exist");
            let open_body = &source[open_start..open_end];
            let guard_index = open_body
                .find("recovery_swift_pair_advertisement_visible_for_persisted_address(address)")
                .expect("persisted notify path must probe the recovery Swift Pair window");
            let persisted_index = open_body
                .find("open_notify_target_for_startup_cached_address(address)")
                .expect("persisted startup notify path should still exist");
            assert!(
                guard_index < persisted_index,
                "EC11 Type-controlled recovery must not let the persisted GATT fast path send TYPE:READY before PairAsync recovery"
            );
            assert!(
                open_body.contains("missing pairing must use Type automatic PairAsync recovery"),
                "the guard error must classify as missing pairing so coordinator routes into the existing Type PairAsync recovery path"
            );
            assert!(
                open_body.contains(
                    "recovery_swift_pair_advertisement_visible_for_persisted_address(address)"
                ),
                "the guard must also protect Type cold start after an interrupted EC11 recovery"
            );

            let scan_start = source
                .find("fn listener_swift_pair_advertisement_visible_for_address")
                .expect("Swift Pair guard scan helper should exist");
            let scan_end = source[scan_start..]
                .find("fn scan_ble_advertisements_by_name")
                .map(|offset| scan_start + offset)
                .expect("Swift Pair guard scan helper boundary should exist");
            let scan_body = &source[scan_start..scan_end];
            assert!(scan_body.contains("advertisement_swift_pair_display_name"));
            assert!(
                scan_body.contains("address != target_address"),
                "native/random-address pairing windows must not be mistaken for the persisted Type-owned address"
            );

            let capture_start = source
                .find("fn capture_notification_events_until_cancelled_impl")
                .expect("notify capture helper should exist");
            let capture_end = source[capture_start..]
                .find("fn type_heartbeat_enabled_for_terminal_behavior")
                .map(|offset| capture_start + offset)
                .expect("notify capture helper boundary should exist");
            let capture_body = &source[capture_start..capture_end];
            let reset_index = capture_body
                .find("notify CCCD reset hit recovery pairing window")
                .expect("CCCD reset cancellation should enter Type recovery");
            let enable_index = capture_body
                .find("enabling notify CCCD")
                .expect("notify enable log should remain after reset handling");
            assert!(
                reset_index < enable_index,
                "reset-time recovery-window cancellation must not burn the notify-enable retry ladder before PairAsync"
            );

            let cccd_start = source
                .find("fn write_cccd_notify_with_retry")
                .expect("notify CCCD helper should exist");
            let cccd_end = source[cccd_start..]
                .find("fn write_cccd_indicate_with_retry")
                .map(|offset| cccd_start + offset)
                .expect("notify CCCD helper boundary should exist");
            let cccd_body = &source[cccd_start..cccd_end];
            assert!(cccd_body.contains("recovery_probe_address"));
            assert!(cccd_body.contains("cccd_notify_recovery_pairing_error"));
            assert!(cccd_body.contains("0x800704C7"));
            assert!(
                cccd_body.contains("entering Type PairAsync recovery instead of retrying CCCD"),
                "recovery-window CCCD cancellation must not burn the full CCCD retry ladder before PairAsync"
            );

            assert!(source.contains(
                "write_cccd_notify_with_retry(0, \"diagnostic log\", &data, CCCD_ENABLE_TIMEOUT, None)"
            ));
            assert!(source.contains("cleanup.target.bluetooth_address"));
        }

        #[test]
        fn recent_pairing_notify_recovery_uses_fresh_advertisement_not_stale_configured_address() {
            let source = include_str!("embedded_ble.rs");
            let notify_start = source
                .find("fn open_notify_target()")
                .expect("notify target helper should exist");
            let notify_end = source[notify_start..]
                .find("fn open_notify_target_with_retry")
                .map(|offset| notify_start + offset)
                .expect("notify retry helper boundary should exist");
            let notify_body = &source[notify_start..notify_end];
            let helper_start = source
                .find("fn open_notify_target_from_recent_pairing_advertisement")
                .expect("recent pairing advertisement helper should exist");
            let helper_end = source[helper_start..]
                .find("fn open_audio_control_target_from_advertisement")
                .map(|offset| helper_start + offset)
                .expect("recent pairing advertisement helper boundary should exist");
            let helper_body = &source[helper_start..helper_end];

            assert!(
                notify_body.contains("open_notify_target_from_recent_pairing_advertisement(state)"),
                "after PairAsync succeeds, notify recovery must try the fresh paired advertisement path instead of waiting on Windows service-index cache"
            );
            assert!(
                !notify_body.contains("skipping 12s advertisement fallback"),
                "recent pairing must not block advertisement recovery while Windows refreshes the service table"
            );
            assert!(helper_body.contains("scan_ble_advertisements_by_name"));
            assert!(helper_body.contains("&state.target_name"));
            assert!(helper_body.contains("ensure_paired_listener_for_advertisement_gatt("));
            assert!(helper_body.contains("\"recent pairing audio notify\""));
            assert!(helper_body.contains("&addresses"));
            assert!(helper_body.contains("true"));
            assert!(
                !helper_body.contains("audio_target_advertisement_addresses"),
                "recent pairing fallback must not reuse stale configured Bluetooth addresses"
            );
        }

        #[test]
        fn persisted_startup_notify_address_is_named_bounded_and_after_recent_pairing() {
            let source = include_str!("embedded_ble.rs");
            let notify_start = source
                .find("fn open_notify_target()")
                .expect("notify target helper should exist");
            let notify_end = source[notify_start..]
                .find("fn open_notify_target_with_retry")
                .map(|offset| notify_start + offset)
                .expect("notify retry helper boundary should exist");
            let notify_body = &source[notify_start..notify_end];
            let recent_index = notify_body
                .find("recent_pairing_fast_gatt_active")
                .expect("recent pairing path must remain first");
            let persisted_index = notify_body
                .find("persisted_successful_notify_target_address_for_current")
                .expect("startup persisted address fast path should exist");
            let service_selector_index = notify_body
                .find("GetDeviceSelectorFromUuid(SERVICE_UUID)")
                .expect("service selector fallback should remain");

            assert!(
                recent_index < persisted_index && persisted_index < service_selector_index,
                "persisted startup fast path must not steal recent-pairing recovery and must fall back before service selector"
            );
            assert!(notify_body.contains("if recent_pairing.is_none()"));
            assert!(source.contains(
                "const STARTUP_NOTIFY_FAST_PATH_TIMEOUT: Duration = Duration::from_millis(2500)"
            ));
            assert!(source.contains("open_notify_target_for_startup_cached_address"));
            assert!(source.contains("BluetoothCacheMode::Cached"));
            assert!(source.contains("open_write_characteristic_from_service_with_timeout"));
            assert!(
                source.contains("AUDIO_CONTROL_UUID")
                    && source.contains("persist_successful_notify_target_address(address, \"Type heartbeat ready\")"),
                "startup fast path must keep audio control and persist only after Type heartbeat ready"
            );
        }

        #[test]
        fn persisted_startup_notify_state_ignores_legacy_or_wrong_target_address() {
            let legacy = PersistedBleDeviceState {
                last_successful_address: Some("FB8FBDD8C90F".to_string()),
                target_name: None,
                updated_at: None,
            };
            assert_eq!(
                persisted_ble_device_state_address_for_target(
                    &legacy,
                    DEFAULT_BLUETOOTH_TARGET_NAME
                ),
                None,
                "legacy address-only state must not be trusted after hardware swaps"
            );

            let wrong_target = PersistedBleDeviceState {
                last_successful_address: Some("FB8FBDD8C90F".to_string()),
                target_name: Some("OldType".to_string()),
                updated_at: None,
            };
            assert_eq!(
                persisted_ble_device_state_address_for_target(
                    &wrong_target,
                    DEFAULT_BLUETOOTH_TARGET_NAME
                ),
                None
            );

            let matching = PersistedBleDeviceState {
                last_successful_address: Some("E5:C3:D5:B8:D2:FC".to_string()),
                target_name: Some("listener".to_string()),
                updated_at: None,
            };
            assert_eq!(
                persisted_ble_device_state_address_for_target(
                    &matching,
                    DEFAULT_BLUETOOTH_TARGET_NAME
                ),
                Some(0xE5C3_D5B8_D2FC)
            );
        }

        #[test]
        fn advertisement_address_selection_does_not_short_circuit_on_runtime_cache() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn audio_target_advertisement_addresses")
                .expect("advertisement address helper should exist");
            let end = source[start..]
                .find("pub(super) fn remember_current_bluetooth_target_address_for_name")
                .map(|offset| start + offset)
                .expect("advertisement address helper boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("configured_bluetooth_address_from_env()"),
                "only an explicit env-pinned BLE address may bypass fresh Windows discovery"
            );
            assert!(
                !body.contains("if let Some(address) = configured_bluetooth_address()"),
                "runtime cached BLE addresses must not short-circuit advertisement/PnP discovery"
            );
            assert!(
                body.contains("listener_pnp_service_signature_addresses()"),
                "current Windows PnP/service evidence must be considered before runtime cache"
            );
            assert!(
                body.contains("runtime_bluetooth_target_address()"),
                "runtime cache may remain a late fallback after fresh Windows candidates"
            );
        }

        #[test]
        fn ble_candidate_filter_allows_trusted_address_when_windows_name_cache_lags() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn ble_candidate_allowed")
                .expect("BLE candidate filter should exist");
            let end = source[start..]
                .find("pub(super) fn parse_bluetooth_address_hex")
                .map(|offset| start + offset)
                .expect("BLE candidate filter boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("listener_recovery_target_addresses()"),
                "Windows can expose the old BLE display name after a firmware rename; a trusted Listener address/service signature must still be allowed"
            );
            assert!(body.contains("trusted_addresses.contains(&address)"));
            assert!(body.contains("despite target name"));
        }

        #[test]
        fn ble_candidate_filter_does_not_hard_reject_by_runtime_cache() {
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn ble_candidate_allowed")
                .expect("BLE candidate filter should exist");
            let end = source[start..]
                .find("pub(super) fn parse_bluetooth_address_hex")
                .map(|offset| start + offset)
                .expect("BLE candidate filter boundary should exist");
            let body = &source[start..end];

            assert!(
                body.contains("configured_bluetooth_address_from_env()"),
                "explicit env-pinned addresses may still hard-filter candidates"
            );
            assert!(
                !body.contains("configured_bluetooth_address()"),
                "runtime cached addresses must not hard-filter current Windows service candidates"
            );
            assert!(
                body.contains("listener_recovery_target_addresses()"),
                "Windows PnP/service addresses should remain a positive trust signal"
            );
        }

        #[test]
        fn swift_pair_manufacturer_data_exposes_display_name() {
            assert_eq!(
                swift_pair_display_name_from_manufacturer_entry(
                    0x0006,
                    &[0x03, 0x00, 0x80, b'l', b'i', b's', b't', b'e', b'n', b'e', b'r', b'B']
                ),
                Some("listenerB".to_string())
            );
            assert_eq!(
                swift_pair_display_name_from_manufacturer_entry(
                    0x0006,
                    &[
                        0x06, 0x00, 0x03, 0x00, 0x80, b'l', b'i', b's', b't', b'e', b'n', b'e',
                        b'r', b'B'
                    ]
                ),
                Some("listenerB".to_string())
            );
            assert_eq!(
                swift_pair_display_name_from_manufacturer_entry(
                    0x004C,
                    &[0x03, 0x00, 0x80, b'l', b'i', b's', b't', b'e', b'n', b'e', b'r', b'B']
                ),
                None
            );
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
        fn recovery_advertisement_scan_prefers_configured_current_name() {
            let _guard = bluetooth_target_name_test_lock().lock().unwrap();
            let previous = configured_bluetooth_target_name();
            set_configured_bluetooth_target_name("listenerC");

            let scan_names = listener_recovery_advertisement_scan_names(&[
                "listenerB".to_string(),
                "listenerC".to_string(),
            ]);

            match previous {
                Some(name) => set_configured_bluetooth_target_name(&name),
                None => set_configured_bluetooth_target_name(""),
            }

            assert_eq!(
                scan_names,
                vec!["listenerC".to_string(), "listenerB".to_string()]
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
                let request = rx
                    .recv_timeout(Duration::from_secs(1))
                    .expect("active control request");
                assert_eq!(request.bytes, b"DEVICE:SET knob_rotation=system_volume\n");
                assert_eq!(request.label, "device settings");
                request.result_tx.send(Ok(())).expect("send result");
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
            assert_eq!(
                device_settings_command_ble_name_target("DEVICE:SET ble_name=Blistener"),
                Some("Blistener".to_string())
            );
            assert_eq!(
                device_settings_command_ble_name_target("DEVICE:SET name=OfficeType01"),
                Some("OfficeType01".to_string())
            );
            assert_eq!(
                device_settings_command_ble_name_target("DEVICE:SET knob_rotation=system_volume"),
                None
            );
        }

        #[test]
        fn runtime_target_address_allows_windows_name_cache_mismatch() {
            let _guard = bluetooth_target_name_test_lock().lock().unwrap();
            if configured_bluetooth_address_from_env().is_some()
                || std::env::var("LISTENER_TYPE_BLE_TARGET_NAME").is_ok()
                || std::env::var("LISTENER_TYPE_BLUETOOTH_TARGET_NAME").is_ok()
            {
                return;
            }

            let previous_name = configured_bluetooth_target_name();
            let previous_address = RUNTIME_BLUETOOTH_TARGET_ADDRESS
                .get_or_init(|| Mutex::new(None))
                .lock()
                .ok()
                .and_then(|slot| slot.clone());

            set_configured_bluetooth_target_name("OfficeType01");
            remember_runtime_bluetooth_target_address_for_name(
                0xA4CB8FF2B512,
                "OfficeType01",
                BLE_RENAME_ADDRESS_GRACE_WINDOW,
                "unit test",
            );

            assert!(ble_candidate_allowed(
                "test",
                0,
                "Blistener",
                Some(0xA4CB8FF2B512)
            ));
            assert!(!ble_candidate_allowed(
                "test",
                1,
                "Blistener",
                Some(0xA4CB8FF2B513)
            ));

            match previous_name {
                Some(name) => set_configured_bluetooth_target_name(&name),
                None => set_configured_bluetooth_target_name(""),
            }
            if let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_ADDRESS
                .get_or_init(|| Mutex::new(None))
                .lock()
            {
                *slot = previous_address;
            }
        }

        #[test]
        fn verified_rename_handoff_address_requires_matching_name_and_rename_grace() {
            let _guard = bluetooth_target_name_test_lock().lock().unwrap();
            if configured_bluetooth_address_from_env().is_some()
                || std::env::var("LISTENER_TYPE_BLE_TARGET_NAME").is_ok()
                || std::env::var("LISTENER_TYPE_BLUETOOTH_TARGET_NAME").is_ok()
            {
                return;
            }

            let previous_name = configured_bluetooth_target_name();
            let previous_address = RUNTIME_BLUETOOTH_TARGET_ADDRESS
                .get_or_init(|| Mutex::new(None))
                .lock()
                .ok()
                .and_then(|slot| slot.clone());

            set_configured_bluetooth_target_name("RenameTarget");
            remember_runtime_bluetooth_target_address_for_name(
                0xA4CB8FF2B512,
                "RenameTarget",
                BLE_RENAME_ADDRESS_GRACE_WINDOW,
                "unit test rename handoff",
            );
            assert_eq!(
                verified_bluetooth_target_rename_handoff_address(),
                Some(0xA4CB8FF2B512),
                "a current target address learned for the rename grace window is safe only after firmware confirms the name transition"
            );

            remember_runtime_bluetooth_target_address_for_name(
                0xA4CB8FF2B512,
                "RenameTarget",
                BLE_TARGET_ADDRESS_CACHE_WINDOW,
                "unit test ordinary cache",
            );
            assert_eq!(
                verified_bluetooth_target_rename_handoff_address(),
                None,
                "an ordinary runtime cache address must not become a rename recovery shortcut"
            );

            match previous_name {
                Some(name) => set_configured_bluetooth_target_name(&name),
                None => set_configured_bluetooth_target_name(""),
            }
            if let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_ADDRESS
                .get_or_init(|| Mutex::new(None))
                .lock()
            {
                *slot = previous_address;
            }
        }

        #[test]
        fn default_target_name_accepts_discovered_custom_listener_service_name() {
            let _guard = bluetooth_target_name_test_lock().lock().unwrap();
            if configured_bluetooth_address_from_env().is_some()
                || std::env::var("LISTENER_TYPE_BLE_TARGET_NAME").is_ok()
                || std::env::var("LISTENER_TYPE_BLUETOOTH_TARGET_NAME").is_ok()
            {
                return;
            }

            let previous_name = configured_bluetooth_target_name();
            set_configured_bluetooth_target_name(DEFAULT_BLUETOOTH_TARGET_NAME);

            assert!(ble_candidate_allowed(
                "test",
                0,
                "Blistener",
                Some(0xA4CB8FF2B512)
            ));
            assert!(!ble_candidate_allowed("test", 1, "", Some(0xA4CB8FF2B512)));

            match previous_name {
                Some(name) => set_configured_bluetooth_target_name(&name),
                None => set_configured_bluetooth_target_name(""),
            }
        }

        #[test]
        fn candidate_address_learning_promotes_default_target_name_to_discovered_name() {
            let _guard = bluetooth_target_name_test_lock().lock().unwrap();
            if configured_bluetooth_address_from_env().is_some()
                || std::env::var("LISTENER_TYPE_BLE_TARGET_NAME").is_ok()
                || std::env::var("LISTENER_TYPE_BLUETOOTH_TARGET_NAME").is_ok()
            {
                return;
            }

            let previous_name = configured_bluetooth_target_name();
            let previous_address = RUNTIME_BLUETOOTH_TARGET_ADDRESS
                .get_or_init(|| Mutex::new(None))
                .lock()
                .ok()
                .and_then(|slot| slot.clone());

            set_configured_bluetooth_target_name(DEFAULT_BLUETOOTH_TARGET_NAME);
            remember_runtime_bluetooth_target_address_for_candidate(
                0xA4CB8FF2B512,
                "Blistener",
                "unit test",
            );

            assert_eq!(
                configured_bluetooth_target_name().as_deref(),
                Some("Blistener")
            );
            assert_eq!(runtime_bluetooth_target_address(), Some(0xA4CB8FF2B512));

            match previous_name {
                Some(name) => set_configured_bluetooth_target_name(&name),
                None => set_configured_bluetooth_target_name(""),
            }
            if let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_ADDRESS
                .get_or_init(|| Mutex::new(None))
                .lock()
            {
                *slot = previous_address;
            }
        }

        #[test]
        fn recording_stop_uses_bounded_active_then_usb_serial_without_fresh_gatt() {
            assert_eq!(
                bounded_recording_stop_active_timeout(Duration::from_secs(5)),
                RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT
            );
            let source = include_str!("embedded_ble.rs");
            let start = source
                .find("fn send_recording_stop_control_command")
                .expect("recording stop helper should exist");
            let end = source[start..]
                .find("pub fn send_recording_control_toggle")
                .map(|offset| start + offset)
                .expect("recording stop helper boundary should exist");
            let body = &source[start..end];

            assert!(body.contains("send_audio_control_via_active_capture"));
            assert!(body.contains("send_control_command_via_usb_serial(\"VREC:STOP\""));
            assert!(
                !body.contains("BleFreshGattGuard::enter"),
                "recording stop must not open a late fresh GATT path while active audio is streaming"
            );
            assert!(
                !body.contains("open_audio_control_target_with_retry"),
                "recording stop must not run the long audio-control rediscovery path"
            );
        }

        #[test]
        fn processing_hints_do_not_retry_with_late_fresh_gatt() {
            assert_eq!(
                processing_state_active_transient_fallback(true),
                ActiveControlTransientFallback::ReturnError
            );
            assert_eq!(
                processing_state_active_transient_fallback(false),
                ActiveControlTransientFallback::ReturnError
            );
        }

        #[test]
        fn foreground_probe_success_disables_notify_cccd() {
            assert_eq!(
                NotifyCccdTeardown::for_probe_success(),
                NotifyCccdTeardown::Disable
            );
        }

        #[test]
        fn ota_background_capture_cancel_leaves_old_cccd_for_connection_handoff() {
            assert_eq!(
                NotifyCccdTeardown::for_capture_cancel(
                    CaptureTerminalBehavior::ContinueListening,
                    true,
                    false,
                ),
                NotifyCccdTeardown::LeaveEnabled
            );
            assert_eq!(
                NotifyCccdTeardown::for_capture_cancel(
                    CaptureTerminalBehavior::ContinueListening,
                    false,
                    false,
                ),
                NotifyCccdTeardown::Disable
            );
            assert_eq!(
                NotifyCccdTeardown::for_capture_cancel(
                    CaptureTerminalBehavior::StopCapture,
                    true,
                    false,
                ),
                NotifyCccdTeardown::Disable
            );
        }

        #[test]
        fn confirmed_name_change_handoff_skips_old_cccd_only_for_continuous_listener() {
            assert_eq!(
                NotifyCccdTeardown::for_capture_cancel(
                    CaptureTerminalBehavior::ContinueListening,
                    false,
                    true,
                ),
                NotifyCccdTeardown::LeaveEnabled,
                "a firmware-confirmed BLE rename terminates the old connection, so its background listener must not wait on an obsolete CCCD disable"
            );
            assert_eq!(
                NotifyCccdTeardown::for_capture_cancel(
                    CaptureTerminalBehavior::StopCapture,
                    false,
                    true,
                ),
                NotifyCccdTeardown::Disable,
                "foreground capture cancellation still tears down its CCCD normally"
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
pub fn request_listener_ota_v1_active_link() -> Result<(), String> {
    windows_ble::request_listener_ota_v1_active_link()
}

#[cfg(not(target_os = "windows"))]
pub fn request_listener_ota_v1_active_link() -> Result<(), String> {
    Ok(())
}

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
pub fn send_recording_control_stop(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recording stop is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn send_recording_control_recovery(_timeout: Duration) -> Result<(), String> {
    Err("Embedded BLE recovery is only supported on Windows".to_string())
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
    #[test]
    fn pairing_prompt_throttle_is_target_scoped_and_expires() {
        let same_name_remaining = windows_ble::pairing_prompt_suppression_remaining_for_test(
            "listenerB",
            "listenerB",
            Duration::from_secs(30),
        )
        .expect("same target should be throttled inside the prompt window");
        assert!(same_name_remaining > Duration::ZERO);

        assert!(
            windows_ble::pairing_prompt_suppression_remaining_for_test(
                "listenerB",
                "Blistener",
                Duration::from_secs(30),
            )
            .is_none(),
            "a different configured BLE name must not be suppressed by an older target"
        );
        assert!(
            windows_ble::pairing_prompt_suppression_remaining_for_test(
                "listenerB",
                "listenerB",
                Duration::from_secs(60 * 60),
            )
            .is_none(),
            "the prompt throttle must expire"
        );
    }

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
    fn active_capture_recovery_advertisement_bypasses_link_timeout() {
        let source = include_str!("embedded_ble.rs");
        let helper_start = source
            .find("fn active_capture_disconnect_recovery_pairing_error")
            .expect("active capture recovery helper should exist");
        let helper_end = source[helper_start..]
            .find("fn open_notify_target_from_advertisement")
            .map(|offset| helper_start + offset)
            .expect("active capture recovery helper boundary should exist");
        let helper = &source[helper_start..helper_end];
        assert!(
            helper.contains("recovery_swift_pair_advertisement_visible_for_persisted_address")
                && helper.contains("missing pairing must use Type automatic PairAsync recovery"),
            "only a matching recovery advertisement may turn an active capture disconnect into Type PairAsync recovery"
        );

        let disconnect_start = source
            .find("BleCaptureSignal::Disconnected(reason) =>")
            .expect("capture disconnect branch should exist");
        let disconnect_end = source[disconnect_start..]
            .find("let terminal = super::is_terminal_notification")
            .map(|offset| disconnect_start + offset)
            .expect("capture disconnect branch boundary should exist");
        let disconnect = &source[disconnect_start..disconnect_end];
        let recovery_index = disconnect
            .find("active_capture_disconnect_recovery_pairing_error")
            .expect("active capture must inspect recovery advertising before waiting");
        let timeout_index = disconnect
            .find("ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT")
            .expect("ordinary active-session recovery timeout should remain available");
        assert!(
            recovery_index < timeout_index,
            "a confirmed EC11 recovery advertisement must bypass the active-session timeout, while ordinary link loss keeps the bounded wait"
        );
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
    fn listener_ota_v1_blocks_cross_process_background_heartbeat() {
        let source = include_str!("embedded_ble.rs");
        assert!(
            source.contains("BLE_OTA_OPERATION_MUTEX_NAME")
                && source.contains("Local\\\\Denzic.Listener.Type.BleOtaOperation")
                && source.contains("acquire_ble_ota_process_mutex(\"listener_ota_v1\")"),
            "Listener OTA v1 must hold a Windows named mutex shared by headless CLI and the installed tray process"
        );
        assert!(
            source.contains("BLE OTA operation active in another process; deferring background listener before notify open")
                && source.contains("BLE OTA operation active in another process; closing idle background listener")
                && source.contains("BACKGROUND_LISTENER_DEFERRED_FOR_OTA"),
            "background capture must not keep sending Type heartbeat or report recovery failures while another process owns Listener OTA"
        );
        let prepare_start = source
            .find("pub(super) fn prepare_listener_ota_v1_transfer")
            .expect("Listener OTA v1 prepare helper should exist");
        let prepare_end = source[prepare_start..]
            .find("pub fn transfer_firmware_ota")
            .map(|offset| prepare_start + offset)
            .expect("Listener OTA v1 prepare helper boundary should exist");
        let prepare = &source[prepare_start..prepare_end];
        assert!(
            prepare.contains("_ota_process_guard: ota_process_guard")
                && source.contains("b\"TYPE:OTA\\n\"")
                && source.contains("Listener OTA v1 reconnect handoff")
                && prepare.contains("request_listener_ota_v1_active_link()")
                && source.contains("LISTENER_OTA_V1_RECONNECT_SETTLE")
                && source.contains("reconnect handoff accepted")
                && prepare.contains("waiting {} ms for reconnect handoff")
                && prepare.find("acquire_ble_ota_process_mutex").unwrap()
                    < prepare
                        .find("request_listener_ota_v1_active_link")
                        .unwrap()
                && prepare
                    .find("request_listener_ota_v1_active_link")
                    .unwrap()
                    < prepare.find("BleCaptureGuard::enter").unwrap(),
            "Listener OTA v1 must acquire the cross-process OTA lock, send TYPE:OTA, then open BLE GATT"
        );

        let handoff_start = source
            .find("pub(super) fn request_listener_ota_v1_active_link")
            .expect("Listener OTA v1 reconnect handoff helper should exist");
        let handoff_end = source[handoff_start..]
            .find("pub(super) fn prepare_listener_ota_v1_transfer")
            .map(|offset| handoff_start + offset)
            .expect("Listener OTA v1 reconnect handoff helper boundary should exist");
        let handoff = &source[handoff_start..handoff_end];
        assert!(
            handoff.contains("send_recording_control_command(")
                && handoff.contains("ActiveControlTransientFallback::ReturnError"),
            "headless OTA must use fresh GATT only when no active capture exists and must never race a failing active capture"
        );
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
            "Linda: BLE device path A4CB8FF2B512 failed: BluetoothCacheMode(0): BLE GATT session did not become active after 8000 ms; advertisement fallback failed: No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es) D4E8768AB2EE; skipping audio notify advertisement GATT fallback until Windows pairing completes"
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
                "No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es) A4CB8FF2B512; skipping audio notify advertisement GATT fallback until Windows pairing completes",
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
    #[ignore = "renames the paired Listener hardware without clearing Windows Bluetooth cache"]
    fn ble_name_apply_hardware_roundtrip() {
        let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
            .lock()
            .expect("device settings hardware test mutex poisoned");
        let old_name = std::env::var("LISTENER_BLE_NAME_RENAME_OLD")
            .expect("set LISTENER_BLE_NAME_RENAME_OLD to the currently advertised BLE name");
        let target_name = std::env::var("LISTENER_BLE_NAME_RENAME_TARGET")
            .expect("set LISTENER_BLE_NAME_RENAME_TARGET to the desired BLE name");

        super::windows_ble::set_configured_bluetooth_target_name(&old_name);
        let learned_address =
            super::windows_ble::remember_current_bluetooth_target_address_for_name(
                &target_name,
                Duration::from_secs(4),
                "BLE name hardware test",
            );
        assert!(
            learned_address.is_some(),
            "hardware test should learn current Listener address before renaming old={old_name} target={target_name}"
        );
        super::windows_ble::send_device_settings_command(
            &format!("DEVICE:SET ble_name={target_name}"),
            Duration::from_secs(4),
        )
        .expect("BLE name write should be acknowledged by firmware");
        super::windows_ble::apply_pending_ble_name(Duration::from_secs(4))
            .expect("BLE name apply should refresh advertising without opening pairing recovery");
        std::thread::sleep(Duration::from_secs(2));

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
        assert!(
            !settings.ble_name_pending_restart,
            "firmware should report the refreshed BLE name as applied"
        );
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
        for minutes in [0, 1, 2, 3, 5, 12, 47, 1439, 1440] {
            let extra_ms_values: &[u32] = if minutes == 0 {
                &[0]
            } else {
                &[0, 1, 17_321, 59_999]
            };
            for extra_ms in extra_ms_values {
                let jittered_ms = minutes * 60_000 + extra_ms;
                let status = super::windows_ble::parse_device_settings_status_line(&format!(
                    "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=battery low_power_idle_ms={jittered_ms} plugged_low_power_idle_ms={jittered_ms} battery_low_power_idle_ms={jittered_ms} plugged_low_power_enabled=1 auto_shutdown_ms={jittered_ms} plugged_auto_shutdown_ms=0 battery_auto_shutdown_ms={jittered_ms} knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=0 external_power_present=0 usb_power_present=0 charging=0 charge_full=0"
                ))
                .expect("parse device settings with millisecond jitter");

                assert_eq!(status.low_power_idle_minutes, minutes);
                assert_eq!(status.plugged_low_power_idle_minutes, minutes);
                assert_eq!(status.battery_low_power_idle_minutes, minutes);
                assert_eq!(status.plugged_auto_shutdown_minutes, 0);
                assert_eq!(status.battery_auto_shutdown_minutes, minutes);
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_device_settings_status_line() {
        let status = super::windows_ble::parse_device_settings_status_line(
            "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK plugged_brightness=80 battery_brightness=50 active_power=external active_brightness=80 led_status=70 led_key=65 led_ec11=60 led_edge=55 compact_set=1 low_power_idle_ms=60000 plugged_low_power_idle_ms=120000 battery_low_power_idle_minutes=3 plugged_low_power_enabled=1 low_power_idle_mode=power_mode auto_shutdown_ms=1800000 plugged_auto_shutdown_ms=0 battery_auto_shutdown_minutes=45 auto_shutdown_mode=power_mode knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=1 ble_name_apply=restart_ble_or_reboot loaded_from_nvs=1 external_power_present=1 usb_power_present=1 charging=0 charge_full=1 valid_ranges=brightness_0_100,led_zone_brightness_0_100,low_power_idle_ms_0_86400000"
        )
        .expect("parse device settings");
        assert_eq!(status.brightness_percent, 80);
        assert_eq!(status.plugged_brightness_percent, 80);
        assert_eq!(status.battery_brightness_percent, 50);
        assert_eq!(status.status_led_brightness_percent, 70);
        assert_eq!(status.key_led_brightness_percent, 65);
        assert_eq!(status.knob_led_brightness_percent, 60);
        assert_eq!(status.edge_led_brightness_percent, 55);
        assert!(status.led_zone_brightness_supported);
        assert!(status.compact_set_supported);
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
        assert_eq!(status.brightness_percent, 100);
        assert_eq!(status.plugged_brightness_percent, 100);
        assert_eq!(status.battery_brightness_percent, 100);
        assert_eq!(status.status_led_brightness_percent, 50);
        assert_eq!(status.key_led_brightness_percent, 80);
        assert_eq!(status.knob_led_brightness_percent, 100);
        assert_eq!(status.edge_led_brightness_percent, 100);
        assert_eq!(status.plugged_low_power_idle_minutes, 1);
        assert_eq!(status.battery_low_power_idle_minutes, 1);
        assert!(!status.plugged_low_power_enabled);
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
        assert!(status.brightness_percent <= 100);
        assert!(status.plugged_brightness_percent <= 100);
        assert!(status.battery_brightness_percent <= 100);
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
