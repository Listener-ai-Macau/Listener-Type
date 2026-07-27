//! Windows-only BLE implementation for Listener embedded audio.
//!
//! Extracted from the former nested `mod windows_ble` in `embedded_ble.rs`
//! for AI maintainability. Parent module owns shared types and public wrappers.

use super::DeviceSettingsCommandTransport;
use denzic_ble_windows::{
    advertisement_manufacturer_data_summary, advertisement_swift_pair_display_name,
    bluetooth_name_matches_any, bluetooth_name_matches_expected, buffer_to_vec,
    configret_detail, device_information_bluetooth_address, device_information_display_name,
    device_information_property_bool, device_information_property_string, hidden_command,
    push_unique_address, run_hidden_pwsh_script, scan_ble_advertisements_by_name,
    wait_gatt_write_result, write_cccd_with_timeout, write_gatt_value_status_with_timeout,
    write_gatt_value_with_timeout, WINDOWS_AEP_BLE_IS_CONNECTABLE_PROPERTY,
    WINDOWS_AEP_DEVICE_ADDRESS_PROPERTY, WINDOWS_AEP_IS_CONNECTED_PROPERTY,
    WINDOWS_AEP_IS_PAIRED_PROPERTY, WINDOWS_AEP_IS_PRESENT_PROPERTY,
    WINDOWS_BLE_AEP_CONNECTABLE_SELECTOR, WINDOWS_BLE_AEP_SELECTOR,
};
pub(super) use denzic_ble_windows::{
    decode_bthport_device_name, normalize_pnp_device_instance_id,
    parse_bluetooth_address_from_device_id, parse_bluetooth_address_hex,
};
use std::cell::RefCell;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
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
    GattValueChangedEventArgs, GattWriteOption,
};
use windows::Devices::Bluetooth::{
    BluetoothAddressType, BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice,
};
use windows::Devices::Enumeration::{
    DeviceAccessStatus, DeviceClass, DeviceInformation, DeviceInformationCustomPairing,
    DeviceInformationKind, DeviceInformationPairing, DevicePairingKinds,
    DevicePairingRequestedEventArgs, DevicePairingResultStatus, DeviceUnpairingResultStatus,
};
use windows::Foundation::{EventRegistrationToken, IAsyncOperation, TypedEventHandler};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Locate_DevNodeW, CM_Query_And_Remove_SubTreeW, CM_LOCATE_DEVNODE_NORMAL,
    CM_LOCATE_DEVNODE_PHANTOM, CM_REMOVE_NO_RESTART, CM_REMOVE_UI_NOT_OK, CR_NO_SUCH_DEVINST,
    CR_NO_SUCH_DEVNODE, CR_SUCCESS, PNP_VETO_TYPE,
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
const DEVICE_SETTINGS_REVISION_UUID: GUID =
    GUID::from_u128(denzic_device_control_v1_core::SETTINGS_REVISION_CHARACTERISTIC_UUID_U128);

const DIAGNOSTIC_SERVICE_UUID: GUID =
    GUID::from_u128(denzic_observability_v1_core::DIAG_LOG_GATT_SERVICE_UUID_U128);
const DIAGNOSTIC_CONTROL_UUID: GUID =
    GUID::from_u128(denzic_observability_v1_core::DIAG_LOG_GATT_CONTROL_UUID_U128);
const DIAGNOSTIC_DATA_UUID: GUID =
    GUID::from_u128(denzic_observability_v1_core::DIAG_LOG_GATT_DATA_UUID_U128);
const DIAGNOSTIC_COUNT_UUID: GUID =
    GUID::from_u128(denzic_observability_v1_core::DIAG_LOG_GATT_COUNT_UUID_U128);
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
const NOTIFY_TARGET_OPEN_OTA_POST_CONFIRM_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(250), Duration::from_millis(500)];
const AUDIO_CONTROL_DISCOVERY_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(150),
    Duration::from_millis(350),
    Duration::from_millis(750),
    Duration::from_millis(1500),
];
const ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const EC11_HARDWARE_RECOVERY_ACK_WRITE_TIMEOUT: Duration = Duration::from_millis(16);
const EC11_HARDWARE_RECOVERY_PREPARE_TIMEOUT: Duration = Duration::from_millis(1500);
const EC11_HARDWARE_RECOVERY_DISCONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
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
const BLE_OTA_PREPARATION_MUTEX_NAME: PCWSTR =
    windows::core::w!("Local\\Denzic.Listener.Type.BleOtaPreparation");
const BACKGROUND_LISTENER_DEFERRED_FOR_OTA: &str =
    "embedded_ble_background_listener_deferred_for_ota";
const BLE_RECENT_PAIRING_FAST_GATT_WINDOW: Duration = Duration::from_secs(45);
const BLE_ADAPTER_RESTART_SETTLE: Duration = Duration::from_millis(2500);
const BLE_PAIRING_IN_PROGRESS_SETTLE: Duration = Duration::from_millis(2200);
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

fn ble_wait_cancel() -> denzic_ble_windows::BleCancel {
    ACTIVE_NOTIFY_CAPTURE_CANCEL.with(|slot| match slot.borrow().as_ref() {
        Some(token) => denzic_ble_windows::BleCancel::new(
            Arc::clone(token),
            "background listener recovery",
        ),
        None => denzic_ble_windows::BleCancel::NONE,
    })
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
const LISTENER_OTA_V1_HANDOFF_DISCOVERY_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
];
const RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT: Duration = Duration::from_millis(700);
const DIS_SERVICE_UUID_TEXT: &str = "0000180a-0000-1000-8000-00805f9b34fb";
const BLE_TARGET_ADDRESS_CACHE_WINDOW: Duration = Duration::from_secs(60 * 60);
const BLE_RENAME_ADDRESS_GRACE_WINDOW: Duration = Duration::from_secs(10 * 60);
const BLE_DEVICE_STATE_FILE: &str = "ble_device_state.json";
const STARTUP_NOTIFY_FAST_PATH_TIMEOUT: Duration = Duration::from_millis(2500);
const STARTUP_NOTIFY_FAST_PATH_OPERATION_TIMEOUT: Duration = Duration::from_millis(1200);
const STARTUP_NOTIFY_FAST_PATH_GATT_TIMEOUT: Duration = Duration::from_millis(1800);
const STARTUP_NATIVE_HID_PERSISTED_GATT_TIMEOUT: Duration = Duration::from_millis(600);
static RUNTIME_BLUETOOTH_TARGET_NAME: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static RUNTIME_BLUETOOTH_TARGET_ADDRESS: OnceLock<
    Mutex<Option<RuntimeBluetoothTargetAddress>>,
