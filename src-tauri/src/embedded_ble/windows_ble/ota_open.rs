// Listener OTA v1 target open / scan / characteristic helpers.
// Included into `windows_ble` via `include!`.

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
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "Listener OTA v1 target open",
                    );
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
                if let Some(address) = target.bluetooth_address {
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "Listener OTA v1 service target open",
                    );
                }
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
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "Listener OTA v1 deadline target open",
                    );
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
                if let Some(address) = target.bluetooth_address {
                    remember_runtime_bluetooth_target_address_for_current(
                        address,
                        "Listener OTA v1 deadline service target open",
                    );
                }
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

fn request_ota_ble_throughput_optimized(
    device: &BluetoothLEDevice,
) -> Option<OtaThroughputPrime> {
    let params = match BluetoothLEPreferredConnectionParameters::ThroughputOptimized() {
        Ok(params) => params,
        Err(err) => {
            log::warn!(
                "[embedded-ble] Listener OTA WinRT ThroughputOptimized parameters unavailable: {err}"
            );
            return None;
        }
    };
    match device.RequestPreferredConnectionParameters(&params) {
        Ok(request) => {
            let status = request.Status().ok();
            log::info!(
                "[embedded-ble] Listener OTA retained WinRT ThroughputOptimized request status={status:?} min_interval={} max_interval={} latency={} timeout={}",
                params.MinConnectionInterval().unwrap_or_default(),
                params.MaxConnectionInterval().unwrap_or_default(),
                params.ConnectionLatency().unwrap_or_default(),
                params.LinkTimeout().unwrap_or_default()
            );
            Some(OtaThroughputPrime {
                request,
                started_at: Instant::now(),
                closed: AtomicBool::new(false),
            })
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] Listener OTA WinRT ThroughputOptimized request failed: {err}"
            );
            None
        }
    }
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
    let throughput_request = request_ota_ble_throughput_optimized(&device);

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
                        data_b: prepared.data_b,
                        status: prepared.status,
                        data_write_option: prepared.data_write_option,
                        data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                        service: Some(service),
                        session: prepared.session,
                        device: Some(device),
                        throughput_request,
                        bluetooth_address: Some(address),
                    });
                }
                Err(err) => {
                    last_error = Some(format!("{cache_mode:?}: {err}"));
                    release_winrt_bluetooth_object(service);
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
    let throughput_request = request_ota_ble_throughput_optimized(&device);

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
                        data_b: prepared.data_b,
                        status: prepared.status,
                        data_write_option: prepared.data_write_option,
                        data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                        service: Some(service),
                        session: prepared.session,
                        device: Some(device),
                        throughput_request,
                        bluetooth_address: Some(address),
                    });
                }
                Err(err) => {
                    last_error = Some(format!("{cache_mode:?}: {err}"));
                    release_winrt_bluetooth_object(service);
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        "No writable Listener OTA v1 characteristics found on device".to_string()
    }))
}

fn open_diagnostic_target_for_device(address: u64) -> Result<OpenDiagnosticTarget, String> {
    open_diagnostic_target_for_device_with_policy(
        address,
        denzic_ble_pairing::DIAGNOSTIC_CACHE_POLICY,
        BLE_DISCOVERY_TIMEOUT,
    )
}

fn open_cached_diagnostic_target_for_device(
    address: u64,
    timeout: Duration,
) -> Result<OpenDiagnosticTarget, String> {
    open_diagnostic_target_for_device_with_policy(
        address,
        denzic_ble_pairing::GattCachePolicy::CachedOnly,
        timeout,
    )
}

