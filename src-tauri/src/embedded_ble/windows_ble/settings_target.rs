// Included in windows_ble: explicit settings routes must never fall back to
// the voice target or a different USB port.
use super::settings_target::{SettingsDevice, SettingsIo};

fn settings_usb_id(port: &SerialPortInfo) -> String {
    match &port.port_type {
        SerialPortType::UsbPort(usb) => format!(
            "usb:{}:{:04x}:{:04x}:{}",
            port.port_name,
            usb.vid,
            usb.pid,
            usb.serial_number.as_deref().unwrap_or("")
        ),
        _ => String::new(),
    }
}

pub fn list_settings_devices() -> Result<Vec<SettingsDevice>, String> {
    let ports = serialport::available_ports().map_err(|e| format!("USB enumeration: {e}"))?;
    let mut result: Vec<SettingsDevice> = listener_usb_serial_candidates(&ports)
        .into_iter()
        .map(|port| {
            let serial = match &port.port_type {
                SerialPortType::UsbPort(usb) => usb.serial_number.clone().unwrap_or_default(),
                _ => String::new(),
            };
            SettingsDevice {
                id: settings_usb_id(&port),
                transport: "usb".into(),
                label: format!("{} · {}", port.port_name, serial),
            }
        })
        .collect();
    let mut connected_addresses = std::collections::HashSet::new();
    let mut paired_names = std::collections::HashMap::new();
    // Read only the local pairing index; no GATT connections are opened to
    // populate names or decide which device to offer first.
    if let Ok(selector) = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true) {
        if let Ok(infos) = DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|e| e.to_string())
            .and_then(|op| {
                wait_async_operation(op, Duration::from_secs(2), "settings paired names")
            })
        {
            for i in 0..infos.Size().unwrap_or(0) {
                let Ok(info) = infos.GetAt(i) else {
                    continue;
                };
                let id = info.Id().map(|v| v.to_string_lossy()).unwrap_or_default();
                let Some(address) = parse_bluetooth_address_from_device_id(&id) else {
                    continue;
                };
                paired_names.insert(
                    address,
                    info.Name().map(|v| v.to_string_lossy()).unwrap_or_default(),
                );
            }
        }
    }
    // Service-index discovery is local. It neither scans advertisements nor
    // opens every nearby device, and excludes keyboards/mice/sniffer serial.
    let selector =
        GattDeviceService::GetDeviceSelectorFromUuid(SERVICE_UUID).map_err(|e| e.to_string())?;
    match DeviceInformation::FindAllAsyncAqsFilter(&selector)
        .map_err(|e| e.to_string())
        .and_then(|op| wait_async_operation(op, Duration::from_secs(3), "settings devices"))
    {
        Ok(devices) => {
            for index in 0..devices.Size().map_err(|e| e.to_string())? {
                let info = devices.GetAt(index).map_err(|e| e.to_string())?;
                let id = info.Id().map_err(|e| e.to_string())?.to_string_lossy();
                let Some(address) = parse_bluetooth_address_from_device_id(&id) else {
                    continue;
                };
                // Runtime/persisted targets can refer to a powered-off board.
                // This queries local connection state only; no GATT or scan.
                if open_ble_device_with_timeout(address, Duration::from_millis(300))
                    .ok()
                    .and_then(|device| device.ConnectionStatus().ok())
                    == Some(BluetoothConnectionStatus::Connected)
                {
                    connected_addresses.insert(address);
                }
                let name = paired_names.get(&address).cloned().unwrap_or_else(|| {
                    info.Name().map(|v| v.to_string_lossy()).unwrap_or_default()
                });
                result.push(SettingsDevice {
                    id: format!("ble:{id}"),
                    transport: "ble".into(),
                    label: format!(
                        "{} · {}",
                        if name.is_empty() { "Listener" } else { &name },
                        crate::embedded_ble::format_bluetooth_address(address)
                    ),
                });
            }
        }
        Err(error) if !result.is_empty() => {
            log::warn!("[device-settings] BLE list unavailable, USB retained: {error}")
        }
        Err(error) => return Err(error),
    }
    result.sort_by(|a, b| {
        a.transport
            .cmp(&b.transport)
            .then_with(|| {
                let current = |d: &SettingsDevice| {
                    parse_bluetooth_address_from_device_id(&d.id)
                        .is_some_and(|addr| connected_addresses.contains(&addr))
                };
                current(b).cmp(&current(a))
            })
            .then_with(|| a.label.cmp(&b.label))
    });
    result.dedup_by(|a, b| a.id == b.id);
    Ok(result)
}

