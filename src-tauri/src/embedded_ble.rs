//! Windows BLE receiver for the embedded VKA1 audio service.
//!
//! This module only owns BLE discovery/subscription. Protocol parsing and
//! dictation finalization stay in `embedded_audio` and `coordinator`.

use std::time::Duration;

use serde::Serialize;

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
    pub dis_service_uuid: &'static str,
    pub configured_device_address: Option<String>,
    pub audio_services: Vec<BleDiagnosticServiceEntry>,
    pub ota_services: Vec<BleDiagnosticServiceEntry>,
    pub firmware_snapshot: FirmwareOtaDeviceSnapshot,
    pub errors: Vec<String>,
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
    } else if ble_error_suggests_low_power_idle_disconnect(&lower) {
        BleFailureKind::LowPowerIdleDisconnect
    } else if ble_error_suggests_device_asleep(&lower) {
        BleFailureKind::DeviceAsleep
    } else if lower.contains("stale")
        || lower.contains("unknown gatt")
        || (lower.contains("cached") && !lower.contains("uncached"))
        || lower.contains("gatt cache")
        || lower.contains("service changed")
    {
        BleFailureKind::StaleGattService
    } else if lower.contains("unreachable")
        || lower.contains("disconnected")
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
            "Press KEY4 or the wake key, wait for the device to reconnect, then retry.",
        ),
        BleFailureKind::MissingPairing => (
            true,
            false,
            "Pair the Listener device in Windows Bluetooth, then return and refresh Listener BLE.",
        ),
        BleFailureKind::LowPowerIdleDisconnect => (
            true,
            true,
            "Listener BLE entered low-power idle; retrying will reconnect, or press KEY4 if the device is asleep.",
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
    lower.contains("reason=546")
        || lower.contains("reason: 546")
        || lower.contains("reason 546")
        || lower.contains("reason=0x222")
        || lower.contains("reason: 0x222")
        || lower.contains("low-power idle")
        || lower.contains("low power idle")
        || lower.contains("idle disconnect")
        || lower.contains("idle-disconnect")
        || lower.contains("intentional idle")
        || lower.contains("transport_not_ready")
        || lower.contains("transport not ready")
}

