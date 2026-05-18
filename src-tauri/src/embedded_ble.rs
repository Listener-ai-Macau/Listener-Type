//! Windows BLE receiver for the embedded VKA1 audio service.
//!
//! This module only owns BLE discovery/subscription. Protocol parsing and
//! dictation finalization stay in `embedded_audio` and `coordinator`.

use std::time::Duration;

#[cfg(target_os = "windows")]
mod windows_ble {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use windows::core::{GUID, HSTRING};
    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
        GattCommunicationStatus, GattDeviceService, GattValueChangedEventArgs,
    };
    use windows::Devices::Bluetooth::BluetoothCacheMode;
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
    use windows::Storage::Streams::{DataReader, IBuffer};

    const SERVICE_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091a);
    const NOTIFY_UUID: GUID = GUID::from_u128(0x710af845_6d9f_6583_0c4d_9e5b3bc3091b);

    pub fn capture_notifications_once(timeout: Duration) -> Result<Vec<Vec<u8>>, String> {
        let characteristic = open_notify_characteristic()?;
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

        let token = characteristic
            .ValueChanged(&handler)
            .map_err(|err| format!("BLE ValueChanged handler registration failed: {err}"))?;
        let mut cleanup = NotifyCleanup::new(characteristic.clone(), token);
        let status = characteristic
            .WriteClientCharacteristicConfigurationDescriptorAsync(
                GattClientCharacteristicConfigurationDescriptorValue::Notify,
            )
            .map_err(|err| format!("BLE CCCD notify write failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE CCCD notify write wait failed: {err}"))?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("BLE CCCD notify write returned status={status:?}"));
        }

        let deadline = Instant::now() + timeout;
        let mut notifications = Vec::new();
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "BLE embedded audio capture timed out after {} ms",
                    timeout.as_millis()
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            let notification = rx
                .recv_timeout(remaining)
                .map_err(|err| format!("BLE embedded audio notification wait failed: {err}"))?;
            let terminal = is_terminal_notification(&notification);
            notifications.push(notification);
            if terminal {
                cleanup.disable_notify();
                return Ok(notifications);
            }
        }
    }

    fn open_notify_characteristic() -> Result<GattCharacteristic, String> {
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
            match open_notify_characteristic_for_service(&id) {
                Ok(characteristic) => {
                    log::info!("[embedded-ble] selected service index={index} name={name}");
                    return Ok(characteristic);
                }
                Err(err) => {
                    last_error = Some(format!("{name}: {err}"));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            "No subscribable embedded audio BLE notify characteristic found".to_string()
        }))
    }

    fn open_notify_characteristic_for_service(
        service_id: &HSTRING,
    ) -> Result<GattCharacteristic, String> {
        let service = GattDeviceService::FromIdAsync(service_id)
            .map_err(|err| format!("BLE service open failed: {err}"))?
            .get()
            .map_err(|err| format!("BLE service open wait failed: {err}"))?;
        if let Ok(session) = service.Session() {
            if session.CanMaintainConnection().unwrap_or(false) {
                let _ = session.SetMaintainConnection(true);
            }
        }

        let result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(NOTIFY_UUID, BluetoothCacheMode::Uncached)
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
        characteristics
            .GetAt(0)
            .map_err(|err| format!("BLE notify characteristic read failed: {err}"))
    }

    fn buffer_to_vec(buffer: &IBuffer) -> windows::core::Result<Vec<u8>> {
        let length = buffer.Length()? as usize;
        let reader = DataReader::FromBuffer(buffer)?;
        let mut bytes = vec![0u8; length];
        reader.ReadBytes(&mut bytes)?;
        Ok(bytes)
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

    struct NotifyCleanup {
        characteristic: GattCharacteristic,
        token: Option<EventRegistrationToken>,
        notify_disabled: bool,
    }

    impl NotifyCleanup {
        fn new(characteristic: GattCharacteristic, token: EventRegistrationToken) -> Self {
            Self {
                characteristic,
                token: Some(token),
                notify_disabled: false,
            }
        }

        fn disable_notify(&mut self) {
            if self.notify_disabled {
                return;
            }
            let _ = self
                .characteristic
                .WriteClientCharacteristicConfigurationDescriptorAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::None,
                )
                .and_then(|operation| operation.get().map(|_| ()));
            self.notify_disabled = true;
        }
    }

    impl Drop for NotifyCleanup {
        fn drop(&mut self) {
            self.disable_notify();
            if let Some(token) = self.token.take() {
                let _ = self.characteristic.RemoveValueChanged(token);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub fn capture_notifications_once(timeout: Duration) -> Result<Vec<Vec<u8>>, String> {
    windows_ble::capture_notifications_once(timeout)
}

#[cfg(not(target_os = "windows"))]
pub fn capture_notifications_once(_timeout: Duration) -> Result<Vec<Vec<u8>>, String> {
    Err("Embedded BLE audio input is only supported on Windows".to_string())
}
