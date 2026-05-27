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
    const OTA_CHUNK_BYTES: usize = 180;

    pub fn capture_notifications_once(timeout: Duration) -> Result<Vec<Vec<u8>>, String> {
        let mut notifications = Vec::new();
        capture_notification_events(timeout, &mut |event| {
            notifications.push(event.notification);
            Ok(())
        })?;
        Ok(notifications)
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

    pub fn transfer_firmware_ota(
        version: &str,
        firmware_sha256: &str,
        firmware_bytes: &[u8],
    ) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
        if firmware_bytes.is_empty() {
            return Err("firmware_ota.bin is empty.".to_string());
        }

        let transfer_guard = BleCaptureGuard::enter(None)?;
        let transfer_id = transfer_guard.session_id();
        let target = open_ota_target()?;
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
        for chunk in firmware_bytes.chunks(OTA_CHUNK_BYTES) {
            write_gatt_value_with_timeout(
                &target.data,
                chunk,
                target.data_write_option,
                OTA_WRITE_TIMEOUT,
                "OTA data",
            )?;
            chunks_sent += 1;
        }

        let finish = format!(
            "{{\"op\":\"finish\",\"size\":{},\"sha256\":\"{}\"}}\n",
            firmware_bytes.len(),
            json_escape(firmware_sha256)
        );
        write_gatt_value_with_timeout(
            &target.control,
            finish.as_bytes(),
            GattWriteOption::WriteWithResponse,
            OTA_WRITE_TIMEOUT,
            "OTA control finish",
        )?;
        log::info!(
            "[embedded-ble] ota #{transfer_id}: transferred {} bytes in {chunks_sent} chunks",
            firmware_bytes.len()
        );
        Ok(crate::embedded_ble::FirmwareOtaTransferStats {
            bytes_transferred: firmware_bytes.len(),
            chunks_sent,
            transport: "listener_ble_ota",
        })
    }

    pub fn firmware_ota_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        match open_ota_target() {
            Ok(target) => {
                let mut snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                    connected: true,
                    hardware_revision: None,
                    firmware_version: None,
                    capabilities: vec!["firmware_ota_v1".to_string()],
                    battery_percent: None,
                    usb_powered: None,
                    detail: None,
                };
                if let Some(device) = target.device.as_ref() {
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
                    snapshot.firmware_version = read_optional_string_characteristic(
                        device,
                        DIS_SERVICE_UUID,
                        DIS_FIRMWARE_REVISION_UUID,
                    );
                    snapshot.battery_percent = read_optional_u8_characteristic(
                        device,
                        BATTERY_SERVICE_UUID,
                        BATTERY_LEVEL_UUID,
                    );
                }
                snapshot
            }
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

    fn open_notify_target() -> Result<OpenNotifyTarget, String> {
        let selector = GattDeviceService::GetDeviceSelectorFromUuid(SERVICE_UUID)
            .map_err(|err| format!("BLE service selector failed: {err}"))?;
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|err| format!("BLE service discovery failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE service discovery wait failed: {err}"))?;
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

    fn read_optional_u8_characteristic(
        device: &BluetoothLEDevice,
        service_uuid: GUID,
        characteristic_uuid: GUID,
    ) -> Option<u8> {
        read_optional_characteristic_bytes(device, service_uuid, characteristic_uuid)
            .and_then(|bytes| bytes.first().copied())
    }

    fn read_optional_characteristic_bytes(
        device: &BluetoothLEDevice,
        service_uuid: GUID,
        characteristic_uuid: GUID,
    ) -> Option<Vec<u8>> {
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
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
            .map_err(|err| format!("BLE OTA service discovery failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE OTA service discovery wait failed: {err}"))?;
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
        if let Ok(access) = device
            .RequestAccessAsync()
            .and_then(|operation| operation.get())
        {
            if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
                return Err(format!("BLE OTA device access denied status={access:?}"));
            }
        }

        let mut last_error = None;
        for cache_mode in [BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] {
            let services_result = match device
                .GetGattServicesForUuidWithCacheModeAsync(OTA_SERVICE_UUID, cache_mode)
                .map_err(|err| format!("BLE OTA {cache_mode:?} service discovery failed: {err}"))
                .and_then(|operation| {
                    operation.get().map_err(|err| {
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
            "No writable Listener BLE OTA characteristics found on device".to_string()
        }))
    }

    fn open_notify_target_for_device(address: u64) -> Result<OpenNotifyTarget, String> {
        let device = open_ble_device(address)?;
        if let Ok(access) = device
            .RequestAccessAsync()
            .and_then(|operation| operation.get())
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
                .and_then(|operation| {
                    operation.get().map_err(|err| {
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
            .map_err(|err| format!("BLE device open by address failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE device open by address wait failed: {err}"))?;

        let device_id = device
            .DeviceId()
            .map(|id| id.to_string_lossy())
            .unwrap_or_default();
        if device_id.is_empty() {
            return Ok(device);
        }

        match BluetoothLEDevice::FromIdAsync(&HSTRING::from(device_id.as_str()))
            .and_then(|operation| operation.get())
        {
            Ok(device_by_id) => {
                let _ = device.Close();
                Ok(device_by_id)
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] BLE device reopen by id failed, using address handle: {err}"
                );
                Ok(device)
            }
        }
    }

    fn open_ota_target_for_service(service_id: &HSTRING) -> Result<OpenOtaTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE OTA service open failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE OTA service open wait failed: {err}"))?;

        let prepared =
            open_ota_characteristics_from_service(&service, BluetoothCacheMode::Uncached)?;
        Ok(OpenOtaTarget {
            control: prepared.control,
            data: prepared.data,
            data_write_option: prepared.data_write_option,
            service: Some(service),
            session: prepared.session,
            device: None,
        })
    }

    fn open_notify_target_for_service(service_id: &HSTRING) -> Result<OpenNotifyTarget, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE service open failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE service open wait failed: {err}"))?;

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
        if let Ok(access) = service
            .RequestAccessAsync()
            .and_then(|operation| operation.get())
        {
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
        let data_write_option =
            if data_properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
                GattWriteOption::WriteWithoutResponse
            } else {
                GattWriteOption::WriteWithResponse
            };
        Ok(PreparedOtaCharacteristics {
            control,
            data,
            data_write_option,
            session,
        })
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
        if let Ok(access) = service
            .RequestAccessAsync()
            .and_then(|operation| operation.get())
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
        let suffix = upper
            .rsplit_once("DEV_")
            .map(|(_, suffix)| suffix)
            .or_else(|| upper.rsplit_once('_').map(|(_, suffix)| suffix))?;
        let hex: String = suffix
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
        service: Option<GattDeviceService>,
        session: Option<GattSession>,
        device: Option<BluetoothLEDevice>,
    }

    struct PreparedOtaCharacteristics {
        control: GattCharacteristic,
        data: GattCharacteristic,
        data_write_option: GattWriteOption,
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
) -> Result<FirmwareOtaTransferStats, String> {
    windows_ble::transfer_firmware_ota(version, firmware_sha256, firmware_bytes)
}

#[cfg(target_os = "windows")]
pub fn firmware_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
    windows_ble::firmware_ota_device_snapshot()
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
) -> Result<FirmwareOtaTransferStats, String> {
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
}
