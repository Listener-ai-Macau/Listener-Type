// Listener OTA v1 transfer / prepare / probe helpers (Windows).
// Included into `windows_ble` via `include!` to keep private helper visibility.

fn transfer_denzic_ota_v1_to_target(
    target: &OpenListenerOtaV1Target,
    transfer_id: u64,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
    if manifest_chunk_bytes != LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES {
        return Err(format!(
            "Denzic OTA v1 manifest chunk size must be {LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES} bytes, got {manifest_chunk_bytes}."
        ));
    }
    let configured_window = listener_ota_v1_window_chunks()?;
    let chunk_payload_bytes = manifest_chunk_bytes
        .min(target.data_chunk_payload_bytes)
        .min(u16::MAX as usize)
        .max(1) as u16;
    let window_chunks = configured_window.min(u16::MAX as usize).max(1) as u16;
    let mut transport = ListenerOtaV1Transport {
        target,
        transfer_id,
        data_write_option: target.data_write_option,
        pending_wwr: Vec::with_capacity(LISTENER_OTA_V1_WWR_PIPELINE_DEPTH),
        pending_wwr_b: Vec::with_capacity(LISTENER_OTA_V1_WWR_PIPELINE_DEPTH),
        next_lane_b: false,
    };
    if target.data_b.is_some() {
        log::info!(
            "[embedded-ble] Denzic OTA v1 #{transfer_id}: dual-lane WWR DATA+DATA_B (device reorder)"
        );
    } else {
        log::info!(
            "[embedded-ble] Denzic OTA v1 #{transfer_id}: single-lane WWR (no DATA_B on device)"
        );
    }

    let report = denzic_ota_core::transfer(
        &mut transport,
        firmware_bytes,
        denzic_ota_core::TransferOptions {
            chunk_payload_bytes,
            window_chunks,
            status_read_attempts: 5,
            max_stalled_windows: 3,
            inactive_link_window_chunks: Some(
                LISTENER_OTA_V1_INACTIVE_LINK_WINDOW_CHUNKS as u16,
            ),
        },
        |completed, total| {
            if let Some(callback) = on_progress {
                callback(completed, total);
            }
        },
    )?;
    log::info!(
        "[embedded-ble] Denzic OTA v1 #{transfer_id}: transferred {}/{} bytes in {} data writes, {} status reads, {} offset recoveries, resumed_bytes={}, active_link_confirmed={}, elapsed_ms={}, data_write_ms={}, control_write_ms={}, status_read_ms={}, non_transfer_ms={}",
        report.firmware_bytes,
        firmware_bytes.len(),
        report.data_writes,
        report.status_reads,
        report.recovered_offsets,
        report.resumed_bytes,
        report.active_link_confirmed,
        report.timings.total.as_millis(),
        report.timings.data_write.as_millis(),
        report.timings.control_write.as_millis(),
        report.timings.status_read.as_millis(),
        report.timings.non_transfer_elapsed().as_millis()
    );
    Ok(crate::embedded_ble::FirmwareOtaTransferStats {
        bytes_transferred: report.firmware_bytes,
        chunks_sent: report.data_writes as usize,
        transport: denzic_ota_core::PROTOCOL_NAME,
        data_write_elapsed_ms: report.timings.data_write.as_millis() as u64,
        control_write_elapsed_ms: report.timings.control_write.as_millis() as u64,
        status_read_elapsed_ms: report.timings.status_read.as_millis() as u64,
    })
}

pub(super) fn request_listener_ota_v1_active_link(
    observability_correlation_id: Option<u64>,
) -> Result<(), String> {
    if let Some(correlation_id) = observability_correlation_id {
        let context = format!("TYPE:OBS:OTA:{correlation_id:016X}\n");
        if let Err(error) = send_recording_control_command(
            context.as_bytes(),
            Duration::from_millis(300),
            "Listener OTA observability context handoff",
            ActiveControlTransientFallback::ReturnError,
        ) {
            log::info!(
                "[embedded-ble] Listener OTA observability context handoff unavailable; continuing with compatible OTA handoff: {error}"
            );
        }
    }
    // Kick WinRT ThroughputOptimized on the live address before exclusive
    // capture so CI can settle during target prepare (Companion re-asserts
    // the same preference before bulk STREAM_ALL).
    request_ota_ble_throughput_for_runtime_address();
    // Prefer the live notify capture when present. Do NOT fall into the full
    // open_audio_control advertisement/retry path here: after re-pair that
    // path can burn ~12–20s on a missing audio-service index and inflate
    // preflight while OTA GATT itself is already reachable.
    if let Some(result) = send_audio_control_via_active_capture(
        b"TYPE:OTA\n",
        Duration::from_millis(800),
        "Listener OTA v1 reconnect handoff",
    ) {
        match result {
            Ok(()) => {
                log::info!("[embedded-ble] Listener OTA v1 reconnect handoff sent via active capture");
                request_ota_ble_throughput_for_runtime_address();
                return Ok(());
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] active Listener OTA v1 reconnect handoff failed; skipping long audio-control discovery: {err}"
                );
            }
        }
    } else {
        log::info!(
            "[embedded-ble] Listener OTA v1 reconnect handoff: no active capture; skipping long audio-control discovery"
        );
    }
    Err(
        "Listener OTA v1 reconnect handoff skipped: no active audio capture (prepare continues on OTA GATT)"
            .to_string(),
    )
}

