// Unpair / BTHPORT cleanup entrypoints used by Type recovery.
// Included into `windows_ble` via `include!`.

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