> = OnceLock::new();
static NATIVE_WINDOWS_HID_PAIRING_VISIBLE: AtomicBool = AtomicBool::new(false);
static OTA_POST_CONFIRM_NOTIFY_TARGET_ADDRESS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
static NATIVE_WINDOWS_HID_PAIRING_ADDRESSES: OnceLock<Mutex<Vec<u64>>> = OnceLock::new();
static NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RUNNING: AtomicBool = AtomicBool::new(false);
static NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RESULT: OnceLock<
    Mutex<Option<Result<Vec<u64>, String>>>,
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
    match unpair_listener_devices_for_known_addresses_inner(extra_names, addresses, true) {
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

pub fn unpair_listener_pairing_for_known_addresses(
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
        try_begin_listener_pairing_maintenance("unpair-direct", &target_name, Instant::now())
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
                "Listener pairing/cache maintenance is already running for {target_name}; deferring direct-pair unpair."
            )],
        };
    };
    match unpair_listener_devices_for_known_addresses_inner(extra_names, addresses, false) {
        Ok(result) => result,
        Err(err) => {
            log::warn!("[embedded-ble] direct-pair Listener address unpair unavailable: {err}");
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

pub fn clear_listener_bthport_cache_for_known_addresses(
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
        try_begin_listener_pairing_maintenance("unpair-cache", &target_name, Instant::now())
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
                "Listener pairing/cache maintenance is already running for {target_name}; deferring exact cache cleanup."
            )],
        };
    };
    match clear_listener_bthport_cache_for_known_addresses_inner(extra_names, addresses) {
        Ok(result) => result,
        Err(err) => {
            log::warn!(
                "[embedded-ble] exact Listener BTHPORT cache cleanup unavailable: {err}"
            );
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

    match listener_pnp_remove_candidates(&target_addresses, &target_names, false) {
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

    match bthport_listener_cache_candidates(&target_addresses, &target_names, false) {
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
    remove_pnp_and_cache: bool,
) -> Result<crate::embedded_ble::BleDeviceUnpairResult, String> {
    let cleanup_started_at = Instant::now();
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
    let discovery_started_at = Instant::now();
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

    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    log::info!(
        "[embedded-ble] known-address cleanup phase=discovery elapsed_ms={} candidates={} addresses={target_addresses:?}",
        discovery_started_at.elapsed().as_millis(),
        candidates.len(),
    );

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

    if candidates.is_empty() {
        result.details.push(
            "No BLE DeviceInformation pairing entry was found for the known recovery address; checking local Windows device/cache records."
                .to_string(),
        );
    }

    let unpair_started_at = Instant::now();
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
    log::info!(
        "[embedded-ble] known-address cleanup phase=unpair elapsed_ms={} removed={} already_clean={} failed={}",
        unpair_started_at.elapsed().as_millis(),
        result.unpaired_devices,
        result.already_unpaired_devices,
        result.failed_devices,
    );

    if !remove_pnp_and_cache {
        result.details.push(
            "Deferred Windows PnP and BTHPORT stale-node cleanup until direct PairAsync fails."
                .to_string(),
        );
        log::info!(
            "[embedded-ble] known-address cleanup phase=pairing_only total_elapsed_ms={} matched={} removed={} already_clean={} failed={}",
            cleanup_started_at.elapsed().as_millis(),
            result.matched_devices,
            result.unpaired_devices,
            result.already_unpaired_devices,
            result.failed_devices,
        );
        return Ok(finalize_known_address_unpair_result(result));
    }

    let pnp_started_at = Instant::now();
    match listener_pnp_remove_candidates(&target_addresses, &target_names, true) {
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
                            "Removed stale Listener device node for known recovery address: {}",
                            candidate.label
                        ));
                    }
                    Ok(DeviceUnpairOutcome::AlreadyUnpaired) => {
                        result.already_unpaired_devices =
                            result.already_unpaired_devices.saturating_add(1);
                        result.details.push(format!(
                            "Listener device node was already removed for known recovery address: {}",
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
            log::warn!("[embedded-ble] known-address Listener PnP stale-node cleanup unavailable: {err}");
            result.details.push(format!(
                "Known-address Listener PnP stale-node cleanup unavailable: {err}"
            ));
        }
    }
    log::info!(
        "[embedded-ble] known-address cleanup phase=pnp elapsed_ms={} matched={} removed={} already_clean={} failed={}",
        pnp_started_at.elapsed().as_millis(),
        result.matched_devices,
        result.unpaired_devices,
        result.already_unpaired_devices,
        result.failed_devices,
    );

    let cache_started_at = Instant::now();
    match bthport_listener_cache_candidates(&target_addresses, &target_names, true) {
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
                            "Removed stale Windows Bluetooth cache for known recovery address: {}",
                            candidate.label
                        ));
                    }
                    Ok(DeviceUnpairOutcome::AlreadyUnpaired) => {
                        result.already_unpaired_devices =
                            result.already_unpaired_devices.saturating_add(1);
                        result.details.push(format!(
                            "Windows Bluetooth cache was already removed for known recovery address: {}",
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
            log::warn!("[embedded-ble] known-address Listener BTHPORT cache cleanup unavailable: {err}");
            result.details.push(format!(
                "Known-address Listener BTHPORT cache cleanup unavailable: {err}"
            ));
        }
    }
    log::info!(
        "[embedded-ble] known-address cleanup phase=bthport_cache elapsed_ms={} total_elapsed_ms={} matched={} removed={} already_clean={} failed={}",
        cache_started_at.elapsed().as_millis(),
        cleanup_started_at.elapsed().as_millis(),
        result.matched_devices,
        result.unpaired_devices,
        result.already_unpaired_devices,
        result.failed_devices,
    );

    Ok(finalize_known_address_unpair_result(result))
}

fn clear_listener_bthport_cache_for_known_addresses_inner(
    extra_names: &[String],
    addresses: &[u64],
) -> Result<crate::embedded_ble::BleDeviceUnpairResult, String> {
    let cleanup_started_at = Instant::now();
    let target_names = listener_target_names(extra_names);
    let mut target_addresses = Vec::new();
    for address in addresses.iter().copied() {
        push_unique_address(&mut target_addresses, address);
    }
    if target_addresses.is_empty() {
        return Err(
            "exact Listener BTHPORT cleanup requires at least one known BLE address"
                .to_string(),
        );
    }

    let mut result = crate::embedded_ble::BleDeviceUnpairResult {
        status: crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 0,
        unpaired_devices: 0,
        already_unpaired_devices: 0,
        failed_devices: 0,
        needs_user_action: true,
        details: Vec::new(),
    };
    for candidate in bthport_listener_cache_candidates(&target_addresses, &target_names, true)?
    {
        result.matched_devices = result.matched_devices.saturating_add(1);
        match delete_bthport_cache_candidate(&candidate) {
            Ok(DeviceUnpairOutcome::Unpaired) => {
                result.unpaired_devices = result.unpaired_devices.saturating_add(1);
                result.details.push(format!(
                    "Removed exact Listener Windows Bluetooth cache: {}",
                    candidate.label
                ));
            }
            Ok(DeviceUnpairOutcome::AlreadyUnpaired) => {
                result.already_unpaired_devices =
                    result.already_unpaired_devices.saturating_add(1);
            }
            Err(err) => {
                result.failed_devices = result.failed_devices.saturating_add(1);
                result.details.push(format!(
                    "Could not remove exact Windows Bluetooth cache {}: {err}",
                    candidate.label
                ));
            }
        }
    }
    log::info!(
        "[embedded-ble] exact known-address cleanup phase=bthport_cache total_elapsed_ms={} matched={} removed={} already_clean={} failed={}",
        cleanup_started_at.elapsed().as_millis(),
        result.matched_devices,
        result.unpaired_devices,
        result.already_unpaired_devices,
        result.failed_devices,
    );
    Ok(finalize_known_address_unpair_result(result))
}

fn finalize_known_address_unpair_result(
    mut result: crate::embedded_ble::BleDeviceUnpairResult,
) -> crate::embedded_ble::BleDeviceUnpairResult {
    if result.matched_devices == 0 {
        result.status = crate::embedded_ble::BleDeviceUnpairStatus::NotFound;
        result.attempted = false;
        result.needs_user_action = true;
        result.details.push(
            "No local Listener pairing/cache entry remained for the known recovery address."
                .to_string(),
        );
        return result;
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
    result
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

struct BleOtaPreparationGuard {
    handle: HANDLE,
    owner: &'static str,
}

impl Drop for BleOtaPreparationGuard {
    fn drop(&mut self) {
        if let Err(err) = unsafe { ReleaseMutex(self.handle) } {
            log::warn!(
                "[embedded-ble] release BLE OTA preparation mutex failed owner={}: {err}",
                self.owner
            );
        }
        if let Err(err) = unsafe { CloseHandle(self.handle) } {
            log::warn!(
                "[embedded-ble] close BLE OTA preparation mutex failed owner={}: {err}",
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
    pub(super) is_listener_hid_keyboard: bool,
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

fn try_acquire_ble_ota_preparation_mutex(
    owner: &'static str,
) -> Option<BleOtaPreparationGuard> {
    let handle = unsafe { CreateMutexW(None, false, BLE_OTA_PREPARATION_MUTEX_NAME) }
        .map_err(|err| {
            log::warn!(
                "[embedded-ble] create BLE OTA preparation mutex failed owner={owner}: {err}"
            );
            err
        })
        .ok()?;
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        return Some(BleOtaPreparationGuard { handle, owner });
    }
    if wait != WAIT_TIMEOUT {
        log::warn!(
            "[embedded-ble] BLE OTA preparation mutex wait returned {wait:?} owner={owner}"
        );
    }
    if let Err(err) = unsafe { CloseHandle(handle) } {
        log::warn!(
            "[embedded-ble] close deferred BLE OTA preparation mutex failed owner={owner}: {err}"
        );
    }
    None
}

fn acquire_ble_ota_preparation_mutex(
    owner: &'static str,
) -> Result<BleOtaPreparationGuard, String> {
    try_acquire_ble_ota_preparation_mutex(owner).ok_or_else(|| {
        format!(
            "BLE OTA preparation is already active in another Listener Type process ({owner})."
        )
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

include!("pairing.rs");

fn listener_pnp_remove_candidates(
    target_addresses: &[u64],
    target_names: &[String],
    exact_address_only: bool,
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
            push_listener_pnp_entry(
                &mut entries,
                &mut known_addresses,
                target_names,
                entry,
                !exact_address_only,
            );
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
                    !exact_address_only,
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
        let name_matches =
            !exact_address_only && bluetooth_name_matches_any(&entry.name, target_names);
        if !listener_pnp_entry_matches_cleanup(
            &entry,
            address_matches,
            name_matches,
            exact_address_only,
        ) {
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
    allow_name_address_expansion: bool,
) {
    if allow_name_address_expansion
        && (bluetooth_name_matches_any(&entry.name, target_names)
            || entry.has_listener_service_signature)
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
        is_listener_hid_keyboard: pnp_instance_is_listener_hid_keyboard(&instance_id),
        has_listener_service_signature: pnp_instance_has_listener_service_signature(
            &instance_id,
        ),
        instance_id,
        address,
    })
}

fn powershell_listener_pnp_entries() -> Result<Vec<ListenerPnpEntry>, String> {
    let raw_entries = denzic_ble_windows::enumerate_ble_hid_pnp_entries()?;
    Ok(listener_pnp_entries_from_raw(raw_entries))
}

fn powershell_listener_present_pnp_entries() -> Result<Vec<ListenerPnpEntry>, String> {
    let raw_entries = denzic_ble_windows::enumerate_present_ble_hid_pnp_entries()?;
    Ok(listener_pnp_entries_from_raw(raw_entries))
}

fn listener_pnp_entries_from_raw(
    raw_entries: Vec<denzic_ble_windows::PnpDeviceEntry>,
) -> Vec<ListenerPnpEntry> {
    let mut entries = Vec::new();
    for entry in raw_entries {
        let Some(instance_id) = entry.instance_id else {
            continue;
        };
        if let Some(entry) = listener_pnp_entry_from_name_and_id(
            entry.friendly_name.unwrap_or_default(),
            instance_id,
        ) {
            entries.push(entry);
        }
    }
    entries
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
    exact_address_only: bool,
) -> bool {
    address_matches
        || (!exact_address_only && (name_matches || entry.has_listener_service_signature))
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

pub(super) fn pnp_instance_is_listener_hid_keyboard(instance_id: &str) -> bool {
    let upper = instance_id.to_ascii_uppercase();
    upper.starts_with(r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_")
        && upper.contains("&COL01\\")
}

fn bthport_listener_cache_candidates(
    target_addresses: &[u64],
    target_names: &[String],
    exact_address_only: bool,
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
        let address = parse_bluetooth_address_hex(&address_key);
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
        if !address_matches
            && (exact_address_only || !bluetooth_name_matches_any(&name, target_names))
        {
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
    match listener_pnp_remove_candidates(&target_addresses, &target_names, false) {
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
    match bthport_listener_cache_candidates(&target_addresses, &target_names, false) {
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
            if header_offset != offset as u32 {
                return Err(format!(
                    "BLE diagnostic chunk offset mismatch: host={offset} firmware_header={header_offset}"
                ));
            }
            let host_crc = denzic_ota_core::crc32_ieee(payload);
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

pub fn send_recording_control_activate(timeout: Duration) -> Result<(), String> {
    send_recording_control_command(
        b"VREC:ACTIVATE\n",
        timeout,
        "automatic recording activation",
        ActiveControlTransientFallback::ReturnError,
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

pub fn send_recording_control_manual_pairing(timeout: Duration) -> Result<(), String> {
    let serial_result =
        send_recovery_control_command_via_usb_serial("VREC:RECOVERY:TYPE:MANUAL", timeout);
    match &serial_result {
        Ok(()) => {
            log::info!("[embedded-ble] manual pairing recovery sent via USB serial");
            return Ok(());
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] manual pairing recovery USB serial path unavailable; trying BLE control: {err}"
            );
        }
    }
    send_recording_control_command(
        b"VREC:RECOVERY:TYPE:MANUAL\n",
        timeout,
        "manual pairing recovery",
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
    let voice_auto_start_enabled =
        optional_bool_field(&fields, "voice_auto_start").unwrap_or(false);
    let voice_auto_stop_enabled =
        optional_bool_field(&fields, "voice_auto_stop").unwrap_or(false);
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
        voice_auto_start_enabled,
        voice_auto_stop_enabled,
        plugged_auto_shutdown_minutes,
        battery_auto_shutdown_minutes,
        settings_revision: optional_u32_field(&fields, "settings_revision").unwrap_or(0),
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
    crate::startup_evidence::record_startup_stage("notify_cccd_enabled");
    let type_heartbeat_enabled =
        type_heartbeat_enabled_for_terminal_behavior(terminal_behavior);
    let mut next_type_heartbeat = None;
    // Notify subscription alone is not the recovery terminal state. The
    // firmware keeps its pairing/LED recovery window open until it has
    // accepted TYPE:READY (or a later Type heartbeat after a retry).
    let mut type_ready_confirmed = false;
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
            crate::startup_evidence::record_startup_stage("type_ready_written");
            cleanup.mark_type_heartbeat_open();
            if let Some(address) = cleanup.target.bluetooth_address {
                persist_successful_notify_target_address(address, "Type heartbeat ready");
            }
            if let Err(err) = cleanup.write_type_heartbeat(
                b"TYPE:AUDIO:LOSSLESS_RICE:3\n",
                "Type lossless audio capability",
            ) {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: lossless audio capability was not acknowledged; firmware will retain raw PCM: {err}"
                );
            } else {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: lossless audio capability announced"
                );
            }
            on_ready()?;
            type_ready_confirmed = true;
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
    let mut ec11_recovery_prepare_disconnect_deadline: Option<Instant> = None;
    let mut ec11_recovery_disconnect_deadline: Option<Instant> = None;
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
                    if !type_ready_confirmed {
                        on_ready()?;
                        type_ready_confirmed = true;
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: Type ready terminal confirmation recovered through heartbeat"
                        );
                    }
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
            return if crate::embedded_audio::transport_v1::stop_drain_expired_finalizes(
                &collector,
            ) {
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
        if ec11_recovery_prepare_disconnect_deadline
            .is_some_and(|prepare_deadline| now >= prepare_deadline)
        {
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization expired without a firmware disconnect"
            );
            ec11_recovery_prepare_disconnect_deadline = None;
        }
        if ec11_recovery_disconnect_deadline
            .is_some_and(|disconnect_deadline| now >= disconnect_deadline)
        {
            let message = format!(
                "Listener EC11 hardware recovery notice did not receive the expected firmware disconnect within {} ms",
                EC11_HARDWARE_RECOVERY_DISCONNECT_TIMEOUT.as_millis()
            );
            log::warn!("[embedded-ble] capture #{capture_id}: {message}");
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
            return Err(message);
        }
        let receive_timeout = [
            stop_drain_deadline,
            deadline,
            link_recovery_deadline,
            ec11_recovery_prepare_disconnect_deadline,
            ec11_recovery_disconnect_deadline,
        ]
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
                    return if crate::embedded_audio::transport_v1::stop_drain_expired_finalizes(
                        &collector,
                    ) {
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
                if ec11_recovery_disconnect_deadline.take().is_some() {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: firmware disconnect observed after EC11 recovery notice; releasing the retained GATT session for recovery arbitration"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
                    return Err(
                        "Listener EC11 hardware recovery notice received before pairing reset; Type observed the firmware disconnect and must scan the matching recovery advertisement and run automatic PairAsync recovery".to_string(),
                    );
                }
                if ec11_recovery_prepare_disconnect_deadline.take().is_some() {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: firmware disconnect observed during EC11 pre-authorized double-click window; recovery remains gated on matching advertising"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
                    return Err(
                        "Listener EC11 hardware recovery notice received before pairing reset via pre-authorization; Type must scan the matching recovery advertisement before automatic PairAsync recovery".to_string(),
                    );
                }
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
        if super::is_ec11_hardware_recovery_prepare_notice(&notification) {
            log::info!(
                "[embedded-ble] capture #{capture_id}: received EC11 recovery pre-authorization during the double-click window"
            );
            if let Err(err) = cleanup.write_ec11_recovery_prepare_acknowledgement() {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization acknowledgement was not queued: {err}"
                );
                continue;
            }
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization acknowledgement queued via active GATT control"
            );
            ec11_recovery_prepare_disconnect_deadline =
                Some(Instant::now() + EC11_HARDWARE_RECOVERY_PREPARE_TIMEOUT);
            continue;
        }
        if super::is_ec11_hardware_recovery_notice(&notification) {
            log::warn!(
                "[embedded-ble] capture #{capture_id}: received EC11 hardware recovery notice; retaining the GATT session until the firmware disconnect completes"
            );
            if let Err(err) = cleanup.write_ec11_recovery_acknowledgement() {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: EC11 recovery notice acknowledgement was not queued; retaining the existing GATT session without PairAsync authorization: {err}"
                );
                continue;
            }
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery acknowledgement queued via active GATT control"
            );
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            ec11_recovery_disconnect_deadline =
                Some(Instant::now() + EC11_HARDWARE_RECOVERY_DISCONNECT_TIMEOUT);
            continue;
        }
        let notification = crate::audio_transport_codec::normalize_listener_audio_notification(
            &notification,
        )
        .map_err(|err| {
            format!(
                "[embedded-ble] capture #{capture_id}: lossless audio notification rejected: {err}"
            )
        })?;
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
    crate::embedded_audio::transport_v1::has_active_recoverable_session(collector)
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

enum PreparedListenerOtaV1TransferOwnership {
    Exclusive {
        transfer_guard: BleCaptureGuard,
        _ota_process_guard: BleOtaProcessGuard,
    },
    Staged,
}

pub(super) struct PreparedListenerOtaV1Transfer {
    target: OpenListenerOtaV1Target,
    snapshot: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    _preparation_guard: BleOtaPreparationGuard,
    ownership: PreparedListenerOtaV1TransferOwnership,
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
        let Self {
            target,
            snapshot: _,
            _preparation_guard,
            ownership,
        } = self;
        let (transfer_guard, _ota_process_guard) = match ownership {
            PreparedListenerOtaV1TransferOwnership::Exclusive {
                transfer_guard,
                _ota_process_guard,
            } => (transfer_guard, _ota_process_guard),
            PreparedListenerOtaV1TransferOwnership::Staged => {
                let ota_process_guard = acquire_ble_ota_process_mutex("listener_ota_v1")?;
                let transfer_guard = BleCaptureGuard::enter(None)?;
                (transfer_guard, ota_process_guard)
            }
        };
        transfer_denzic_ota_v1_to_target(
            &target,
            transfer_guard.session_id(),
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
        // SYNC has no state transition on Firmware. Its following uncached status
        // read verifies the acknowledged offset, so avoid the slower detailed
        // WinRT write-result path while retaining ATT write-with-response.
        let use_status_write = listener_ota_v1_sync_control_uses_status_write(packet);
        // 设备 OTA control 特征值要求加密连接。BEGIN / SYNC / FINISH 等控制写在 GATT
        // session 还没完成加密/绑定、或长传输后加密丢失时都会 ATT protocol_error=14
        // (Insufficient Authentication)。短暂等待加密恢复后重试，避免第一次点 OTA 失败、
        // 或长传输结束 FINISH 失败（用户实测 FINISH 在 75s 传输后丢加密而失败）。
        const CONTROL_AUTH_RETRIES: usize = 5;
        const CONTROL_AUTH_RETRY_DELAY_MS: u64 = 1000;
        let max_attempts: usize = CONTROL_AUTH_RETRIES + 1;
        for attempt in 0..max_attempts {
            let result = if use_status_write {
                write_gatt_value_status_with_timeout(
                    &self.target.control,
                    packet,
                    GattWriteOption::WriteWithResponse,
                    OTA_WRITE_TIMEOUT,
                    label,
                )
            } else {
                write_gatt_value_with_timeout(
                    &self.target.control,
                    packet,
                    GattWriteOption::WriteWithResponse,
                    OTA_WRITE_TIMEOUT,
                    label,
                )
            };
            match result {
                Ok(_) => return Ok(()),
                Err(err) => {
                    let needs_auth_retry = attempt + 1 < max_attempts
                        && (err.contains("protocol_error") || err.contains("ProtocolError"));
                    if needs_auth_retry {
                        log::warn!(
                            "[embedded-ble] Denzic OTA v1 #{}: {} — GATT session 加密/绑定未就绪，等待 {}ms 后重试 attempt={}",
                            self.transfer_id,
                            label,
                            CONTROL_AUTH_RETRY_DELAY_MS,
                            attempt + 1
                        );
                        std::thread::sleep(std::time::Duration::from_millis(
                            CONTROL_AUTH_RETRY_DELAY_MS,
                        ));
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        unreachable!("write_control retry loop must return inside the loop")
    }

    fn write_data(&mut self, packet: &[u8]) -> Result<(), String> {
        let result = write_listener_ota_v1_value_with_fallback(
            &self.target.data,
            packet,
            self.data_write_option,
            OTA_WRITE_TIMEOUT,
            "Denzic OTA v1 data",
        );
        self.data_write_option = result?;
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
        // FINISH 和 BEGIN 一样要求加密 GATT session。长传输(~75s)后 session 加密可能
        // 丢失，FINISH 会 ATT protocol_error=14 (Insufficient Authentication)。等待加密
        // 恢复后重试，避免长传输结束却提交失败。reboot handoff 错误（设备已重启）不算。
        const FINISH_AUTH_RETRIES: usize = 5;
        const FINISH_AUTH_RETRY_DELAY_MS: u64 = 1000;
        for attempt in 0..(FINISH_AUTH_RETRIES + 1) {
            match write_gatt_value_with_timeout(
                &self.target.control,
                packet,
                GattWriteOption::WriteWithResponse,
                OTA_FINISH_WRITE_TIMEOUT,
                "Denzic OTA v1 finish",
            ) {
                Ok(_) => return Ok(()),
                Err(error) if is_ota_finish_reboot_handoff_error(&error) => {
                    log::info!(
                        "[embedded-ble] Denzic OTA v1 #{}: finish completed through reboot handoff: {}",
                        self.transfer_id,
                        error
                    );
                    return Ok(());
                }
                Err(error) => {
                    let needs_auth_retry = attempt < FINISH_AUTH_RETRIES
                        && (error.contains("protocol_error") || error.contains("ProtocolError"));
                    if needs_auth_retry {
                        log::warn!(
                            "[embedded-ble] Denzic OTA v1 #{}: finish write {} — GATT session 加密/绑定未就绪，等待 {}ms 后重试 attempt={}",
                            self.transfer_id,
                            error,
                            FINISH_AUTH_RETRY_DELAY_MS,
                            attempt + 1
                        );
                        std::thread::sleep(std::time::Duration::from_millis(
                            FINISH_AUTH_RETRY_DELAY_MS,
                        ));
                        continue;
                    }
                    return Err(error);
                }
            }
        }
        unreachable!("write_finish retry loop must return inside the loop")
    }
}

include!("ota_transfer.rs");

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
    gatt_ready_timeout: Duration,
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
        match open_notify_characteristic_from_service_for_startup_fast_path(
            &service,
            deadline,
            gatt_ready_timeout,
        ) {
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

fn open_notify_target_for_current_native_windows_hid_service_endpoint(
    addresses: &[u64],
    timeout: Duration,
) -> Result<OpenNotifyTarget, String> {
    let selector = GattDeviceService::GetDeviceSelectorFromUuid(SERVICE_UUID)
        .map_err(|err| format!("native Windows HID service selector failed: {err}"))?;
    let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
        .map_err(|err| format!("native Windows HID service query failed: {err}"))
        .and_then(|op| wait_async_operation(op, timeout, "native Windows HID service query"))?;
    let count = devices
        .Size()
        .map_err(|err| format!("native Windows HID service collection size failed: {err}"))?;
    let mut last_error = None;

    for index in 0..count {
        let info = devices.GetAt(index).map_err(|err| {
            format!("native Windows HID service entry {index} read failed: {err}")
        })?;
        let id = info.Id().map_err(|err| {
            format!("native Windows HID service entry {index} id read failed: {err}")
        })?;
        let Some(address) = parse_bluetooth_address_from_device_id(&id.to_string_lossy())
        else {
            continue;
        };
        if !addresses.contains(&address) {
            continue;
        }

        match open_notify_target_for_service_with_timeout(&id, timeout).and_then(|target| {
            require_audio_control_for_notify_target(
                target,
                "current native Windows HID service-id endpoint",
            )
        }) {
            Ok(target) => return Ok(target),
            Err(err) => {
                last_error = Some(format!(
                    "native Windows HID service endpoint address={address:012X} failed: {err}"
                ))
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        "no current native Windows HID service endpoint matched the paired identity".to_string()
    }))
}

fn open_notify_target() -> Result<OpenNotifyTarget, String> {
    if notify_capture_cancel_requested() {
        return Err(notify_capture_cancelled_error("notify target open"));
    }
    let recent_pairing = recent_pairing_fast_gatt_active(Instant::now());
    if let Some(state) = recent_pairing.as_ref() {
        match open_notify_target_for_known_addresses_with_cache_modes(
            "recent pairing fast GATT",
            state.address,
            bluetooth_cache_modes_for_policy(
                denzic_ble_pairing::RECENT_PAIRING_NOTIFY_CACHE_POLICY,
            ),
        ) {
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

    let native_windows_hid_addresses =
        if recent_pairing.is_none() && native_windows_hid_pairing_visible_for_startup() {
            native_windows_hid_pairing_addresses_for_startup()
        } else {
            Vec::new()
        };
    let native_windows_hid_pairing = !native_windows_hid_addresses.is_empty();
    if recent_pairing.is_none() {
        let gatt_ready_timeout = if native_windows_hid_pairing {
            STARTUP_NATIVE_HID_PERSISTED_GATT_TIMEOUT
        } else {
            STARTUP_NOTIFY_FAST_PATH_GATT_TIMEOUT
        };
        let mut native_windows_hid_error = None;
        for address in &native_windows_hid_addresses {
            match open_notify_target_for_current_native_windows_hid(*address) {
                Ok(target) => {
                    remember_runtime_bluetooth_target_address_for_current(
                        *address,
                        "native Windows HID startup audio notify",
                    );
                    log::info!(
                        "[embedded-ble] selected native Windows HID startup audio notify address={address:012X}"
                    );
                    crate::startup_evidence::record_startup_stage(
                        "native_hid_notify_target_ready",
                    );
                    crate::startup_evidence::record_startup_path("native_windows_hid");
                    return Ok(target);
                }
                Err(err) => {
                    native_windows_hid_error = Some(err.clone());
                    log::info!(
                        "[embedded-ble] native Windows HID startup audio notify address={address:012X} not ready: {}",
                        err.chars().take(240).collect::<String>()
                    );
                }
            }
        }
        if native_windows_hid_pairing {
            let startup_error = native_windows_hid_error.unwrap_or_else(|| {
                "No current native Windows HID address was available for audio notify recovery"
                    .to_string()
            });
            match native_windows_hid_pairing_addresses() {
                Ok(refreshed_addresses)
                    if native_windows_hid_snapshot_refresh_is_useful(
                        &native_windows_hid_addresses,
                        &refreshed_addresses,
                    ) =>
                {
                    log::info!(
                        "[embedded-ble] native Windows HID startup identity changed after direct GATT miss; retrying refreshed current identities only"
                    );
                    let mut refreshed_error = None;
                    for address in &refreshed_addresses {
                        match open_notify_target_for_current_native_windows_hid(*address) {
                            Ok(target) => {
                                remember_runtime_bluetooth_target_address_for_current(
                                    *address,
                                    "refreshed native Windows HID audio notify",
                                );
                                log::info!(
                                    "[embedded-ble] selected refreshed native Windows HID audio notify address={address:012X}"
                                );
                                crate::startup_evidence::record_startup_stage(
                                    "native_hid_notify_target_ready",
                                );
                                crate::startup_evidence::record_startup_path(
                                    "native_windows_hid",
                                );
                                return Ok(target);
                            }
                            Err(err) => {
                                refreshed_error = Some(err.clone());
                                log::info!(
                                    "[embedded-ble] refreshed native Windows HID audio notify address={address:012X} not ready: {}",
                                    err.chars().take(240).collect::<String>()
                                );
                            }
                        }
                    }
                    return Err(refreshed_error.unwrap_or(startup_error));
                }
                Ok(_) => {}
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] native Windows HID startup identity refresh failed after direct GATT miss: {}",
                        err.chars().take(240).collect::<String>()
                    );
                }
            }
            match open_notify_target_for_current_native_windows_hid_service_endpoint(
                &native_windows_hid_addresses,
                STARTUP_NOTIFY_FAST_PATH_GATT_TIMEOUT,
            ) {
                Ok(target) => {
                    if let Some(address) = target.bluetooth_address {
                        remember_runtime_bluetooth_target_address_for_current(
                            address,
                            "native Windows HID service-id endpoint",
                        );
                    }
                    log::info!(
                        "[embedded-ble] selected current native Windows HID service-id endpoint after direct GATT miss"
                    );
                    crate::startup_evidence::record_startup_stage(
                        "native_hid_notify_target_ready",
                    );
                    crate::startup_evidence::record_startup_path("native_windows_hid");
                    return Ok(target);
                }
                Err(endpoint_error) => {
                    return Err(format!(
                        "{startup_error}; current native Windows HID service-id endpoint failed: {endpoint_error}"
                    ));
                }
            }
        }
        // An Idle wake happens in the same Type process that most recently had
        // a working notify subscription. Prefer that verified in-memory address
        // over an older on-disk address, which may belong to the pre-recovery
        // BLE identity and otherwise burns the entire cached-service timeout.
        if native_windows_hid_addresses.is_empty() {
            let runtime_address = runtime_bluetooth_target_address();
            if let Some(address) = runtime_address {
                match open_notify_target_for_startup_cached_address(address, gatt_ready_timeout)
                {
                    Ok(target) => {
                        remember_runtime_bluetooth_target_address_for_current(
                            address,
                            "runtime startup audio notify",
                        );
                        log::info!(
                            "[embedded-ble] selected runtime startup audio notify address={address:012X}"
                        );
                        crate::startup_evidence::record_startup_path("runtime_cached");
                        return Ok(target);
                    }
                    Err(err) => {
                        log::info!(
                            "[embedded-ble] runtime startup audio notify address={address:012X} not ready: {}",
                            err.chars().take(240).collect::<String>()
                        );
                    }
                }
            }
            if let Some(address) = persisted_successful_notify_target_address_for_current()
                .filter(|address| Some(*address) != runtime_address)
            {
                match open_notify_target_for_startup_cached_address(address, gatt_ready_timeout)
                {
                    Ok(target) => {
                        remember_runtime_bluetooth_target_address_for_current(
                            address,
                            "persisted startup audio notify",
                        );
                        log::info!(
                            "[embedded-ble] selected persisted startup audio notify address={address:012X}"
                        );
                        crate::startup_evidence::record_startup_path("persisted_cached");
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
            if native_windows_hid_pairing
                && !matches!(
                    address,
                    Some(address) if native_windows_hid_addresses.contains(&address)
                )
            {
                log::debug!(
                    "[embedded-ble] skipping stale service-selector address={address:?}; it is outside the current native Windows HID identity set"
                );
                continue;
            }
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
                        if native_windows_hid_pairing {
                            log::info!(
                                "[embedded-ble] selected native Windows HID startup audio notify address={address:012X}"
                            );
                            crate::startup_evidence::record_startup_path("native_windows_hid");
                        } else {
                            log::info!(
                                "[embedded-ble] selected device path index={index} name={name} address={address:012X}"
                            );
                            crate::startup_evidence::record_startup_path("service_selector");
                        }
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
                    crate::startup_evidence::record_startup_path("service_selector");
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
    let ota_post_confirm_address = take_listener_ota_post_confirm_notify_target_address();
    let retry_delays = notify_target_open_retry_delays(ota_post_confirm_address.is_some());
    if let Some(address) = ota_post_confirm_address {
        log::info!(
            "[embedded-ble] capture #{capture_id}: using verified post-confirm OTA notify target address={address:012X}"
        );
    }
    let mut last_error = None;
    for attempt in 1..=retry_delays.len() + 1 {
        if notify_capture_cancel_requested() {
            return Err(notify_capture_cancelled_error("notify target open"));
        }
        let opened = match ota_post_confirm_address {
            Some(address) => open_notify_target_for_post_confirm_native_windows_hid(address),
            None => open_notify_target(),
        };
        match opened {
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
                if attempt > retry_delays.len() || !is_transient_notify_target_open_error(&err)
                {
                    return Err(err);
                }
                let delay = retry_delays[attempt - 1];
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

fn notify_target_open_retry_delays(ota_post_confirm: bool) -> &'static [Duration] {
    if ota_post_confirm {
        &NOTIFY_TARGET_OPEN_OTA_POST_CONFIRM_RETRY_DELAYS
    } else {
        &NOTIFY_TARGET_OPEN_RETRY_DELAYS
    }
}

fn take_listener_ota_post_confirm_notify_target_address() -> Option<u64> {
    OTA_POST_CONFIRM_NOTIFY_TARGET_ADDRESS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()?
        .take()
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

pub fn read_embedded_audio_status_for_device(
    address: u64,
    timeout: Duration,
) -> Result<crate::embedded_ble::EmbeddedAudioBleStatus, String> {
    let _fresh_guard = BleFreshGattGuard::enter("embedded audio status for paired device")?;
    let deadline = Instant::now() + timeout.max(Duration::from_millis(250));
    let target = open_embedded_audio_status_target_for_device(address, deadline)?;
    Ok(read_embedded_audio_status_from_target_bounded(
        &target, deadline,
    ))
}

pub fn read_device_settings_revision(timeout: Duration) -> Result<u32, String> {
    let _fresh_guard = BleFreshGattGuard::enter("device settings revision")?;
    let deadline = Instant::now() + timeout.max(Duration::from_millis(250));
    let target = open_embedded_audio_status_target(timeout)?;
    let value = read_embedded_audio_status_string_once(
        &target.service,
        DEVICE_SETTINGS_REVISION_UUID,
        deadline,
        "device settings revision",
    )
    .ok_or_else(|| "fresh Listener device settings revision read timed out".to_string())?;
    crate::embedded_ble::parse_device_settings_revision_characteristic(&value)
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
    err.contains("GattCommunicationStatus(1)")
        || err.contains("GattCommunicationStatus(3)")
        || err.contains("Unreachable")
        || err.contains("unreachable")
        || err.contains("HRESULT(0x80070016)")
        || err.contains("HRESULT(0x800706BA)")
        || err.contains("BLE characteristic discovery returned status")
        || err.contains("BLE service open wait failed")
        || err.contains("service discovery wait failed")
        || err.contains("GATT session did not become active")
        || err.contains("BLE audio control unavailable while opening notify target")
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

// Maps a platform GATT cache policy (denzic_ble_pairing, protocol section 9)
// onto the WinRT cache-mode attempt sequence. The platform table decides
// which policy each scenario uses; this adapter only translates the modes.
fn bluetooth_cache_modes_for_policy(
    policy: denzic_ble_pairing::GattCachePolicy,
) -> &'static [BluetoothCacheMode] {
    match policy {
        denzic_ble_pairing::GattCachePolicy::CachedOnly => &[BluetoothCacheMode::Cached],
        denzic_ble_pairing::GattCachePolicy::UncachedOnly => &[BluetoothCacheMode::Uncached],
        denzic_ble_pairing::GattCachePolicy::UncachedFirst => {
            &[BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached]
        }
        denzic_ble_pairing::GattCachePolicy::CachedFirst => {
            &[BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached]
        }
    }
}

fn open_notify_target_for_known_addresses(
    context: &str,
    preferred_address: Option<u64>,
) -> Result<OpenNotifyTarget, String> {
    open_notify_target_for_known_addresses_with_cache_modes(
        context,
        preferred_address,
        bluetooth_cache_modes_for_policy(denzic_ble_pairing::KNOWN_ADDRESS_NOTIFY_CACHE_POLICY),
    )
}

fn open_notify_target_for_known_addresses_with_cache_modes(
    context: &str,
    preferred_address: Option<u64>,
    cache_modes: &[BluetoothCacheMode],
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
        match open_notify_target_for_device_with_cache_modes(address, cache_modes) {
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
    denzic_ble_windows::find_bluetooth_target_service_address(
        service_uuid,
        target_name,
        timeout,
        context,
        &ble_wait_cancel(),
    )
}

fn find_paired_bluetooth_target_address(
    target_name: &str,
    timeout: Duration,
    context: &str,
) -> Option<u64> {
    denzic_ble_windows::find_paired_bluetooth_target_address(
        target_name,
        timeout,
        context,
        &ble_wait_cancel(),
    )
}

fn read_optional_string_characteristic(
    device: &BluetoothLEDevice,
    service_uuid: GUID,
    characteristic_uuid: GUID,
) -> Option<String> {
    denzic_ble_windows::read_optional_string_characteristic(
        device,
        service_uuid,
        characteristic_uuid,
        &ble_wait_cancel(),
    )
}

fn read_optional_string_characteristic_from_service_with_timeout(
    service: &GattDeviceService,
    characteristic_uuid: GUID,
    cache_mode: BluetoothCacheMode,
    timeout: Duration,
) -> Option<String> {
    denzic_ble_windows::read_optional_string_characteristic_from_service_with_timeout(
        service,
        characteristic_uuid,
        cache_mode,
        timeout,
        &ble_wait_cancel(),
    )
}

fn read_optional_string_characteristic_from_service(
    service: &GattDeviceService,
    characteristic_uuid: GUID,
    cache_mode: BluetoothCacheMode,
) -> Option<String> {
    denzic_ble_windows::read_optional_string_characteristic_from_service(
        service,
        characteristic_uuid,
        cache_mode,
        &ble_wait_cancel(),
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

fn normalize_listener_ota_hardware_revision(
    model: Option<String>,
    hardware: Option<String>,
) -> Option<String> {
    normalize_optional_hardware_revision(hardware)
        .or_else(|| normalize_optional_hardware_revision(model))
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
    denzic_ble_windows::read_optional_u8_characteristic(
        device,
        service_uuid,
        characteristic_uuid,
        &ble_wait_cancel(),
    )
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
    denzic_ble_windows::read_optional_characteristic_bytes(
        device,
        service_uuid,
        characteristic_uuid,
        &ble_wait_cancel(),
    )
}

fn read_optional_characteristic_from_service_with_timeout(
    service: &GattDeviceService,
    characteristic_uuid: GUID,
    cache_mode: BluetoothCacheMode,
    timeout: Duration,
) -> Option<Vec<u8>> {
    denzic_ble_windows::read_optional_characteristic_from_service_with_timeout(
        service,
        characteristic_uuid,
        cache_mode,
        timeout,
        &ble_wait_cancel(),
    )
}

fn read_optional_characteristic_from_service(
    service: &GattDeviceService,
    characteristic_uuid: GUID,
    cache_mode: BluetoothCacheMode,
) -> Option<Vec<u8>> {
    denzic_ble_windows::read_optional_characteristic_from_service(
        service,
        characteristic_uuid,
        cache_mode,
        &ble_wait_cancel(),
    )
}

fn open_listener_ota_v1_target_after_active_link_handoff(
) -> Result<OpenListenerOtaV1Target, String> {
    let Some(address) = runtime_bluetooth_target_address() else {
        log::info!(
            "[embedded-ble] Listener OTA v1 handoff has no verified runtime address; using normal service discovery"
        );
        return open_listener_ota_v1_target();
    };

    let mut direct_error = None;
    for attempt in 1..=LISTENER_OTA_V1_HANDOFF_DISCOVERY_RETRY_DELAYS.len() + 1 {
        match open_listener_ota_v1_target_for_verified_active_handoff(address) {
            Ok(target) => {
                log::info!(
                    "[embedded-ble] Listener OTA v1 handoff opened verified address {address:012X} on attempt {attempt}"
                );
                return Ok(target);
            }
            Err(err) => {
                let Some(delay) = LISTENER_OTA_V1_HANDOFF_DISCOVERY_RETRY_DELAYS
                    .get(attempt - 1)
                    .copied()
                else {
                    direct_error = Some(err);
                    break;
                };
                if !is_transient_listener_ota_v1_discovery_error(&err) {
                    direct_error = Some(err);
                    break;
                }
                log::info!(
                    "[embedded-ble] Listener OTA v1 handoff address {address:012X} not ready on attempt {attempt}: {err}; retrying in {} ms",
                    delay.as_millis()
                );
                direct_error = Some(err);
                std::thread::sleep(delay);
            }
        }
    }

    let direct_error = direct_error.unwrap_or_else(|| {
        "verified Listener address did not expose a writable OTA v1 service".to_string()
    });
    let native_windows_hid_addresses = native_windows_hid_pairing_addresses_for_startup();
    if native_windows_hid_addresses.contains(&address) {
        match open_listener_ota_v1_target_for_current_native_windows_hid_service_endpoint(
            &native_windows_hid_addresses,
            BLE_DISCOVERY_TIMEOUT,
        ) {
            Ok(target) => {
                remember_runtime_bluetooth_target_address_for_current(
                    address,
                    "native Windows HID OTA service-id endpoint",
                );
                log::info!(
                    "[embedded-ble] Listener OTA v1 selected current native Windows HID service-id endpoint after direct GATT miss"
                );
                return Ok(target);
            }
            Err(endpoint_error) => {
                return Err(format!(
                    "Listener OTA v1 handoff direct address path failed: {direct_error}; current native Windows HID service-id endpoint failed: {endpoint_error}"
                ));
            }
        }
    }
    log::warn!(
        "[embedded-ble] Listener OTA v1 handoff direct address path exhausted; falling back to normal service discovery: {direct_error}"
    );
    open_listener_ota_v1_target().map_err(|fallback_error| {
        format!(
            "Listener OTA v1 handoff direct address path failed: {direct_error}; normal service discovery fallback failed: {fallback_error}"
        )
    })
}

fn open_listener_ota_v1_target_after_active_link_handoff_with_deadline(
    deadline: Instant,
) -> Result<OpenListenerOtaV1Target, String> {
    let Some(address) = runtime_bluetooth_target_address() else {
        return open_listener_ota_v1_target_with_deadline(deadline);
    };

    let mut direct_error = None;
    for attempt in 1..=LISTENER_OTA_V1_HANDOFF_DISCOVERY_RETRY_DELAYS.len() + 1 {
        match open_listener_ota_v1_target_for_verified_active_handoff_with_deadline(
            address, deadline,
        ) {
            Ok(target) => {
                log::info!(
                    "[embedded-ble] Listener OTA v1 handoff preflight opened verified address {address:012X} on attempt {attempt}"
                );
                return Ok(target);
            }
            Err(err) => {
                direct_error = Some(err);
                let Some(delay) = LISTENER_OTA_V1_HANDOFF_DISCOVERY_RETRY_DELAYS
                    .get(attempt - 1)
                    .copied()
                else {
                    break;
                };
                if !is_transient_listener_ota_v1_discovery_error(
                    direct_error.as_deref().unwrap_or_default(),
                ) {
                    break;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let delay = delay.min(remaining);
                log::info!(
                    "[embedded-ble] Listener OTA v1 handoff preflight address {address:012X} not ready on attempt {attempt}; retrying in {} ms",
                    delay.as_millis()
                );
                std::thread::sleep(delay);
            }
        }
    }

    let direct_error = direct_error.unwrap_or_else(|| {
        "verified Listener address did not expose a writable OTA v1 service".to_string()
    });
    open_listener_ota_v1_target_with_deadline(deadline).map_err(|fallback_error| {
        format!(
            "Listener OTA v1 handoff preflight direct address path failed: {direct_error}; normal service discovery fallback failed: {fallback_error}"
        )
    })
}

fn open_listener_ota_v1_target_for_current_native_windows_hid_service_endpoint(
    addresses: &[u64],
    timeout: Duration,
) -> Result<OpenListenerOtaV1Target, String> {
    let selector =
        GattDeviceService::GetDeviceSelectorFromUuid(LISTENER_OTA_V1_SERVICE_UUID)
            .map_err(|err| format!("native Windows HID OTA service selector failed: {err}"))?;
    let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
        .map_err(|err| format!("native Windows HID OTA service query failed: {err}"))
        .and_then(|op| {
            wait_async_operation(op, timeout, "native Windows HID OTA service query")
        })?;
    let count = devices.Size().map_err(|err| {
        format!("native Windows HID OTA service collection size failed: {err}")
    })?;
    let mut last_error = None;

    for index in 0..count {
        let info = devices.GetAt(index).map_err(|err| {
            format!("native Windows HID OTA service entry {index} read failed: {err}")
        })?;
        let id = info.Id().map_err(|err| {
            format!("native Windows HID OTA service entry {index} id read failed: {err}")
        })?;
        let Some(address) = parse_bluetooth_address_from_device_id(&id.to_string_lossy())
        else {
            continue;
        };
        if !addresses.contains(&address) {
            continue;
        }

        // Windows can retain a service-id record while rejecting its uncached
        // characteristic query. Firmware pins the legacy OTA data-plane handles,
        // so this exact-identity endpoint may use its cached handle only after
        // the uncached attempt has failed.
        match open_listener_ota_v1_target_for_service_with_cache_policy(&id, true) {
            Ok(target) => return Ok(target),
            Err(err) => {
                last_error = Some(format!(
                "native Windows HID OTA service endpoint address={address:012X} failed: {err}"
            ))
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        "no current native Windows HID OTA service endpoint matched the paired identity"
            .to_string()
    }))
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
                addresses: candidates.iter().map(|(address, _, _)| *address).collect(),
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
    open_listener_ota_v1_target_for_device_with_options(address, false)
}

fn open_listener_ota_v1_target_for_verified_active_handoff(
    address: u64,
) -> Result<OpenListenerOtaV1Target, String> {
    open_listener_ota_v1_target_for_device_with_options(address, true)
}

fn open_listener_ota_v1_target_for_device_with_options(
    address: u64,
    verified_active_handoff: bool,
) -> Result<OpenListenerOtaV1Target, String> {
    let device = if verified_active_handoff {
        open_ble_device_by_address(address)?
    } else {
        open_ble_device(address)?
    };
    if !verified_active_handoff {
        if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
            wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "Listener OTA v1 device access")
                .ok()
        }) {
            if access != DeviceAccessStatus::Allowed
                && access != DeviceAccessStatus::Unspecified
            {
                return Err(format!(
                    "Listener OTA v1 device access denied status={access:?}"
                ));
            }
        }
    }

    let mut last_error = None;
    // A verified active link makes its device handle and access grant reusable, but
    // OTA control writes still require fresh GATT characteristic handles.
    let cache_modes = bluetooth_cache_modes_for_policy(
        denzic_ble_pairing::ota_device_control_cache_policy(verified_active_handoff),
    );
    for &cache_mode in cache_modes {
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
    open_listener_ota_v1_target_for_device_with_deadline_options(address, false, deadline)
}

fn open_listener_ota_v1_target_for_verified_active_handoff_with_deadline(
    address: u64,
    deadline: Instant,
) -> Result<OpenListenerOtaV1Target, String> {
    open_listener_ota_v1_target_for_device_with_deadline_options(address, true, deadline)
}

fn open_listener_ota_v1_target_for_device_with_deadline_options(
    address: u64,
    verified_active_handoff: bool,
    deadline: Instant,
) -> Result<OpenListenerOtaV1Target, String> {
    let open_timeout = remaining_ble_timeout(
        deadline,
        BLE_DISCOVERY_TIMEOUT,
        "Listener OTA v1 device open",
    )?;
    let device = if verified_active_handoff {
        open_ble_device_by_address_with_timeout(address, open_timeout)?
    } else {
        open_ble_device_with_timeout(address, open_timeout)?
    };
    if !verified_active_handoff {
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
            if access != DeviceAccessStatus::Allowed
                && access != DeviceAccessStatus::Unspecified
            {
                return Err(format!(
                    "Listener OTA v1 device access denied status={access:?}"
                ));
            }
        }
    }

    let mut last_error = None;
    let cache_modes = bluetooth_cache_modes_for_policy(
        denzic_ble_pairing::ota_device_control_cache_policy(verified_active_handoff),
    );
    for &cache_mode in cache_modes {
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
    for &cache_mode in
        bluetooth_cache_modes_for_policy(denzic_ble_pairing::DIAGNOSTIC_CACHE_POLICY)
    {
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
    for &cache_mode in
        bluetooth_cache_modes_for_policy(denzic_ble_pairing::STATUS_PROBE_CACHE_POLICY)
    {
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

fn open_notify_target_for_current_native_windows_hid(
    address: u64,
) -> Result<OpenNotifyTarget, String> {
    open_notify_target_for_device_with_cache_modes_and_timeout(
        address,
        // A persisted native-HID bond can have a valid system GATT cache
        // while Windows is temporarily unable to complete a fresh
        // Uncached service query (for example during link rehydration).
        // Keep the exact current address, then fall back to the device.
        bluetooth_cache_modes_for_policy(
            denzic_ble_pairing::PERSISTED_BOND_NOTIFY_CACHE_POLICY,
        ),
        STARTUP_NATIVE_HID_PERSISTED_GATT_TIMEOUT,
    )
    .and_then(|target| {
        require_audio_control_for_notify_target(target, "current native Windows HID address")
    })
}

fn open_notify_target_for_post_confirm_native_windows_hid(
    address: u64,
) -> Result<OpenNotifyTarget, String> {
    // The confirmed image retains its GATT schema. Rehydrate the Windows
    // system cache first, while preserving an uncached fallback for a
    // Service Changed/schema transition.
    open_notify_target_for_device_with_cache_modes_and_timeout(
        address,
        bluetooth_cache_modes_for_policy(denzic_ble_pairing::POST_CONFIRM_NOTIFY_CACHE_POLICY),
        STARTUP_NATIVE_HID_PERSISTED_GATT_TIMEOUT,
    )
    .and_then(|target| {
        require_audio_control_for_notify_target(
            target,
            "post-confirm native Windows HID address",
        )
    })
}

// Type readiness requires the control characteristic as well as audio notify.
// Otherwise the capture can receive packets but cannot send its heartbeat or
// recording lifecycle control, leaving it in a permanent half-ready state.
fn require_audio_control_for_notify_target(
    target: OpenNotifyTarget,
    source: &str,
) -> Result<OpenNotifyTarget, String> {
    if target.control.is_some() {
        return Ok(target);
    }

    Err(format!(
        "BLE audio control unavailable while opening notify target for {source}; retrying before capture starts"
    ))
}

fn open_notify_target_for_device(address: u64) -> Result<OpenNotifyTarget, String> {
    open_notify_target_for_device_with_cache_modes(
        address,
        bluetooth_cache_modes_for_policy(denzic_ble_pairing::DEVICE_NOTIFY_CACHE_POLICY),
    )
}

fn open_notify_target_for_device_with_cache_modes(
    address: u64,
    cache_modes: &[BluetoothCacheMode],
) -> Result<OpenNotifyTarget, String> {
    open_notify_target_for_device_with_cache_modes_and_timeout(
        address,
        cache_modes,
        BLE_DISCOVERY_TIMEOUT,
    )
}

fn open_notify_target_for_device_with_cache_modes_and_timeout(
    address: u64,
    cache_modes: &[BluetoothCacheMode],
    timeout: Duration,
) -> Result<OpenNotifyTarget, String> {
    let timeout = timeout.min(BLE_DISCOVERY_TIMEOUT);
    let device = open_ble_device_with_timeout(address, timeout)?;
    if let Some(access) = device
        .RequestAccessAsync()
        .ok()
        .and_then(|op| wait_async_operation(op, timeout, "device access").ok())
    {
        if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
            return Err(format!("BLE device access denied status={access:?}"));
        }
    }

    let mut last_error = None;
    for &cache_mode in cache_modes {
        let services_result = match device
            .GetGattServicesForUuidWithCacheModeAsync(SERVICE_UUID, cache_mode)
            .map_err(|err| format!("BLE {cache_mode:?} service discovery failed: {err}"))
            .and_then(|op| {
                wait_async_operation(op, timeout, &format!("{cache_mode:?} service")).map_err(
                    |err| format!("BLE {cache_mode:?} service discovery wait failed: {err}"),
                )
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
            match open_notify_characteristic_from_service_with_timeout(
                &service, cache_mode, timeout,
            ) {
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
    for &cache_mode in
        bluetooth_cache_modes_for_policy(denzic_ble_pairing::CONTROL_WRITE_CACHE_POLICY)
    {
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

fn open_ble_device_by_address(address: u64) -> Result<BluetoothLEDevice, String> {
    open_ble_device_by_address_with_timeout(address, BLE_DISCOVERY_TIMEOUT)
}

fn open_ble_device_with_timeout(
    address: u64,
    timeout: Duration,
) -> Result<BluetoothLEDevice, String> {
    denzic_ble_windows::open_ble_device_with_timeout(
        address,
        timeout,
        native_windows_hid_address_uses_random_identity(address),
        &ble_wait_cancel(),
    )
}

fn open_ble_device_by_address_with_timeout(
    address: u64,
    timeout: Duration,
) -> Result<BluetoothLEDevice, String> {
    denzic_ble_windows::open_ble_device_by_address_with_timeout(
        address,
        timeout,
        native_windows_hid_address_uses_random_identity(address),
        &ble_wait_cancel(),
    )
}

fn native_windows_hid_address_uses_random_identity(address: u64) -> bool {
    native_windows_hid_current_address_uses_random_identity(
        address,
        &native_windows_hid_pairing_addresses_for_startup(),
    )
}

fn open_listener_ota_v1_target_for_service(
    service_id: &HSTRING,
) -> Result<OpenListenerOtaV1Target, String> {
    open_listener_ota_v1_target_for_service_with_cache_policy(service_id, true)
}

fn open_listener_ota_v1_target_for_service_with_cache_policy(
    service_id: &HSTRING,
    allow_cached: bool,
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
    let cache_modes = bluetooth_cache_modes_for_policy(
        denzic_ble_pairing::ota_service_endpoint_cache_policy(allow_cached),
    );
    for &cache_mode in cache_modes {
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
    for &cache_mode in bluetooth_cache_modes_for_policy(
        denzic_ble_pairing::DEADLINE_SERVICE_ENDPOINT_CACHE_POLICY,
    ) {
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

fn open_notify_target_for_service_with_timeout(
    service_id: &HSTRING,
    timeout: Duration,
) -> Result<OpenNotifyTarget, String> {
    let service = GattDeviceService::FromIdAsync(service_id)
        .map_err(|err| format!("BLE service open failed: {err}"))
        .and_then(|op| wait_async_operation(op, timeout, "service open"))?;

    let prepared = open_notify_characteristic_from_service_with_timeout(
        &service,
        BluetoothCacheMode::Uncached,
        timeout,
    )?;
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

fn open_notify_target_for_service(service_id: &HSTRING) -> Result<OpenNotifyTarget, String> {
    open_notify_target_for_service_with_timeout(service_id, BLE_DISCOVERY_TIMEOUT)
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

pub(super) fn listener_ota_v1_sync_control_uses_status_write(
    packet: &[u8; denzic_ota_core::CONTROL_BYTES],
) -> bool {
    packet[4] == denzic_ota_core::OP_SYNC
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
    open_notify_characteristic_from_service_with_timeout(
        service,
        cache_mode,
        BLE_DISCOVERY_TIMEOUT,
    )
}

fn open_notify_characteristic_from_service_with_timeout(
    service: &GattDeviceService,
    cache_mode: BluetoothCacheMode,
    timeout: Duration,
) -> Result<PreparedNotifyCharacteristic, String> {
    let timeout = timeout.min(BLE_DISCOVERY_TIMEOUT);
    if let Some(access) = service
        .RequestAccessAsync()
        .ok()
        .and_then(|op| wait_async_operation(op, timeout, "service access").ok())
    {
        if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
            return Err(format!("BLE service access denied status={access:?}"));
        }
    }
    let session = prepare_gatt_session(service, GATT_READY_TIMEOUT.min(timeout))?;
    let control = if timeout == BLE_DISCOVERY_TIMEOUT {
        open_optional_audio_control_for_notify_setup(service, cache_mode)
    } else {
        open_write_characteristic_from_service_with_timeout(
            service,
            AUDIO_CONTROL_UUID,
            "native Windows HID audio control",
            cache_mode,
            timeout,
        )
        .ok()
    };

    let result = service
        .GetCharacteristicsForUuidWithCacheModeAsync(NOTIFY_UUID, cache_mode)
        .map_err(|err| format!("BLE characteristic discovery failed: {err}"))?
        .wait_ble_result(timeout, "notify characteristic")?;
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
    gatt_ready_timeout: Duration,
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
        remaining_ble_timeout(deadline, gatt_ready_timeout, "persisted startup GATT ready")?,
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

fn read_characteristic_bytes(
    characteristic: &GattCharacteristic,
    cache_mode: BluetoothCacheMode,
    label: &str,
) -> Result<Vec<u8>, String> {
    denzic_ble_windows::read_characteristic_bytes(
        characteristic,
        cache_mode,
        label,
        &ble_wait_cancel(),
    )
}

fn read_characteristic_bytes_with_timeout(
    characteristic: &GattCharacteristic,
    cache_mode: BluetoothCacheMode,
    label: &str,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    denzic_ble_windows::read_characteristic_bytes_with_timeout(
        characteristic,
        cache_mode,
        label,
        timeout,
        &ble_wait_cancel(),
    )
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
) -> Result<(u16, u32, u32, &[u8]), String> {
    let chunk = denzic_observability_v1_core::parse_diag_log_chunk(packet).map_err(|err| {
        match err {
            denzic_observability_v1_core::DiagLogChunkError::TooShort { packet_len } => {
                format!(
                    "BLE diagnostic notification too short at offset {host_offset}: {packet_len} bytes"
                )
            }
            denzic_observability_v1_core::DiagLogChunkError::EmptyChunk => {
                format!("BLE diagnostic empty chunk at offset {host_offset}")
            }
            denzic_observability_v1_core::DiagLogChunkError::PayloadLengthMismatch {
                event_count,
                actual,
                expected,
            } => format!(
                "BLE diagnostic chunk payload length mismatch: offset={host_offset} count={event_count} bytes={actual} expected={expected}"
            ),
        }
    })?;
    Ok((
        chunk.event_count,
        chunk.global_offset,
        chunk.events_crc32,
        chunk.payload,
    ))
}

fn write_cccd_notify_with_retry(
    capture_id: u64,
    label: &str,
    characteristic: &GattCharacteristic,
    timeout: Duration,
    recovery_probe_address: Option<u64>,
) -> Result<GattCommunicationStatus, String> {
    let recovery_error =
        |err: &str| cccd_notify_recovery_pairing_error(err, recovery_probe_address);
    denzic_ble_windows::write_cccd_with_retry(
        label,
        capture_id,
        characteristic,
        GattClientCharacteristicConfigurationDescriptorValue::Notify,
        timeout,
        &CCCD_ENABLE_RETRY_DELAYS,
        Some(&recovery_error),
    )
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
    denzic_ble_windows::write_cccd_with_retry(
        label,
        transfer_id,
        characteristic,
        GattClientCharacteristicConfigurationDescriptorValue::Indicate,
        timeout,
        &CCCD_ENABLE_RETRY_DELAYS,
        None,
    )
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
    denzic_ble_windows::wait_async_operation_with_cancel(
        operation,
        timeout,
        label,
        &ble_wait_cancel(),
    )
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

    fn write_ec11_recovery_acknowledgement(&self) -> Result<(), String> {
        let Some(control) = self.target.control.as_ref() else {
            return Err("audio control unavailable".to_string());
        };
        write_audio_control_value_with_timeout(
            control,
            super::EC11_HARDWARE_RECOVERY_ACK,
            EC11_HARDWARE_RECOVERY_ACK_WRITE_TIMEOUT,
            "EC11 recovery acknowledgement",
        )
    }

    fn write_ec11_recovery_prepare_acknowledgement(&self) -> Result<(), String> {
        let Some(control) = self.target.control.as_ref() else {
            return Err("audio control unavailable".to_string());
        };
        write_audio_control_value_with_timeout(
            control,
            super::EC11_HARDWARE_RECOVERY_PREPARE_ACK,
            EC11_HARDWARE_RECOVERY_ACK_WRITE_TIMEOUT,
            "EC11 recovery pre-authorization acknowledgement",
        )
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
    if bytes == b"VREC:TOGGLE\n"
        || bytes == b"VREC:STOP\n"
        || bytes == super::EC11_HARDWARE_RECOVERY_ACK
        || bytes == super::EC11_HARDWARE_RECOVERY_PREPARE_ACK
    {
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
#[path = "../windows_ble_tests.rs"]
mod tests;