pub(super) fn request_listener_ota_post_confirm_notify_fast_retry() {
    let Some(address) = runtime_bluetooth_target_address() else {
        log::warn!(
            "[embedded-ble] OTA post-confirm notify reopen has no verified current address; using normal recovery"
        );
        return;
    };
    let Ok(mut slot) = OTA_POST_CONFIRM_NOTIFY_TARGET_ADDRESS
        .get_or_init(|| Mutex::new(None))
        .lock()
    else {
        log::warn!("[embedded-ble] OTA post-confirm notify target slot is poisoned");
        return;
    };
    *slot = Some(address);
}

pub(super) fn prepare_listener_ota_v1_transfer() -> Result<PreparedListenerOtaV1Transfer, String>
{
    prepare_listener_ota_v1_transfer_impl(true, true, None)
}

pub(super) fn prepare_listener_ota_v1_transfer_after_active_link_hint(
) -> Result<PreparedListenerOtaV1Transfer, String> {
    prepare_listener_ota_v1_transfer_impl(false, true, None)
}

fn prepare_listener_ota_v1_transfer_staged_after_active_link_hint(
    target_prepare_timeout: Duration,
) -> Result<PreparedListenerOtaV1Transfer, String> {
    prepare_listener_ota_v1_transfer_impl(false, false, Some(target_prepare_timeout))
}

fn prepare_listener_ota_v1_transfer_impl(
    send_active_link_hint: bool,
    exclusive_transfer: bool,
    target_prepare_timeout: Option<Duration>,
) -> Result<PreparedListenerOtaV1Transfer, String> {
    let preparation_guard = acquire_ble_ota_preparation_mutex("listener_ota_v1_prepare")?;
    let ota_process_guard = if exclusive_transfer {
        Some(acquire_ble_ota_process_mutex("listener_ota_v1")?)
    } else {
        None
    };
    if send_active_link_hint {
        // TYPE:OTA prefers BLE audio control. After re-pair / partial Windows
        // service index, that char may be missing while OTA GATT still works
        // (preflight already opens denzic_ota_v1). Soft-fail handoff so bulk
        // transfer can still run without inflating non-transfer latency.
        match request_listener_ota_v1_active_link(None) {
            Ok(()) => {
                log::info!("[embedded-ble] Listener OTA v1 reconnect handoff accepted");
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] Listener OTA v1 reconnect handoff unavailable; continuing OTA GATT prepare: {err}"
                );
            }
        }
    } else {
        log::info!(
            "[embedded-ble] Listener OTA v1 prepare: reusing reconnect handoff sent before background pause"
        );
    }
    let transfer_guard = if exclusive_transfer {
        Some(BleCaptureGuard::enter(None)?)
    } else {
        None
    };
    let _fresh_guard = BleFreshGattGuard::enter("Listener OTA v1 prepare")?;
    let handoff_phase = if exclusive_transfer {
        "after capture handoff"
    } else {
        "before listener pause"
    };
    log::info!(
        "[embedded-ble] Listener OTA v1 prepare: opening Listener OTA v1 service {handoff_phase}"
    );
    let target = if let Some(timeout) = target_prepare_timeout {
        open_listener_ota_v1_target_after_active_link_handoff_with_deadline(
            Instant::now() + timeout.max(Duration::from_millis(1)),
        )?
    } else if send_active_link_hint {
        open_listener_ota_v1_target_after_active_link_handoff()?
    } else {
        open_listener_ota_v1_target_after_active_link_handoff_with_deadline(
            Instant::now() + Duration::from_secs(8),
        )?
    };
    let snapshot = listener_ota_v1_gatt_probe_snapshot_from_target(&target);
    log::info!(
        "[embedded-ble] Listener OTA v1 prepare: ready (detail={})",
        snapshot.detail.as_deref().unwrap_or("unknown")
    );
    Ok(PreparedListenerOtaV1Transfer {
        target,
        snapshot,
        _preparation_guard: preparation_guard,
        ownership: match (transfer_guard, ota_process_guard) {
            (Some(transfer_guard), Some(ota_process_guard)) => {
                PreparedListenerOtaV1TransferOwnership::Exclusive {
                    transfer_guard,
                    _ota_process_guard: ota_process_guard,
                }
            }
            (None, None) => PreparedListenerOtaV1TransferOwnership::Staged,
            _ => unreachable!("Listener OTA transfer ownership must stay paired"),
        },
    })
}

