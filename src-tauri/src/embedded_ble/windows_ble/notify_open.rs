// Notify / audio-control target open helpers (Windows).
// Included into `windows_ble` via `include!`.

/// Prefer verified Listener identities among multi-root Windows HID matches.
/// Ghost nodes from EC11/OTA identity rotation stay in the set but are tried last,
/// so post-reboot reconnect does not spend seconds timing out dead addresses first.
fn prioritize_native_hid_notify_addresses(addresses: Vec<u64>) -> Vec<u64> {
    if addresses.len() <= 1 {
        return addresses;
    }
    let mut ordered = Vec::with_capacity(addresses.len());
    let mut push_if_present = |address: Option<u64>| {
        let Some(address) = address else {
            return;
        };
        if addresses.contains(&address) && !ordered.contains(&address) {
            ordered.push(address);
        }
    };
    push_if_present(runtime_bluetooth_target_address());
    push_if_present(persisted_successful_notify_target_address_for_current());
    push_if_present(peek_listener_ota_post_confirm_notify_target_address());
    for address in addresses {
        if !ordered.contains(&address) {
            ordered.push(address);
        }
    }
    ordered
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
                    post_ota_preserved_cccd: false,
                });
            }
            Err(err) => {
                last_error = Some(format!("persisted startup Cached index={index}: {err}"));
                release_winrt_bluetooth_object(service);
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
            false,
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
            // Multiple same-name HID roots (EC11 identity rotation ghosts) must not
            // burn 600 ms each before the last-successful / runtime address. Prefer
            // verified identities that still appear in the current HID set first.
            prioritize_native_hid_notify_addresses(
                native_windows_hid_pairing_addresses_for_startup(),
            )
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
            Ok(mut target) => {
                target.post_ota_preserved_cccd = ota_post_confirm_address.is_some();
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

fn peek_listener_ota_post_confirm_notify_target_address() -> Option<u64> {
    OTA_POST_CONFIRM_NOTIFY_TARGET_ADDRESS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|slot| *slot)
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