struct SelectedUsbSettings {
    port: Box<dyn serialport::SerialPort>,
    synchronized: bool,
}
impl SelectedUsbSettings {
    fn exchange(&mut self, command: &str) -> Result<String, String> {
        // Only queries and transaction-tokened profile staging are replay-safe.
        // Firmware accepts identical D chunks and same-token B before any D.
        // Never retry SET, COMMIT, RESET or a Bluetooth-name application.
        let replay_safe = command == "DEVICE:SETTINGS"
            || ["DEVICE:KEY R ", "DEVICE:KEY B ", "DEVICE:KEY D "]
                .iter().any(|prefix| command.starts_with(prefix));
        if let Some(line) = self.exchange_once(command)? { return Ok(line); }
        if replay_safe {
            if let Some(line) = self.exchange_once(command)? { return Ok(line); }
        }
        let kind = command.split_ascii_whitespace().take(2).collect::<Vec<_>>().join(" ");
        Err(format!("Selected USB did not acknowledge {kind}; no other device was tried."))
    }
    fn exchange_once(&mut self, command: &str) -> Result<Option<String>, String> {
        if !self.synchronized {
            drain_serial_input_until_quiet(
                &mut *self.port,
                DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION,
                DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION,
            );
        }
        // One exclusive COM handle, one outstanding command, one complete ACK.
        // Only a confirmed exchange lets the next packet reuse synchronization.
        // Any error requires draining again before a later explicit command.
        self.synchronized = false;
        self.port
            .write_all(format!("~{command}\n").as_bytes())
            .map_err(|e| e.to_string())?;
        self.port.flush().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut response = String::new();
        let mut buffer = [0u8; 1024];
        while Instant::now() < deadline {
            match self.port.read(&mut buffer) {
                Ok(n) if n > 0 => {
                    response.push_str(&String::from_utf8_lossy(&buffer[..n]));
                    if let Some(line) = response.lines().find(|line| line.contains("~DEVICE:ERROR"))
                    {
                        return Err(line.to_string());
                    }
                    if let Some(line) = complete_device_settings_ok_line(&response) {
                        self.synchronized = true;
                        return Ok(Some(line));
                    }
                    if command == "DEVICE:APPLY_BLE_NAME"
                        && response.contains("~DEVICE:APPLY_BLE_NAME result=OK")
                    {
                        self.synchronized = true;
                        return Ok(Some(String::new()));
                    }
                    if response.len() > 65536 {
                        let mut cut = response.len() - 32768;
                        while !response.is_char_boundary(cut) {
                            cut += 1;
                        }
                        response.drain(..cut);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(format!("Selected USB disconnected: {e}")),
            }
        }
        let kind = command.split_ascii_whitespace().take(2).collect::<Vec<_>>().join(" ");
        log::warn!("[device-settings] USB acknowledgement missing kind={kind} received_bytes={} complete_lines={}", response.len(), response.bytes().filter(|b| *b == b'\n').count());
        Ok(None)
    }
}
impl SettingsIo for SelectedUsbSettings {
    fn read(&mut self) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
        parse_device_settings_status_line(&self.exchange("DEVICE:SETTINGS")?)
    }
    fn write(&mut self, command: &str) -> Result<(), String> {
        if command == "DEVICE:APPLY_BLE_NAME" {
            // The shared USB control dispatcher schedules APPLY without a
            // DEVICE:SETTINGS ACK. Its completion is confirmed by readback.
            self.synchronized = false;
            self.port
                .write_all(b"~DEVICE:APPLY_BLE_NAME\n")
                .map_err(|e| e.to_string())?;
            return self.port.flush().map_err(|e| e.to_string());
        }
        self.exchange(command).map(|_| ())
    }
}

struct SelectedBleSettings {
    target: OpenEmbeddedAudioStatusTarget,
    control: Option<GattCharacteristic>,
    capture_id: Option<u64>,
}
impl SelectedBleSettings {
    fn ensure_capture_current(&self) -> Result<(), String> {
        if let Some(id) = self.capture_id {
            if !active_audio_control_sender().is_some_and(|active| active.capture_id == id && active.readiness.is_ready()) {
                return Err("Selected Listener connection changed; draft retained. Read this device again.".into());
            }
        }
        Ok(())
    }
}
impl SettingsIo for SelectedBleSettings {
    fn read(&mut self) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
        self.ensure_capture_current()?;
        let characteristic = open_read_characteristic_from_service_with_timeout(
            &self.target.service,
            DEVICE_SETTINGS_REVISION_UUID,
            "selected settings",
            BluetoothCacheMode::Uncached,
            Duration::from_secs(3),
        )?;
        let bytes = read_characteristic_bytes_with_timeout(
            &characteristic,
            BluetoothCacheMode::Uncached,
            "selected settings",
            Duration::from_secs(4),
        )?;
        parse_selected_ble_settings(&String::from_utf8(bytes).map_err(|e| e.to_string())?)
    }
    fn write(&mut self, command: &str) -> Result<(), String> {
        self.ensure_capture_current()?;
        let payload = device_settings_payload(command)?;
        if payload.len() > 63 {
            return Err("Settings command exceeds BLE control limit.".into());
        }
        let control = if let Some(control) = &self.control {
            control.clone()
        } else {
            open_write_characteristic_from_service_with_timeout(
                &self.target.service, AUDIO_CONTROL_UUID, "selected settings control",
                BluetoothCacheMode::Cached, Duration::from_secs(3),
            )?
        };
        let status = write_gatt_value_with_timeout(
            &control,
            payload.as_bytes(),
            GattWriteOption::WriteWithResponse,
            Duration::from_secs(4),
            "selected settings",
        )?;
        if status != GattCommunicationStatus::Success {
            return Err(format!("Selected settings write rejected: {status:?}"));
        }
        Ok(())
    }
}