pub(super) fn is_ota_finish_reboot_handoff_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("0x800704c7")
        || lower.contains("0x800706ba")
        || lower.contains("transport_not_ready")
        || lower.contains("disconnected")
        || lower.contains("gattcommunicationstatus(3)")
        || lower.contains("service open async result failed")
        || (lower.contains("ota control finish") && lower.contains("timed out"))
}

pub fn transfer_listener_ota_v1(
    firmware_sha256: &str,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
    if firmware_bytes.is_empty() {
        return Err("firmware_ota.bin is empty.".to_string());
    }

    let prepared = prepare_listener_ota_v1_transfer()?;
    prepared.transfer(
        firmware_sha256,
        firmware_bytes,
        manifest_chunk_bytes,
        on_progress,
    )
}

pub fn transfer_listener_ota_v1_after_active_link_hint(
    firmware_sha256: &str,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
    if firmware_bytes.is_empty() {
        return Err("firmware_ota.bin is empty.".to_string());
    }

    let prepared = prepare_listener_ota_v1_transfer_after_active_link_hint()?;
    prepared.transfer(
        firmware_sha256,
        firmware_bytes,
        manifest_chunk_bytes,
        on_progress,
    )
}

pub fn transfer_listener_ota_v1_after_active_link_hint_staged(
    target_ready: mpsc::SyncSender<Result<(), String>>,
    start_transfer: mpsc::Receiver<()>,
    target_prepare_timeout: Duration,
    firmware_sha256: &str,
    firmware_bytes: &[u8],
    manifest_chunk_bytes: usize,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<crate::embedded_ble::FirmwareOtaTransferStats, String> {
    if firmware_bytes.is_empty() {
        return Err("firmware_ota.bin is empty.".to_string());
    }

    let prepared = match prepare_listener_ota_v1_transfer_staged_after_active_link_hint(
        target_prepare_timeout,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = target_ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    if target_ready.send(Ok(())).is_err() {
        return Err(
            "Listener OTA v1 target staging was canceled before listener pause.".to_string(),
        );
    }
    start_transfer
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| {
            "Listener OTA v1 target staging timed out before transfer start.".to_string()
        })?;
    prepared.transfer(
        firmware_sha256,
        firmware_bytes,
        manifest_chunk_bytes,
        on_progress,
    )
}

fn listener_ota_v1_window_chunks() -> Result<usize, String> {
    let configured = std::env::var(LISTENER_OTA_V1_WINDOW_ENV)
        .ok()
        .and_then(ota_env_value);
    match configured.as_deref() {
        None => Ok(LISTENER_OTA_V1_DEFAULT_WINDOW_CHUNKS),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|window| (1..=512).contains(window))
            .ok_or_else(|| {
                format!(
                    "Unsupported {LISTENER_OTA_V1_WINDOW_ENV}={value}; use a window from 1 to 512 chunks."
                )
            }),
    }
}

fn write_listener_ota_v1_value_with_fallback(
    characteristic: &GattCharacteristic,
    bytes: &[u8],
    primary: GattWriteOption,
    timeout: Duration,
    label: &str,
) -> Result<GattWriteOption, String> {
    match write_gatt_value_with_timeout(characteristic, bytes, primary, timeout, label) {
        Ok(_) => Ok(primary),
        Err(primary_err) => {
            let fallback = match primary {
                GattWriteOption::WriteWithoutResponse => GattWriteOption::WriteWithResponse,
                GattWriteOption::WriteWithResponse => GattWriteOption::WriteWithoutResponse,
                _ => return Err(primary_err),
            };
            log::warn!(
                "[embedded-ble] {label} failed with {primary:?}: {primary_err}; retrying with {fallback:?}"
            );
            write_gatt_value_with_timeout(characteristic, bytes, fallback, timeout, label)
                .map(|_| fallback)
                .map_err(|fallback_err| {
                    format!(
                        "{primary_err}; fallback {fallback:?} for {label} also failed: {fallback_err}"
                    )
                })
        }
    }
}

pub fn firmware_ota_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    listener_ota_v1_device_snapshot()
}