fn open_diagnostic_target_for_device_with_policy(
    address: u64,
    cache_policy: denzic_ble_pairing::GattCachePolicy,
    timeout: Duration,
) -> Result<OpenDiagnosticTarget, String> {
    let deadline = Instant::now() + timeout;
    let device = open_ble_device_with_timeout(
        address,
        remaining_ble_timeout(deadline, timeout, "diagnostic device open")?,
    )?;
    if let Some(access) = device.RequestAccessAsync().ok().and_then(|op| {
        remaining_ble_timeout(deadline, Duration::from_secs(1), "diagnostic device access")
            .ok()
            .and_then(|remaining| {
                wait_async_operation(op, remaining, "diagnostic device access").ok()
            })
    }) {
        if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
            return Err(format!(
                "BLE diagnostic device access denied status={access:?}"
            ));
        }
    }

    let mut last_error = None;
    for &cache_mode in bluetooth_cache_modes_for_policy(cache_policy) {
        let services_result = match device
            .GetGattServicesForUuidWithCacheModeAsync(DIAGNOSTIC_SERVICE_UUID, cache_mode)
            .map_err(|err| {
                format!("BLE diagnostic {cache_mode:?} service discovery failed: {err}")
            })
            .and_then(|op| {
                wait_async_operation(
                    op,
                    remaining_ble_timeout(
                        deadline,
                        timeout,
                        "diagnostic service discovery",
                    )?,
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
            match open_diagnostic_characteristics_from_service_with_timeout(
                &service,
                cache_mode,
                remaining_ble_timeout(
                    deadline,
                    timeout,
                    "diagnostic characteristic discovery",
                )?,
            ) {
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
                    release_winrt_bluetooth_object(service);
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
    // The fresh audio-control probe already proved the unchanged
    // new-generation GATT database and ATT path. Reopen the same stable audio
    // handles from Windows' bonded cache and reuse the restored CCCD.
    open_notify_target_for_device_with_cache_modes_and_timeout(
        address,
        &POST_OTA_VERIFIED_CACHED_CACHE_MODES,
        POST_OTA_VERIFIED_CACHED_GATT_TIMEOUT,
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
                        post_ota_preserved_cccd: false,
                    });
                }
                Err(err) => {
                    last_error = Some(format!("{cache_mode:?}: {err}"));
                    release_winrt_bluetooth_object(service);
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
                    release_winrt_bluetooth_object(service);
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
    let throughput_request = device
        .as_ref()
        .and_then(request_ota_ble_throughput_optimized);

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
                    data_b: prepared.data_b,
                    status: prepared.status,
                    data_write_option: prepared.data_write_option,
                    data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                    service: Some(service),
                    session: prepared.session,
                    device,
                    throughput_request,
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
    let throughput_request = device
        .as_ref()
        .and_then(request_ota_ble_throughput_optimized);

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
                    data_b: prepared.data_b,
                    status: prepared.status,
                    data_write_option: prepared.data_write_option,
                    data_chunk_payload_bytes: prepared.data_chunk_payload_bytes,
                    service: Some(service),
                    session: prepared.session,
                    device,
                    throughput_request,
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
        post_ota_preserved_cccd: false,
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
    let readiness = open_read_characteristic_from_service(
        service,
        OTA_READINESS_UUID,
        "Listener OTA v1 readiness prime",
        cache_mode,
    )?;
    let readiness_bytes = read_characteristic_bytes_with_timeout(
        &readiness,
        BluetoothCacheMode::Uncached,
        "Listener OTA v1 readiness prime",
        BLE_DISCOVERY_TIMEOUT,
    )?;
    log::info!(
        "[embedded-ble] Listener OTA v1 readiness prime completed bytes={}",
        readiness_bytes.len()
    );
    release_winrt_bluetooth_object(readiness);
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
    let data_b = open_write_characteristic_from_service(
        service,
        LISTENER_OTA_V1_DATA_B_UUID,
        "Listener OTA v1 data_b",
        cache_mode,
    )
    .ok();
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
        "[embedded-ble] Listener OTA v1 data write option={data_write_option:?} chunk_payload_bytes={payload_bytes} dual_lane={}",
        data_b.is_some()
    );
    Ok(PreparedListenerOtaV1Characteristics {
        control,
        data,
        data_b,
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
    let readiness = open_read_characteristic_from_service_with_timeout(
        service,
        OTA_READINESS_UUID,
        "Listener OTA v1 readiness prime",
        cache_mode,
        remaining_ble_timeout(
            deadline,
            BLE_DISCOVERY_TIMEOUT,
            "Listener OTA v1 readiness prime characteristic",
        )?,
    )?;
    let readiness_bytes = read_characteristic_bytes_with_timeout(
        &readiness,
        BluetoothCacheMode::Uncached,
        "Listener OTA v1 readiness prime",
        remaining_ble_timeout(
            deadline,
            BLE_DISCOVERY_TIMEOUT,
            "Listener OTA v1 readiness prime read",
        )?,
    )?;
    log::info!(
        "[embedded-ble] Listener OTA v1 readiness prime completed bytes={}",
        readiness_bytes.len()
    );
    release_winrt_bluetooth_object(readiness);
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
    let data_b = remaining_ble_timeout(
        deadline,
        BLE_DISCOVERY_TIMEOUT,
        "Listener OTA v1 data_b characteristic",
    )
    .ok()
    .and_then(|timeout| {
        open_write_characteristic_from_service_with_timeout(
            service,
            LISTENER_OTA_V1_DATA_B_UUID,
            "Listener OTA v1 data_b",
            cache_mode,
            timeout,
        )
        .ok()
    });
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
        "[embedded-ble] Listener OTA v1 data write option={data_write_option:?} chunk_payload_bytes={payload_bytes} dual_lane={}",
        data_b.is_some()
    );
    Ok(PreparedListenerOtaV1Characteristics {
        control,
        data,
        data_b,
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
    open_diagnostic_characteristics_from_service_with_timeout(
        service,
        cache_mode,
        BLE_DISCOVERY_TIMEOUT,
    )
}

fn open_diagnostic_characteristics_from_service_with_timeout(
    service: &GattDeviceService,
    cache_mode: BluetoothCacheMode,
    timeout: Duration,
) -> Result<PreparedDiagnosticCharacteristics, String> {
    let deadline = Instant::now() + timeout;
    if let Some(access) = service.RequestAccessAsync().ok().and_then(|op| {
        remaining_ble_timeout(deadline, Duration::from_secs(1), "diagnostic service access")
            .ok()
            .and_then(|remaining| {
                wait_async_operation(op, remaining, "diagnostic service access").ok()
            })
    }) {
        if access != DeviceAccessStatus::Allowed && access != DeviceAccessStatus::Unspecified {
            return Err(format!(
                "BLE diagnostic service access denied status={access:?}"
            ));
        }
    }
    let session = prepare_gatt_session(
        service,
        remaining_ble_timeout(deadline, GATT_READY_TIMEOUT, "diagnostic GATT session")?,
    )?;
    let control = open_write_characteristic_from_service_with_timeout(
        service,
        DIAGNOSTIC_CONTROL_UUID,
        "diagnostic control",
        cache_mode,
        remaining_ble_timeout(
            deadline,
            timeout,
            "diagnostic control characteristic",
        )?,
    )?;
    let data = open_notify_characteristic_by_uuid_from_service_with_timeout(
        service,
        DIAGNOSTIC_DATA_UUID,
        "diagnostic data",
        cache_mode,
        remaining_ble_timeout(deadline, timeout, "diagnostic data characteristic")?,
    )?;
    let count = open_read_characteristic_from_service_with_timeout(
        service,
        DIAGNOSTIC_COUNT_UUID,
        "diagnostic count",
        cache_mode,
        remaining_ble_timeout(deadline, timeout, "diagnostic count characteristic")?,
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
        // Wait for Windows ATT MTU / MaxPduSize to climb after encryption + DLE.
        // Forcing 500-byte WWR while MaxPdu is still ~23 causes protocol_error=3
        // (Write Not Permitted) and never reaches the device OTA handler.
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline && payload_bytes < desired_payload {
            std::thread::sleep(Duration::from_millis(150));
            if let Some(next_payload) = session
                .and_then(|session| session.MaxPduSize().ok())
                .map(|max_pdu_size| {
                    usize::from(max_pdu_size).saturating_sub(ATT_WRITE_HEADER_BYTES)
                })
                .filter(|payload_bytes| *payload_bytes > denzic_ota_core::DATA_HEADER_BYTES)
            {
                if next_payload > payload_bytes {
                    log::info!(
                        "[embedded-ble] Listener OTA v1 MaxPdu payload grew {payload_bytes} -> {next_payload}"
                    );
                }
                payload_bytes = payload_bytes.max(next_payload);
            }
        }
        if payload_bytes < desired_payload {
            log::warn!(
                "[embedded-ble] Listener OTA v1 MaxPduSize stayed at payload_bytes={payload_bytes} (need {desired_payload}); clamping WWR chunk to link MTU instead of forcing 500"
            );
            // Use what the link actually allows (minus data header). Bulk will be
            // slower until MTU rises, but BEGIN/data writes stay legal.
            return payload_bytes
                .saturating_sub(denzic_ota_core::DATA_HEADER_BYTES)
                .max(1)
                .min(LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES);
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