fn parse_selected_ble_settings(
    value: &str,
) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
    let fields: std::collections::HashMap<_, _> = value
        .split(';')
        .filter_map(|part| part.split_once('='))
        .collect();
    if fields.get("schema") != Some(&"listener.device_settings.v1")
        || fields.get("snapshot") != Some(&"1")
    {
        return Err("This firmware only exposes a Bluetooth settings revision. Update Listener firmware, or select USB to read its complete settings.".into());
    }
    let keys = [
        ("settings_revision", "settings_revision"),
        ("ls", "led_status"),
        ("lk", "led_key"),
        ("l11", "led_ec11"),
        ("le", "led_edge"),
        ("pl", "plugged_low_power_idle_ms"),
        ("bl", "battery_low_power_idle_ms"),
        ("pe", "plugged_low_power_enabled"),
        ("pa", "plugged_auto_shutdown_ms"),
        ("ba", "battery_auto_shutdown_ms"),
        ("vs", "voice_auto_start"),
        ("ve", "voice_auto_stop"),
        ("kn", "knob_rotation"),
        ("bn", "ble_name"),
        ("bp", "ble_name_pending"),
        ("ep", "external_power_present"),
        ("up", "usb_power_present"),
        ("cg", "charging"),
        ("cf", "charge_full"),
    ];
    let mut line = String::from("~DEVICE:SETTINGS result=OK compact_set=1");
    for (compact, serial) in keys {
        let field = fields
            .get(compact)
            .ok_or_else(|| format!("Incomplete Bluetooth settings snapshot: {compact}"))?;
        line.push_str(&format!(" {serial}={field}"));
    }
    for key in ["ki", "kr", "ks", "kc", "ko", "kd"] {
        if let Some(field) = fields.get(key) { line.push_str(&format!(" {key}={field}")); }
    }
    parse_device_settings_status_line(&line)
}