pub fn listener_ota_v1_device_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    let _fresh_guard = match BleFreshGattGuard::enter("Listener OTA v1 snapshot") {
        Ok(guard) => guard,
        Err(err) => {
            return crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                connected: false,
                hardware_revision: None,
                firmware_version: None,
                capabilities: Vec::new(),
                battery_percent: None,
                usb_powered: None,
                detail: Some(err),
            };
        }
    };
    match open_listener_ota_v1_target() {
        Ok(target) => listener_ota_v1_device_snapshot_from_target(&target),
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

pub fn listener_ota_v1_gatt_probe_snapshot(
    timeout: Duration,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    let _fresh_guard = match BleFreshGattGuard::enter("Listener OTA v1 GATT probe") {
        Ok(guard) => guard,
        Err(err) => {
            return crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                connected: false,
                hardware_revision: None,
                firmware_version: None,
                capabilities: Vec::new(),
                battery_percent: None,
                usb_powered: None,
                detail: Some(err),
            };
        }
    };
    let deadline = Instant::now() + timeout.max(Duration::from_millis(1));
    match open_listener_ota_v1_target_with_deadline(deadline) {
        Ok(target) => listener_ota_v1_gatt_probe_snapshot_from_target(&target),
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

pub fn listener_ota_v1_gatt_probe_after_active_link_hint(
    timeout: Duration,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    let unavailable = |detail: String| crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some(detail),
    };
    let _preparation_guard =
        match acquire_ble_ota_preparation_mutex("listener_ota_v1_preflight") {
            Ok(guard) => guard,
            Err(err) => return unavailable(err),
        };
    let _fresh_guard = match BleFreshGattGuard::enter("Listener OTA v1 handoff preflight") {
        Ok(guard) => guard,
        Err(err) => return unavailable(err),
    };
    let deadline = Instant::now() + timeout.max(Duration::from_millis(1));
    match open_listener_ota_v1_target_after_active_link_handoff_with_deadline(deadline) {
        Ok(target) => listener_ota_v1_gatt_probe_snapshot_from_target(&target),
        Err(err) => unavailable(err),
    }
}

pub fn listener_ota_v1_service_reachable_snapshot(
    timeout: Duration,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    let unavailable = |detail: String| crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some(detail),
    };
    let _fresh_guard = match BleFreshGattGuard::enter("Listener OTA v1 fast service probe") {
        Ok(guard) => guard,
        Err(err) => return unavailable(err),
    };
    let deadline = Instant::now() + timeout.max(Duration::from_millis(1));
    let address = match runtime_bluetooth_target_address() {
        Some(address) => address,
        None => {
            return unavailable(
                "Listener OTA v1 fast service probe has no cached Bluetooth address"
                    .to_string(),
            );
        }
    };
    let open_timeout = match remaining_ble_timeout(
        deadline,
        BLE_DISCOVERY_TIMEOUT,
        "Listener OTA v1 fast service device open",
    ) {
        Ok(timeout) => timeout,
        Err(err) => return unavailable(err),
    };
    let device = match open_ble_device_with_timeout(address, open_timeout) {
        Ok(device) => device,
        Err(err) => return unavailable(err),
    };
    let mut last_error = None;
    for &cache_mode in bluetooth_cache_modes_for_policy(
        denzic_ble_pairing::SERVICE_REACHABILITY_PROBE_CACHE_POLICY,
    ) {
        let discovery_timeout = match remaining_ble_timeout(
            deadline,
            BLE_DISCOVERY_TIMEOUT,
            "Listener OTA v1 fast service discovery",
        ) {
            Ok(timeout) => timeout,
            Err(err) => {
                last_error = Some(err);
                break;
            }
        };
        let operation = match device
            .GetGattServicesForUuidWithCacheModeAsync(LISTENER_OTA_V1_SERVICE_UUID, cache_mode)
        {
            Ok(operation) => operation,
            Err(err) => {
                last_error = Some(format!(
                    "Listener OTA v1 fast {cache_mode:?} service discovery failed: {err}"
                ));
                continue;
            }
        };
        let services = match wait_async_operation(
            operation,
            discovery_timeout,
            &format!("Listener OTA v1 fast {cache_mode:?} service discovery"),
        ) {
            Ok(services) => services,
            Err(err) => {
                last_error = Some(format!(
                    "Listener OTA v1 fast {cache_mode:?} service discovery wait failed: {err}"
                ));
                continue;
            }
        };
        let status = match services.Status() {
            Ok(status) => status,
            Err(err) => {
                last_error = Some(format!(
                    "Listener OTA v1 fast {cache_mode:?} service status read failed: {err}"
                ));
                continue;
            }
        };
        let count = match services.Services().and_then(|items| items.Size()) {
            Ok(count) => count,
            Err(err) => {
                last_error = Some(format!(
                    "Listener OTA v1 fast {cache_mode:?} service list read failed: {err}"
                ));
                continue;
            }
        };
        if status == GattCommunicationStatus::Success && count > 0 {
            let snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
                connected: true,
                hardware_revision: None,
                firmware_version: None,
                capabilities: vec![denzic_ota_core::PROTOCOL_NAME.to_string()],
                battery_percent: None,
                usb_powered: None,
                detail: Some(format!(
                    "Listener OTA v1 service is reachable at {}; fast confirmation skipped characteristic discovery.",
                    crate::embedded_ble::format_bluetooth_address(address)
                )),
            };
            log::info!(
                "[embedded-ble] Listener OTA v1 fast service probe connected={} cache_mode={cache_mode:?} address={address:012X}",
                snapshot.connected
            );
            return snapshot;
        }
        last_error = Some(format!(
            "Listener OTA v1 fast {cache_mode:?} service discovery returned status={status:?} count={count}"
        ));
    }
    unavailable(last_error.unwrap_or_else(|| {
        "Listener OTA v1 fast service discovery did not find the OTA service".to_string()
    }))
}

