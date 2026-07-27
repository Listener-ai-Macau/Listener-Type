// GATT characteristic open / session-ready helpers.
// Included into `windows_ble` via `include!`.

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