pub fn with_settings_device<T>(
    selected: &SettingsDevice,
    task: impl FnOnce(&mut dyn SettingsIo) -> Result<T, String>,
) -> Result<T, String> {
    let deadline=Instant::now()+Duration::from_secs(60);
    // Serialize complete read/diff/write/readback sequences, rather than only
    // individual packets. Never hold this lock on the async UI thread.
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let transaction_lock=LOCK.get_or_init(||Mutex::new(()));
    let wait_started=Instant::now();
    let _lock=loop {
        match transaction_lock.try_lock() {
            Ok(guard)=>break guard,
            Err(std::sync::TryLockError::WouldBlock) if wait_started.elapsed()<Duration::from_secs(8)=>std::thread::sleep(Duration::from_millis(25)),
            Err(std::sync::TryLockError::WouldBlock)=>return Err("Another selected-device operation is still finishing. Draft retained; no second write was queued.".into()),
            Err(_)=>return Err("Settings transaction lock poisoned".into()),
        }
    };
    match selected.transport.as_str() {
        "usb" => {
            let ports = serialport::available_ports().map_err(|e| e.to_string())?;
            let port = listener_usb_serial_candidates(&ports)
                .into_iter()
                .find(|p| settings_usb_id(p) == selected.id)
                .ok_or("Selected USB identity is no longer present. Refresh the device list.")?;
            let mut handle = serialport::new(&port.port_name, DEVICE_SETTINGS_SERIAL_BAUD_RATE)
                .dtr_on_open(false)
                .timeout(Duration::from_millis(120))
                .open()
                .map_err(|e| e.to_string())?;
            let _ = handle.write_data_terminal_ready(false);
            let _ = handle.write_request_to_send(false);
            // Keep the same OS handle throughout: COM reuse cannot redirect
            // a later packet to a different board.
            super::settings_target::bounded(&mut SelectedUsbSettings { port: handle, synchronized: false },deadline,task)
        }
        "ble" => {
            let id = selected
                .id
                .strip_prefix("ble:")
                .ok_or("Invalid Bluetooth settings identity")?;
            let _gate = BleFreshGattGuard::enter("selected settings")?;
            // Service IDs are cached across firmware reconnects. Resolve the
            // selected peer afresh by its immutable address, as the proven
            // status path does, while retaining the same device and service
            // for this whole transaction. Never fall back to the voice target.
            let address = parse_bluetooth_address_from_device_id(id)
                .ok_or("Invalid selected Bluetooth device address")?;
            if let Some(active) = active_audio_control_sender().filter(|active| {
                active.bluetooth_address == Some(address) && active.readiness.is_ready()
            }) {
                if let Some(service) = active.settings_service.clone() {
                    log::info!("[device-settings] borrowing live selected peer service address={address:012X} capture_id={}", active.capture_id);
                    // Borrow the same WinRT service object, not a second open.
                    // The notify owner alone closes it; never close a clone.
                    return super::settings_target::bounded(&mut SelectedBleSettings {
                        target: OpenEmbeddedAudioStatusTarget { service, device: active.settings_device.clone() },
                        control: active.settings_control.clone(),
                        capture_id: Some(active.capture_id),
                    },deadline,task);
                }
            }
            let target = open_embedded_audio_status_target_for_device(
                address,
                Instant::now() + Duration::from_secs(5),
            )?;
            // Explicit shared access lets settings coexist with audio/HID.
            // FromIdAsync's default service opening must not become a hidden
            // exclusive owner that blocks later reads in the same app.
            use windows::Devices::Bluetooth::GenericAttributeProfile::{GattSharingMode, GattOpenStatus};
            let opened = wait_async_operation(
                target.service.OpenAsync(GattSharingMode::SharedReadAndWrite)
                    .map_err(|e| format!("Selected settings shared service open failed: {e}"))?,
                Duration::from_secs(2),
                "selected settings shared service",
            )?;
            if opened != GattOpenStatus::Success && opened != GattOpenStatus::AlreadyOpened {
                return Err(format!("Selected Listener settings service is unavailable: {opened:?}; draft retained, no settings written."));
            }
            // Require our exact service, even if IPC was given a forged ID.
            if target.service.Uuid().map_err(|e| e.to_string())? != SERVICE_UUID {
                return Err("Selected device is not a Listener settings service.".into());
            }
            super::settings_target::bounded(&mut SelectedBleSettings { target, control: None, capture_id: None },deadline,task)
        }
        _ => Err("Unknown settings transport".into()),
    }
}

#[cfg(test)]
mod selected_settings_tests {
    use super::*;
    #[test]
    #[ignore = "Requires physical Listener; read-only, run explicitly"]
    fn selected_usb_live_readback() {
        let devices = list_settings_devices().unwrap();
        for device in &devices {
            println!(
                "settings_device transport={} label={:?} id={:?}",
                device.transport, device.label, device.id
            );
        }
        let usb = devices
            .iter()
            .find(|d| d.transport == "usb")
            .expect("No Listener USB candidate");
        let status = with_settings_device(usb, |io| io.read()).unwrap();
        println!(
            "selected_usb_readback name={:?} revision={} source=firmware",
            status.ble_name, status.settings_revision
        );
        assert!(status.settings_revision > 0);
    }
    #[test]
    fn refuses_revision_only_or_incomplete_bluetooth_snapshot() {
        assert!(parse_selected_ble_settings(
            "schema=listener.device_settings.v1;settings_revision=5"
        )
        .is_err());
        assert!(parse_selected_ble_settings(
            "schema=listener.device_settings.v1;snapshot=1;settings_revision=5;bn=B"
        )
        .is_err());
    }
    #[test]
    fn complete_bluetooth_snapshot_matches_usb_fields() {
        let s = parse_selected_ble_settings("schema=listener.device_settings.v1;settings_revision=7;snapshot=1;ls=80;lk=20;l11=50;le=0;pl=180000;bl=60000;pe=1;pa=0;ba=600000;vs=1;ve=1;kn=system_volume;bn=B;bp=0;ep=1;up=1;cg=0;cf=0").unwrap();
        assert_eq!(
            (
                s.ble_name.as_str(),
                s.settings_revision,
                s.plugged_low_power_idle_minutes,
                s.battery_auto_shutdown_minutes
            ),
            ("B", 7, 3, 10)
        );
        assert!(s.voice_auto_start_enabled && s.usb_power_present);
    }
}

pub fn settings_device_peer_key(device: &SettingsDevice) -> String {
    if device.transport == "ble" {
        if let Some(address)=parse_bluetooth_address_from_device_id(&device.id) {return format!("ble:{address:012X}");}
    }
    device.id.clone()
}