fn listener_ota_v1_device_snapshot_from_target(
    target: &OpenListenerOtaV1Target,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    let mut snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected: true,
        hardware_revision: None,
        firmware_version: None,
        capabilities: vec![denzic_ota_core::PROTOCOL_NAME.to_string()],
        battery_percent: None,
        usb_powered: None,
        detail: None,
    };
    // The OTA v1 service itself is the capability proof. DIS metadata is best-effort:
    // read it once when Windows exposes it, but do not make it a hard preflight blocker.
    if !snapshot
        .capabilities
        .iter()
        .any(|item| item == denzic_ota_core::PROTOCOL_NAME)
    {
        snapshot
            .capabilities
            .push(denzic_ota_core::PROTOCOL_NAME.to_string());
    }
    let (dis_model, dis_hardware, dis_firmware, dis_battery) =
        read_dis_metadata_from_discovered_services(target.bluetooth_address);
    snapshot.hardware_revision =
        normalize_listener_ota_hardware_revision(dis_model, dis_hardware);
    snapshot.firmware_version = dis_firmware;
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
            snapshot.hardware_revision =
                normalize_listener_ota_hardware_revision(model, hardware);
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
    if snapshot.hardware_revision.is_none() && snapshot.firmware_version.is_none() {
        let address = target.bluetooth_address.map(|value| {
            format!(
                " at {}",
                crate::embedded_ble::format_bluetooth_address(value)
            )
        });
        snapshot.detail = Some(format!(
            "Listener OTA v1 service is reachable{}, but DIS identity metadata was not exposed in this BLE session.",
            address.as_deref().unwrap_or("")
        ));
    } else {
        snapshot.detail = Some("Listener OTA v1 service is reachable; DIS metadata was read when Windows exposed it.".to_string());
    }
    log::info!(
        "[embedded-ble] Listener OTA v1 snapshot connected={} hardware={:?} firmware={:?} battery={:?} usb_powered={:?} detail={:?}",
        snapshot.connected,
        snapshot.hardware_revision,
        snapshot.firmware_version,
        snapshot.battery_percent,
        snapshot.usb_powered,
        snapshot.detail
    );
    snapshot
}

fn listener_ota_v1_gatt_probe_snapshot_from_target(
    target: &OpenListenerOtaV1Target,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    let address = target.bluetooth_address.map(|value| {
        format!(
            " at {}",
            crate::embedded_ble::format_bluetooth_address(value)
        )
    });
    let (dis_model, dis_hardware, dis_firmware, dis_battery) =
        read_dis_metadata_from_discovered_services(target.bluetooth_address);
    let snapshot = crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected: true,
        hardware_revision: normalize_listener_ota_hardware_revision(dis_model, dis_hardware),
        firmware_version: dis_firmware,
        capabilities: vec![denzic_ota_core::PROTOCOL_NAME.to_string()],
        battery_percent: dis_battery,
        usb_powered: None,
        detail: Some(format!(
            "Listener OTA v1 service is reachable{}; DIS metadata read on a best-effort basis.",
            address.as_deref().unwrap_or("")
        )),
    };
    log::info!(
        "[embedded-ble] Listener OTA v1 GATT probe connected={} capabilities={:?} detail={:?}",
        snapshot.connected,
        snapshot.capabilities,
        snapshot.detail
    );
    snapshot
}
