// PnP / BTHPORT pairing-cache cleanup helpers (Windows).
// Included into `windows_ble` via `include!`.

fn listener_pnp_remove_candidates(
    target_addresses: &[u64],
    target_names: &[String],
    exact_address_only: bool,
) -> Result<Vec<ListenerPnpRemoveCandidate>, String> {
    if exact_address_only {
        let mut candidates = Vec::new();
        for address in target_addresses.iter().copied() {
            if candidates
                .iter()
                .any(|candidate: &ListenerPnpRemoveCandidate| candidate.address == Some(address))
            {
                continue;
            }
            let address_label = crate::embedded_ble::format_bluetooth_address(address);
            let instance_id = format!(r"BTHLE\Dev_{address:012x}");
            candidates.push(ListenerPnpRemoveCandidate {
                label: format!("{address_label} [{instance_id}]"),
                instance_id,
                name: String::new(),
                address: Some(address),
                is_ble_device_root: true,
            });
        }
        log::info!(
            "[embedded-ble] exact-address PnP cleanup using {} direct BTHLE root(s) without global device enumeration",
            candidates.len()
        );
        return Ok(candidates);
    }

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
