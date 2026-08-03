// Windows BLE pairing / unpair / AEP discovery helpers.
// Included into `windows_ble` via `include!` to keep private helper visibility.

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

pub fn recent_listener_pairing_fast_gatt_address() -> Option<u64> {
    recent_pairing_fast_gatt_active(Instant::now()).and_then(|state| state.address)
}

pub fn prompt_listener_pairing(
    expected_name: Option<&str>,
) -> crate::embedded_ble::BleDevicePairingPromptResult {
    match prompt_listener_pairing_inner(expected_name, false, false, true, false, &[], false) {
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
    match prompt_listener_pairing_inner(expected_name, true, false, true, false, &[], false) {
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
    match prompt_listener_pairing_inner(expected_name, true, false, false, false, &[], false) {
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
    match prompt_listener_pairing_inner(expected_name, true, true, true, true, &[], false) {
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
    match prompt_listener_pairing_inner(expected_name, true, true, false, true, &[], false) {
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
        false,
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

pub fn recover_listener_pairing_after_type_recovery_for_addresses(
    expected_name: Option<&str>,
    cleanup_addresses: &[u64],
    pairing_addresses: &[u64],
) -> crate::embedded_ble::BleDevicePairingPromptResult {
    recover_listener_pairing_after_type_recovery_with_cleanup_for_addresses(
        expected_name,
        cleanup_addresses,
        pairing_addresses,
    )
    .1
}

pub fn recover_listener_pairing_after_type_recovery_with_cleanup_for_addresses(
    expected_name: Option<&str>,
    cleanup_addresses: &[u64],
    pairing_addresses: &[u64],
) -> (
    crate::embedded_ble::BleDeviceUnpairResult,
    crate::embedded_ble::BleDevicePairingPromptResult,
) {
    match recover_listener_pairing_after_type_recovery_for_addresses_inner(
        expected_name,
        cleanup_addresses,
        pairing_addresses,
    ) {
        Ok(result) => result,
        Err(err) => {
            log::warn!("[embedded-ble] atomic Listener pairing recovery unavailable: {err}");
            (
                crate::embedded_ble::BleDeviceUnpairResult {
                    status: crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction,
                    attempted: false,
                    matched_devices: 0,
                    unpaired_devices: 0,
                    already_unpaired_devices: 0,
                    failed_devices: 0,
                    needs_user_action: false,
                    details: vec![err.clone()],
                },
                crate::embedded_ble::BleDevicePairingPromptResult {
                    status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
                    attempted: false,
                    matched_devices: 0,
                    prompted_devices: 0,
                    already_paired_devices: 0,
                    failed_devices: 0,
                    open_bluetooth_settings: false,
                    details: vec![err],
                },
            )
        }
    }
}

fn recover_listener_pairing_after_type_recovery_for_addresses_inner(
    expected_name: Option<&str>,
    cleanup_addresses: &[u64],
    pairing_addresses: &[u64],
) -> Result<
    (
        crate::embedded_ble::BleDeviceUnpairResult,
        crate::embedded_ble::BleDevicePairingPromptResult,
    ),
    String,
> {
    const OWNERSHIP_WAIT: Duration = Duration::from_secs(5);

    let target_name = effective_bluetooth_target_name(expected_name);
    let Some(_maintenance) = begin_listener_pairing_maintenance_after_wait(
        "type-recovery",
        &target_name,
        OWNERSHIP_WAIT,
    ) else {
        return Ok((
            atomic_type_recovery_no_cleanup_result(
                "Pairing maintenance was busy before exact stale-address cleanup.",
            ),
            pairing_maintenance_busy_prompt_result(&target_name),
        ));
    };

    let cleanup_names = vec![target_name.clone()];
    let direct_pairing_uses_fresh_recovery_address = !pairing_addresses.is_empty();
    let mut unpair = if cleanup_addresses.is_empty() {
        log::warn!(
            "[embedded-ble] atomic Type recovery has no exact stale address to clean before PairAsync"
        );
        atomic_type_recovery_no_cleanup_result(
            "No exact pre-recovery address was available; exhaustive ghost cleanup remains deferred until TYPE:READY.",
        )
    } else {
        let unpair = unpair_listener_devices_for_known_addresses_inner(
            &cleanup_names,
            cleanup_addresses,
            true,
        )?;
        log::warn!(
            "[embedded-ble] atomic Type recovery known-address cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={} fresh_direct_pairing={} cleanup_addresses={cleanup_addresses:?} pairing_addresses={pairing_addresses:?}",
            unpair.status,
            unpair.matched_devices,
            unpair.unpaired_devices,
            unpair.already_unpaired_devices,
            unpair.failed_devices,
            unpair.needs_user_action,
            direct_pairing_uses_fresh_recovery_address,
        );
        unpair
    };

    let pairing = prompt_listener_pairing_inner(
        Some(target_name.as_str()),
        true,
        true,
        false,
        false,
        pairing_addresses,
        true,
    )?;
    if !direct_pairing_uses_fresh_recovery_address
        || pairing_prompt_result_ready_for_atomic_recovery(&pairing)
    {
        return Ok((unpair, pairing));
    }

    if cleanup_addresses.is_empty() {
        log::warn!(
            "[embedded-ble] atomic Type recovery fresh-address PairAsync did not complete; preserving the previous Windows pairing and deferring all previous-identity cleanup until TYPE:READY status={:?} matched={} prompted={} failed={}",
            pairing.status,
            pairing.matched_devices,
            pairing.prompted_devices,
            pairing.failed_devices,
        );
        return Ok((unpair, pairing));
    }

    log::warn!(
        "[embedded-ble] atomic Type recovery fresh-address PairAsync did not complete; clearing only the exact BTHPORT cache before the final PairAsync status={:?} matched={} prompted={} failed={}",
        pairing.status,
        pairing.matched_devices,
        pairing.prompted_devices,
        pairing.failed_devices,
    );
    let fallback_unpair =
        clear_listener_bthport_cache_for_known_addresses_inner(&cleanup_names, cleanup_addresses)?;
    log::warn!(
        "[embedded-ble] atomic Type recovery exact BTHPORT cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={}",
        fallback_unpair.status,
        fallback_unpair.matched_devices,
        fallback_unpair.unpaired_devices,
        fallback_unpair.already_unpaired_devices,
        fallback_unpair.failed_devices,
        fallback_unpair.needs_user_action,
    );
    merge_atomic_type_recovery_cleanup_result(&mut unpair, fallback_unpair);
    let pairing = prompt_listener_pairing_inner(
        Some(target_name.as_str()),
        true,
        true,
        false,
        false,
        pairing_addresses,
        true,
    )?;
    Ok((unpair, pairing))
}

fn atomic_type_recovery_no_cleanup_result(
    detail: &str,
) -> crate::embedded_ble::BleDeviceUnpairResult {
    crate::embedded_ble::BleDeviceUnpairResult {
        status: crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean,
        attempted: false,
        matched_devices: 0,
        unpaired_devices: 0,
        already_unpaired_devices: 0,
        failed_devices: 0,
        needs_user_action: false,
        details: vec![detail.to_string()],
    }
}

fn merge_atomic_type_recovery_cleanup_result(
    base: &mut crate::embedded_ble::BleDeviceUnpairResult,
    fallback: crate::embedded_ble::BleDeviceUnpairResult,
) {
    base.attempted |= fallback.attempted;
    base.matched_devices = base.matched_devices.saturating_add(fallback.matched_devices);
    base.unpaired_devices = base.unpaired_devices.saturating_add(fallback.unpaired_devices);
    base.already_unpaired_devices = base
        .already_unpaired_devices
        .saturating_add(fallback.already_unpaired_devices);
    base.failed_devices = base.failed_devices.saturating_add(fallback.failed_devices);
    base.details.extend(fallback.details);
    base.status = if base.failed_devices > 0 {
        crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction
    } else if base.unpaired_devices > 0 {
        crate::embedded_ble::BleDeviceUnpairStatus::Removed
    } else {
        crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean
    };
    base.needs_user_action = base.failed_devices > 0;
}

fn pairing_prompt_result_ready_for_atomic_recovery(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::Paired
            | crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
    )
}

fn begin_listener_pairing_maintenance_after_wait(
    owner: &'static str,
    target_name: &str,
    timeout: Duration,
) -> Option<PairingMaintenanceGuard> {
    let started_at = Instant::now();
    loop {
        if !listener_pairing_maintenance_active() {
            if let Some(guard) =
                try_begin_listener_pairing_maintenance(owner, target_name, Instant::now())
            {
                let waited = started_at.elapsed();
                if !waited.is_zero() {
                    log::info!(
                        "[embedded-ble] Listener pairing maintenance ownership acquired after wait owner={owner} target={target_name:?} waited_ms={}",
                        waited.as_millis(),
                    );
                }
                return Some(guard);
            }
        }
        if started_at.elapsed() >= timeout {
            log::warn!(
                "[embedded-ble] Listener pairing maintenance ownership wait timed out owner={owner} target={target_name:?} timeout_ms={}",
                timeout.as_millis(),
            );
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
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

fn store_native_windows_hid_pairing_addresses(addresses: &[u64]) {
    NATIVE_WINDOWS_HID_PAIRING_VISIBLE.store(!addresses.is_empty(), Ordering::SeqCst);
    if let Ok(mut snapshot) = NATIVE_WINDOWS_HID_PAIRING_ADDRESSES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        *snapshot = addresses.to_vec();
    }
}

pub fn native_windows_hid_pairing_addresses() -> Result<Vec<u64>, String> {
    let entries = powershell_listener_pnp_entries()?;
    let addresses = native_windows_hid_pairing_addresses_from_entries(&entries);
    store_native_windows_hid_pairing_addresses(&addresses);
    Ok(addresses)
}

pub fn native_windows_hid_present_pairing_addresses() -> Result<Vec<u64>, String> {
    let entries = powershell_listener_present_pnp_entries()?;
    let addresses = native_windows_hid_pairing_addresses_from_entries(&entries);
    store_native_windows_hid_pairing_addresses(&addresses);
    Ok(addresses)
}

pub fn warm_native_windows_hid_present_pairing_snapshot() {
    if NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(err) = std::thread::Builder::new()
        .name("listener-native-hid-pnp".to_string())
        .spawn(|| {
            let result = native_windows_hid_present_pairing_addresses();
            if let Ok(mut slot) = NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RESULT
                .get_or_init(|| Mutex::new(None))
                .lock()
            {
                *slot = Some(result);
            }
            NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RUNNING.store(false, Ordering::SeqCst);
        })
    {
        NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RUNNING.store(false, Ordering::SeqCst);
        log::warn!("[embedded-ble] could not start native HID PnP prefetch: {err}");
    }
}

pub fn native_windows_hid_present_pairing_addresses_for_startup() -> Result<Vec<u64>, String> {
    // The PnP command is read-only and starts while Type initializes its UI/runtime.
    // Wait briefly for that single prefetch so startup does not pay the pwsh launch cost
    // serially, then retain the ordinary fresh query as the fallback.
    for _ in 0..20 {
        if let Ok(slot) = NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RESULT
            .get_or_init(|| Mutex::new(None))
            .lock()
        {
            if let Some(result) = slot.as_ref() {
                crate::startup_evidence::record_startup_stage("native_hid_pnp_prefetch_ready");
                return result.clone();
            }
        }
        if !NATIVE_WINDOWS_HID_PRESENT_PREFETCH_RUNNING.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    native_windows_hid_present_pairing_addresses()
}

pub fn native_windows_hid_pairing_active_connection(
    addresses: &[u64],
) -> Result<Option<u64>, String> {
    let mut probe_errors = Vec::new();
    for address in addresses {
        let address_text = crate::embedded_ble::format_bluetooth_address(*address);
        match open_ble_device_by_address_with_timeout(*address, Duration::from_millis(750)) {
            Ok(device) => {
                let status = device.ConnectionStatus();
                release_winrt_bluetooth_object(device);
                match status {
                    Ok(BluetoothConnectionStatus::Connected) => return Ok(Some(*address)),
                    Ok(BluetoothConnectionStatus::Disconnected) => {}
                    Ok(other) => probe_errors.push(format!(
                        "{address_text}: unexpected connection status {other:?}"
                    )),
                    Err(err) => probe_errors.push(format!(
                        "{address_text}: connection status read failed: {err}"
                    )),
                }
            }
            Err(err) => {
                probe_errors.push(format!("{address_text}: BLE device open failed: {err}"))
            }
        }
    }
    if !addresses.is_empty() && probe_errors.len() == addresses.len() {
        return Err(format!(
            "native Windows HID active-connection probe failed for every address: {}",
            probe_errors.join("; ")
        ));
    }
    Ok(None)
}

fn native_windows_hid_pairing_visible_for_startup() -> bool {
    NATIVE_WINDOWS_HID_PAIRING_VISIBLE.load(Ordering::SeqCst)
}

fn native_windows_hid_pairing_addresses_for_startup() -> Vec<u64> {
    NATIVE_WINDOWS_HID_PAIRING_ADDRESSES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default()
}

fn native_windows_hid_snapshot_refresh_is_useful(
    startup_addresses: &[u64],
    refreshed_addresses: &[u64],
) -> bool {
    !refreshed_addresses.is_empty() && refreshed_addresses != startup_addresses
}

fn native_windows_hid_current_address_uses_random_identity(
    address: u64,
    startup_addresses: &[u64],
) -> bool {
    startup_addresses.contains(&address)
}

pub(super) fn native_windows_hid_pairing_addresses_from_entries(
    entries: &[ListenerPnpEntry],
) -> Vec<u64> {
    let mut roots = Vec::new();
    let mut keyboards = Vec::new();
    for entry in entries {
        let Some(address) = entry.address else {
            continue;
        };
        if entry.is_ble_device_root && !roots.contains(&address) {
            roots.push(address);
        }
        if entry.is_listener_hid_keyboard && !keyboards.contains(&address) {
            keyboards.push(address);
        }
    }
    roots
        .into_iter()
        .filter(|address| keyboards.contains(address))
        .collect()
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
    maintenance_already_held: bool,
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
    let _maintenance = if maintenance_already_held {
        None
    } else {
        let Some(maintenance) =
            try_begin_listener_pairing_maintenance("pair", &target_name, now)
        else {
            return Ok(pairing_maintenance_busy_prompt_result(&target_name));
        };
        Some(maintenance)
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

    // Restarting the host adapter tears down any concurrent native Windows /
    // Swift Pair ceremony and can turn the next attempt into a stale-key failure.
    // Adapter recovery is operator-owned; Type only owns its bounded PairAsync.
    let allow_adapter_restart = false;
    let fast_recovery_pairing_failure = pair_listener_candidates_into_prompt_result(
        &mut result,
        candidates,
        &target_name,
        bypass_prompt_suppression,
        type_recovery_command_confirmed || !allow_user_pairing_prompt,
        allow_adapter_restart,
        type_recovery_command_confirmed,
    );
    let exact_recovery_address = !observed_recovery_addresses.is_empty();
    if bypass_prompt_suppression
        && result.prompted_devices == 0
        && result.already_paired_devices == 0
        && fast_recovery_pairing_failure
    {
        if exact_recovery_address {
            result.details.push(
                "Fresh Listener recovery address direct PairAsync failed; skipped slow AEP discovery so the bounded recovery transaction can classify and retry precisely."
                    .to_string(),
            );
            log::warn!(
                "[embedded-ble] fresh recovery-address direct PairAsync failed; skipping slow AEP discovery before the exact cache retry"
            );
        } else {
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
    let remaining_ms = denzic_ble_pairing::window_remaining_ms(
        1,
        BLE_PAIRING_PROMPT_SUPPRESS_WINDOW.as_millis() as i64,
        1 + elapsed.as_millis() as i64,
    );
    if remaining_ms <= 0 {
        return None;
    }
    Some(Duration::from_millis(remaining_ms as u64))
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
    let fast_known_addresses = if observed_recovery_addresses.is_empty() {
        listener_recovery_fast_target_addresses()
    } else {
        Vec::new()
    };
    let mut fresh_advertised_addresses = Vec::new();

    if !fast_known_addresses.is_empty() {
        let mut known_candidates = listener_recovery_direct_pairing_candidates(
            &fast_known_addresses,
            &[],
            expected_name,
            &mut seen_ids,
        );
        known_candidates.retain(|candidate| {
            candidate
                .info
                .Pairing()
                .and_then(|pairing| pairing.IsPaired())
                .unwrap_or(false)
        });
        if !known_candidates.is_empty() {
            log::info!(
                "[embedded-ble] recovery pairing found {} already-paired known-address candidate(s); skipping the advertisement scan",
                known_candidates.len()
            );
            return Ok(known_candidates);
        }
        seen_ids.clear();
    }

    let mut addresses = if observed_recovery_addresses.is_empty() {
        listener_recovery_target_addresses()
    } else {
        Vec::new()
    };

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
            "[embedded-ble] recovery pairing using Type-observed recovery address(es) before any duplicate advertisement scan: {observed_labels:?}"
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
            "[embedded-ble] recovery AEP fallback using Type-observed recovery address(es) without duplicate advertisement scan: {observed_labels:?}"
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
        let address_type = fresh_advertised_addresses
            .contains(&address)
            .then(|| recent_recovery_pairing_address_type(address))
            .flatten();
        match pairing_device_information_from_bluetooth_address_handle(
            address,
            address_type,
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
                    "[embedded-ble] recovery direct pairing candidate name={name:?} address={address_text} address_type={address_type:?} path=typed_handle"
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

    match pairing_device_information_from_bluetooth_address_handle(address, address_type, label) {
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
    address_type: Option<BluetoothAddressType>,
    label: &str,
) -> Result<Option<DeviceInformation>, String> {
    let address_text = crate::embedded_ble::format_bluetooth_address(address);
    let address_type = address_type
        .or_else(|| recent_recovery_pairing_address_type(address))
        .filter(|kind| *kind != BluetoothAddressType::Unspecified);
    let operation = match address_type {
        Some(kind) => BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(
            address, kind,
        ),
        None => BluetoothLEDevice::FromBluetoothAddressAsync(address),
    };
    let device = operation
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
                "[embedded-ble] opened transient BLE DeviceInformation for pairing address={address_text} address_type={address_type:?}"
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
    release_winrt_bluetooth_object(device);
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
                let fast_trusted_addresses = listener_recovery_fast_target_addresses();
                if !listener_pairing_candidate_has_trusted_address(
                    &candidate,
                    &fast_trusted_addresses,
                ) {
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

    if retry_failed_pairasync_once {
        log::info!(
            "[embedded-ble] fresh Type recovery registering custom ConfirmOnly handler before the first Windows pairing ceremony for {}",
            candidate.label
        );
        match custom_pair_listener_candidate(pairing, &candidate.label, expected_name) {
            Ok(outcome) => return Ok(outcome),
            Err(err) => log::warn!(
                "[embedded-ble] fresh Type recovery first custom PairAsync failed for {}: {err}; retaining bounded generic fallback",
                candidate.label
            ),
        }
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
    if !retry_failed_pairasync_once && pairing_status_should_try_custom_fallback(status) {
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
    crate::startup_evidence::record_pair_async_attempt();
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
    crate::startup_evidence::record_pair_async_attempt();
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
            recent_recovery_pairing_address_type(address),
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

    crate::startup_evidence::record_unpair_async_attempt();
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
    let mut addresses = listener_recovery_fast_target_addresses();
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

fn listener_recovery_fast_target_addresses() -> Vec<u64> {
    let mut addresses = Vec::new();
    if let Some(address) = configured_bluetooth_address() {
        push_unique_address(&mut addresses, address);
    }
    if let Some(address) = persisted_successful_notify_target_address_for_current() {
        push_unique_address(&mut addresses, address);
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
                release_winrt_bluetooth_object(device);
                log::warn!(
                    "[embedded-ble] BLE address cleanup DeviceInformation for {address_text} failed: {err}"
                );
                continue;
            }
        };
        release_winrt_bluetooth_object(device);
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