fn utc_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn format_bluetooth_address(address: u64) -> String {
    format!("{address:012X}")
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};
    use std::time::{Duration, Instant};

    use windows::core::{GUID, HSTRING};
    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCharacteristic, GattCharacteristicProperties,
        GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
        GattDeviceService, GattSession, GattSessionStatus, GattValueChangedEventArgs,
        GattWriteOption, GattWriteResult,
    };
    use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
    use windows::Devices::Enumeration::{DeviceAccessStatus, DeviceInformation};
    use windows::Foundation::{
        AsyncStatus, EventRegistrationToken, IAsyncOperation, TypedEventHandler,
    };
    use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};

    const SERVICE_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091a);
    const NOTIFY_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091b);
    const OTA_SERVICE_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3092a);
    const OTA_CONTROL_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3092b);
    const OTA_DATA_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3092c);
    const OTA_READINESS_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091c);
    const OTA_CAPABILITIES_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091d);
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
    const GATT_READY_TIMEOUT: Duration = Duration::from_secs(8);
    const GATT_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
    const CCCD_ENABLE_TIMEOUT: Duration = Duration::from_secs(8);
    const CCCD_ENABLE_RETRY_DELAYS: [Duration; 3] = [
        Duration::from_millis(250),
        Duration::from_millis(750),
        Duration::from_millis(1500),
    ];
    const OTA_WRITE_TIMEOUT: Duration = Duration::from_secs(8);
    const OTA_FINISH_WRITE_TIMEOUT: Duration = Duration::from_secs(45);
    const BLE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
    const ATT_WRITE_HEADER_BYTES: usize = 3;
    const ATT_DEFAULT_PAYLOAD_BYTES: usize = 20;
    const SERVICE_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3091a";
    const OTA_SERVICE_UUID_TEXT: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092a";
    const DIS_SERVICE_UUID_TEXT: &str = "0000180a-0000-1000-8000-00805f9b34fb";
    const OTA_REQUIRED_DATA_CHUNK_BYTES: usize = 500;

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
        let firmware_snapshot = firmware_ota_device_snapshot();
        if let Some(detail) = firmware_snapshot.detail.as_ref() {
            errors.push(detail.clone());
        }

        crate::embedded_ble::BleDiagnosticSnapshot {
            captured_at: crate::embedded_ble::utc_now_rfc3339(),
            platform: "windows",
            audio_service_uuid: SERVICE_UUID_TEXT,
            ota_service_uuid: OTA_SERVICE_UUID_TEXT,
            dis_service_uuid: DIS_SERVICE_UUID_TEXT,
            configured_device_address: configured_bluetooth_address()
                .map(crate::embedded_ble::format_bluetooth_address),
            audio_services,
            ota_services,
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

    pub fn probe_notify_subscription(timeout: Duration) -> Result<(), String> {
        let capture_guard = BleCaptureGuard::enter(Some(timeout))?;
        let capture_id = capture_guard.session_id();
        let target = open_notify_target()?;
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
        let capture_guard = BleCaptureGuard::enter(idle_timeout)?;
        let capture_id = capture_guard.session_id();
        let target = open_notify_target()?;
        let characteristic = target.characteristic.clone();
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

        let mut cleanup = NotifyCleanup::new(capture_id, target);
        let token = characteristic
            .ValueChanged(&handler)
            .map_err(|err| format!("BLE ValueChanged handler registration failed: {err}"))?;
        cleanup.set_token(token);
        log::info!("[embedded-ble] capture #{capture_id}: ValueChanged handler registered");

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

        let deadline = idle_timeout.map(|timeout| Instant::now() + timeout);
        let mut collector = crate::embedded_audio::SessionCollector::default();
        let mut stop_drain_deadline: Option<Instant> = None;
        loop {
            let now = Instant::now();
            if cancel_requested.load(Ordering::SeqCst) {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: cancelled by caller; closing notify"
                );
                cleanup.disable_notify();
                return Ok(());
            }
            if deadline.is_some_and(|deadline| now >= deadline) {
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
            let receive_timeout = stop_drain_deadline
                .map(|drain_deadline| drain_deadline.saturating_duration_since(now))
                .or_else(|| deadline.map(|deadline| deadline.saturating_duration_since(now)))
                .unwrap_or(RECEIVE_POLL_INTERVAL)
                .min(RECEIVE_POLL_INTERVAL);
            let notification = match rx.recv_timeout(receive_timeout) {
                Ok(notification) => notification,
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
                    continue;
                }
                Err(err) => {
                    return Err(format!(
                        "BLE embedded audio notification wait failed: {err}"
                    ));
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
                cleanup.disable_notify();
                return Ok(());
            }
            if collector.terminal_received() && stop_drain_deadline.is_some() {
                stop_drain_deadline = Some(Instant::now() + super::STOP_DRAIN_TIMEOUT);
            }
        }
    }

    pub(super) struct PreparedFirmwareOtaTransfer {
        target: OpenOtaTarget,
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
        write_gatt_value_with_timeout(
            &target.control,
            finish.as_bytes(),
            GattWriteOption::WriteWithResponse,
            OTA_FINISH_WRITE_TIMEOUT,
            "OTA control finish",
        )?;
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

    pub fn firmware_ota_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        match open_ota_target() {
            Ok(target) => firmware_ota_device_snapshot_from_target(&target),
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
            });
            if let Some(readiness) = ota_readiness {
                snapshot.hardware_revision =
                    readiness_field(&readiness, "model").or(snapshot.hardware_revision);
                snapshot.firmware_version =
                    readiness_field(&readiness, "fw_version").or(snapshot.firmware_version);
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
            });
            if let Some(capabilities) = ota_capabilities {
                let parsed = split_capability_tokens(&capabilities);
                if parsed.iter().any(|item| item == "firmware_ota_v1") {
                    snapshot.capabilities = parsed;
                }
            }
        }
        let (dis_model, dis_hardware, dis_firmware, dis_battery) =
            read_dis_metadata_from_discovered_services(target.bluetooth_address);
        if snapshot.hardware_revision.is_none() {
            snapshot.hardware_revision = dis_model.or(dis_hardware);
        }
        if snapshot.firmware_version.is_none() {
            snapshot.firmware_version = dis_firmware;
        }
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
                snapshot.hardware_revision = model.or(hardware);
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
        if count == 0 {
            return Err(format!(
                "Embedded audio BLE service {SERVICE_UUID:?} not found; ensure device is paired and online"
            ));
        }

        let mut last_error = None;
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

            let mut candidate_error = None;
            if let Some(address) = parse_bluetooth_address_from_device_id(&id.to_string_lossy()) {
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

        Err(last_error.unwrap_or_else(|| {
            "No subscribable embedded audio BLE notify characteristic found".to_string()
        }))
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
        for index in 0..services.Size().ok()? {
            let info = services.GetAt(index).ok()?;
            let name = info
                .Name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            if !name.eq_ignore_ascii_case("listener") {
                continue;
            }
            let id = info.Id().ok()?;
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
        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(characteristic_uuid, cache_mode)
            .ok()?
            .get()
            .ok()?;
        if result.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let characteristics = result.Characteristics().ok()?;
        if characteristics.Size().ok()? == 0 {
            return None;
        }
        let characteristic = characteristics.GetAt(0).ok()?;
        let read = characteristic
            .ReadValueWithCacheModeAsync(cache_mode)
            .ok()?
            .get()
            .ok()?;
        if read.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        buffer_to_vec(&read.Value().ok()?).ok()
    }

    fn open_ota_target() -> Result<OpenOtaTarget, String> {
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

        let mut last_error = None;
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

            let mut candidate_error = None;
            if let Some(address) = parse_bluetooth_address_from_device_id(&id.to_string_lossy()) {
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
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
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

    fn open_notify_target_for_service(service_id: &HSTRING) -> Result<OpenNotifyTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE service open failed: {err}"))
            .and_then(|op| wait_async_operation(op, BLE_DISCOVERY_TIMEOUT, "service open"))?;

        let prepared =
            open_notify_characteristic_from_service(&service, BluetoothCacheMode::Uncached)?;
        Ok(OpenNotifyTarget {
            characteristic: prepared.characteristic,
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
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT);
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
        if !data_properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
            return Err(
                "BLE OTA data characteristic must support WriteWithoutResponse.".to_string(),
            );
        }
        let data_write_option = GattWriteOption::WriteWithoutResponse;
        let data_chunk_bytes = ota_data_chunk_bytes(session.as_ref());
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

    fn ota_data_chunk_bytes(session: Option<&GattSession>) -> usize {
        let payload_bytes = session
            .and_then(|session| session.MaxPduSize().ok())
            .map(|max_pdu_size| usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES))
            .filter(|payload_bytes| *payload_bytes > 0)
            .unwrap_or(ATT_DEFAULT_PAYLOAD_BYTES);
        payload_bytes.max(1)
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
        if transport_limit_bytes < OTA_REQUIRED_DATA_CHUNK_BYTES {
            return Err(format!(
                "BLE transport payload limit is {transport_limit_bytes} bytes; OTA requires {OTA_REQUIRED_DATA_CHUNK_BYTES} bytes."
            ));
        }
        Ok(OTA_REQUIRED_DATA_CHUNK_BYTES)
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
        let session = prepare_gatt_session(service, GATT_READY_TIMEOUT);

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
            session,
        })
    }

    fn prepare_gatt_session(service: &GattDeviceService, timeout: Duration) -> Option<GattSession> {
        let session = match service.Session() {
            Ok(session) => session,
            Err(err) => {
                log::warn!("[embedded-ble] GATT session unavailable: {err}");
                return None;
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
            log::warn!(
                "[embedded-ble] GATT session still not active after {} ms initial={:?} current={:?}; continuing",
                timeout.as_millis(),
                initial_status,
                session.SessionStatus().ok()
            );
        }
        Some(session)
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

    fn parse_bluetooth_address_hex(value: &str) -> Option<u64> {
        let hex: String = value
            .chars()
            .filter(|ch| ch.is_ascii_hexdigit())
            .take(12)
            .collect();
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
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
    }

    struct PreparedNotifyCharacteristic {
        characteristic: GattCharacteristic,
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

    struct PreparedOtaCharacteristics {
        control: GattCharacteristic,
        data: GattCharacteristic,
        data_write_option: GattWriteOption,
        data_chunk_bytes: usize,
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

    struct NotifyCleanup {
        capture_id: u64,
        target: OpenNotifyTarget,
        token: Option<EventRegistrationToken>,
        notify_disabled: bool,
    }

    impl NotifyCleanup {
        fn new(capture_id: u64, target: OpenNotifyTarget) -> Self {
            Self {
                capture_id,
                target,
                token: None,
                notify_disabled: false,
            }
        }

        fn set_token(&mut self, token: EventRegistrationToken) {
            self.token = Some(token);
        }

        fn disable_notify(&mut self) {
            self.finish(NotifyCccdTeardown::Disable);
        }

        fn finish(&mut self, teardown: NotifyCccdTeardown) {
            if self.notify_disabled {
                return;
            }
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
            self.notify_disabled = true;
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

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum NotifyCccdTeardown {
        Disable,
        LeaveEnabled,
    }

    impl NotifyCccdTeardown {
        fn for_probe_success() -> Self {
            Self::LeaveEnabled
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

        #[test]
        fn foreground_probe_success_leaves_notify_cccd_enabled() {
            assert_eq!(
                NotifyCccdTeardown::for_probe_success(),
                NotifyCccdTeardown::LeaveEnabled
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

#[cfg(target_os = "windows")]
pub struct FirmwareOtaPreparedTransfer(windows_ble::PreparedFirmwareOtaTransfer);

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

#[cfg(target_os = "windows")]
pub fn prepare_firmware_ota_transfer() -> Result<FirmwareOtaPreparedTransfer, String> {
    windows_ble::prepare_firmware_ota_transfer().map(FirmwareOtaPreparedTransfer)
}

#[cfg(target_os = "windows")]
pub fn firmware_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::firmware_ota_device_snapshot()
}

#[cfg(target_os = "windows")]
pub fn ble_diagnostic_snapshot() -> BleDiagnosticSnapshot {
    windows_ble::diagnostic_snapshot()
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
pub fn transfer_firmware_ota(
    _version: &str,
    _firmware_sha256: &str,
    _firmware_bytes: &[u8],
    _manifest_chunk_bytes: usize,
    _on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FirmwareOtaTransferStats, String> {
    Err("Firmware OTA over Listener BLE is only supported on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub struct FirmwareOtaPreparedTransfer;

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

#[cfg(not(target_os = "windows"))]
pub fn prepare_firmware_ota_transfer() -> Result<FirmwareOtaPreparedTransfer, String> {
    Err("Firmware OTA over Listener BLE is only supported on Windows".to_string())
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
pub fn ble_diagnostic_snapshot() -> BleDiagnosticSnapshot {
    let detail = "Listener BLE diagnostics are only supported on Windows".to_string();
    BleDiagnosticSnapshot {
        captured_at: utc_now_rfc3339(),
        platform: std::env::consts::OS,
        audio_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3091a",
        ota_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3092a",
        dis_service_uuid: "0000180a-0000-1000-8000-00805f9b34fb",
        configured_device_address: None,
        audio_services: Vec::new(),
        ota_services: Vec::new(),
        firmware_snapshot: firmware_ota_device_snapshot(),
        errors: vec![detail],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded_audio::{
        build_audio_data_notification, build_session_cancel_notification,
        build_session_error_notification, build_session_start_notification,
        build_session_stop_notification, SessionErrorCode,
    };

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
                "stale cached GATT path after BLE reason=546 returned transport_not_ready",
                BleFailureKind::LowPowerIdleDisconnect,
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
    fn ota_chunk_selection_uses_manifest_and_transport_limits() {
        assert_eq!(
            super::windows_ble::ota_transfer_chunk_bytes(514, 500),
            Ok(500)
        );
        assert!(super::windows_ble::ota_transfer_chunk_bytes(499, 500).is_err());
        assert!(super::windows_ble::ota_transfer_chunk_bytes(514, 499).is_err());
    }
}
