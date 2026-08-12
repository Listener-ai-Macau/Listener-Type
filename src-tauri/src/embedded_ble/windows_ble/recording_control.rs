// Recording / processing / recovery control commands (Windows).
// Included into `windows_ble` via `include!`.

pub fn probe_notify_subscription(timeout: Duration) -> Result<(), String> {
    let capture_guard = BleCaptureGuard::enter(Some(timeout))?;
    let capture_id = capture_guard.session_id();
    let target = open_notify_target_with_retry(capture_id)?;
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
    let recovery_probe_address = cleanup.target.bluetooth_address;
    let status = write_cccd_notify_with_retry(
        capture_id,
        "probe",
        &characteristic,
        notify_timeout,
        recovery_probe_address,
    )?;
    if status != GattCommunicationStatus::Success {
        return Err(format!("BLE CCCD notify write returned status={status:?}"));
    }
    log::info!("[embedded-ble] probe #{capture_id}: notify CCCD enabled");
    cleanup.finish(NotifyCccdTeardown::for_probe_success());
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveControlTransientFallback {
    TryFreshGatt,
    ReturnError,
}

fn processing_state_active_transient_fallback(active: bool) -> ActiveControlTransientFallback {
    let _ = active;
    ActiveControlTransientFallback::ReturnError
}

fn bounded_recording_stop_active_timeout(timeout: Duration) -> Duration {
    if timeout > RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT {
        RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT
    } else {
        timeout
    }
}

fn send_recording_control_command(
    command: &[u8],
    timeout: Duration,
    label: &'static str,
    active_transient_fallback: ActiveControlTransientFallback,
) -> Result<(), String> {
    if let Some(result) = send_audio_control_via_active_capture(command, timeout, label) {
        match result {
            Ok(_) => {
                log::info!("[embedded-ble] {label} sent via active capture");
                return Ok(());
            }
            Err(err) if is_transient_audio_control_write_error(&err) => {
                if active_transient_fallback == ActiveControlTransientFallback::ReturnError {
                    log::warn!(
                        "[embedded-ble] active {label} write failed with transient error; returning for caller recovery: {err}"
                    );
                    return Err(err);
                }
                log::warn!(
                    "[embedded-ble] active {label} write failed with transient error; retrying fresh GATT path: {err}"
                );
            }
            Err(err) => return Err(err),
        }
    }
    let _fresh_guard = BleFreshGattGuard::enter(label)?;
    let target = open_audio_control_target_with_retry(label)?;
    write_audio_control_value_with_timeout(&target.control, command, timeout, label)?;
    log::info!("[embedded-ble] {label} sent");
    Ok(())
}

fn send_processing_hint_control_command(
    command: &[u8],
    serial_command: &'static str,
    timeout: Duration,
    label: &'static str,
) -> Result<(), String> {
    let mut active_error = None;
    if let Some(result) = send_audio_control_via_active_capture(command, timeout, label) {
        match result {
            Ok(()) => {
                log::info!("[embedded-ble] {label} sent via active capture");
                return Ok(());
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] active {label} write failed; trying USB serial processing hint fallback: {err}"
                );
                active_error = Some(err);
            }
        }
    }

    match send_control_command_via_usb_serial(serial_command, timeout) {
        Ok(()) => {
            log::info!("[embedded-ble] {label} sent via USB serial fallback");
            Ok(())
        }
        Err(err) => {
            let active_part = active_error
                .map(|err| format!("active BLE failed: {err}; "))
                .unwrap_or_default();
            Err(format!("{active_part}USB serial fallback failed: {err}"))
        }
    }
}

fn send_recording_stop_control_command(timeout: Duration) -> Result<(), String> {
    let command = b"VREC:STOP\n";
    let label = "audio control stop";
    let active_timeout = bounded_recording_stop_active_timeout(timeout);
    let mut active_error = None;
    if let Some(result) = send_audio_control_via_active_capture(command, active_timeout, label)
    {
        match result {
            Ok(()) => {
                log::info!(
                    "[embedded-ble] {label} sent via active capture active_timeout_ms={}",
                    active_timeout.as_millis()
                );
                return Ok(());
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] active {label} write failed; trying USB serial stop fallback: {err}"
                );
                active_error = Some(err);
            }
        }
    }

    match send_control_command_via_usb_serial("VREC:STOP", timeout) {
        Ok(()) => {
            log::info!("[embedded-ble] {label} sent via USB serial fallback");
            Ok(())
        }
        Err(err) => {
            let active_part = active_error
                .map(|err| format!("active BLE failed: {err}; "))
                .unwrap_or_default();
            Err(format!("{active_part}USB serial fallback failed: {err}"))
        }
    }
}

pub fn send_recording_control_toggle(timeout: Duration) -> Result<(), String> {
    send_recording_control_command(
        b"VREC:TOGGLE\n",
        timeout,
        "audio control toggle",
        ActiveControlTransientFallback::TryFreshGatt,
    )
}

pub fn send_recording_control_cancel(timeout: Duration) -> Result<(), String> {
    send_recording_control_command(
        b"VREC:CANCEL\n",
        timeout,
        "audio control cancel",
        ActiveControlTransientFallback::TryFreshGatt,
    )
}

pub fn send_recording_control_activate(timeout: Duration) -> Result<(), String> {
    send_recording_control_command(
        b"VREC:ACTIVATE\n",
        timeout,
        "automatic recording activation",
        ActiveControlTransientFallback::ReturnError,
    )
}

pub fn send_recording_control_enrollment(timeout: Duration) -> Result<(), String> {
    send_recording_control_command(
        b"VREC:ENROLL\n",
        timeout,
        "owner enrollment recording start",
        ActiveControlTransientFallback::TryFreshGatt,
    )
}

pub fn send_recording_control_speech_activity(timeout: Duration) -> Result<(), String> {
    let Some(result) = send_audio_control_via_active_capture(
        b"VREC:SPEECH\n",
        timeout,
        "recognized speech activity",
    ) else {
        return Err("no active Listener audio capture for recognized speech activity".to_string());
    };
    result
}

pub fn send_recording_control_stop(timeout: Duration) -> Result<(), String> {
    send_recording_stop_control_command(timeout)
}

pub fn send_recording_control_recovery(timeout: Duration) -> Result<(), String> {
    let serial_result =
        send_recovery_control_command_via_usb_serial("VREC:RECOVERY:TYPE", timeout);
    match &serial_result {
        Ok(()) => {
            log::info!("[embedded-ble] audio control recovery sent via USB serial");
            return Ok(());
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] audio control recovery USB serial path unavailable; trying BLE control: {err}"
            );
        }
    }
    send_recording_control_command(
        b"VREC:RECOVERY:TYPE\n",
        timeout,
        "audio control recovery",
        ActiveControlTransientFallback::TryFreshGatt,
    )
}

pub fn send_recording_control_manual_pairing(timeout: Duration) -> Result<(), String> {
    let serial_result =
        send_recovery_control_command_via_usb_serial("VREC:RECOVERY:TYPE:MANUAL", timeout);
    match &serial_result {
        Ok(()) => {
            log::info!("[embedded-ble] manual pairing recovery sent via USB serial");
            return Ok(());
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] manual pairing recovery USB serial path unavailable; trying BLE control: {err}"
            );
        }
    }
    send_recording_control_command(
        b"VREC:RECOVERY:TYPE:MANUAL\n",
        timeout,
        "manual pairing recovery",
        ActiveControlTransientFallback::TryFreshGatt,
    )
}

pub fn send_recording_control_silent_recovery(timeout: Duration) -> Result<(), String> {
    let serial_result =
        send_recovery_control_command_via_usb_serial("VREC:RECOVERY:TYPE:SILENT", timeout);
    match &serial_result {
        Ok(()) => {
            log::info!("[embedded-ble] audio control silent recovery sent via USB serial");
            return Ok(());
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] audio control silent recovery USB serial path unavailable; trying BLE control: {err}"
            );
        }
    }
    send_recording_control_command(
        b"VREC:RECOVERY:TYPE:SILENT\n",
        timeout,
        "audio control silent recovery",
        ActiveControlTransientFallback::TryFreshGatt,
    )
}

pub fn send_recording_control_silent_fresh_identity_recovery(
    timeout: Duration,
) -> Result<(), String> {
    let serial_result = send_recovery_control_command_via_usb_serial(
        "VREC:RECOVERY:TYPE:SILENT:FRESH",
        timeout,
    );
    match &serial_result {
        Ok(()) => {
            log::info!(
                "[embedded-ble] BLE rename fresh-identity recovery execution confirmed by firmware via USB serial"
            );
            return Ok(());
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] BLE rename fresh-identity recovery USB serial path unavailable; trying BLE control: {err}"
            );
        }
    }
    send_recording_control_command(
        b"VREC:RECOVERY:TYPE:SILENT:FRESH\n",
        timeout,
        "BLE rename fresh-identity recovery",
        ActiveControlTransientFallback::TryFreshGatt,
    )
}

pub fn send_recording_control_type_bye(timeout: Duration) -> Result<(), String> {
    if let Some(result) =
        send_audio_control_via_active_capture(b"TYPE:BYE\n", timeout, "audio type bye")
    {
        return result;
    }
    Err("active Listener BLE audio control unavailable for shutdown bye".to_string())
}

pub fn send_recording_processing_state(active: bool, timeout: Duration) -> Result<(), String> {
    let command = if active {
        b"VREC:PROCESSING:START\n".as_slice()
    } else {
        b"VREC:PROCESSING:STOP\n".as_slice()
    };
    let serial_command = if active {
        "VREC:PROCESSING:START"
    } else {
        "VREC:PROCESSING:STOP"
    };
    let label = if active {
        "audio processing start"
    } else {
        "audio processing stop"
    };
    send_processing_hint_control_command(command, serial_command, timeout, label)
}

pub fn send_recording_processing_done(timeout: Duration) -> Result<(), String> {
    send_processing_hint_control_command(
        b"VREC:PROCESSING:DONE\n",
        "VREC:PROCESSING:DONE",
        timeout,
        "audio processing done",
    )
}

pub fn send_recording_processing_warning(timeout: Duration) -> Result<(), String> {
    send_processing_hint_control_command(
        b"VREC:PROCESSING:WARN\n",
        "VREC:PROCESSING:WARN",
        timeout,
        "audio processing warning",
    )
}

pub fn send_ec11_rotation_mode(mode: &str, timeout: Duration) -> Result<(), String> {
    let command = format!("EC11:MODE:{mode}\n");
    if let Some(result) =
        send_audio_control_via_active_capture(command.as_bytes(), timeout, "EC11 rotation mode")
    {
        match result {
            Ok(()) => {
                log::info!(
                    "[embedded-ble] EC11 rotation mode sent via active capture mode={mode}"
                );
                return Ok(());
            }
            Err(err) if is_transient_audio_control_write_error(&err) => {
                log::warn!(
                    "[embedded-ble] active EC11 rotation write failed with transient error; retrying fresh GATT path: {err}"
                );
            }
            Err(err) => return Err(err),
        }
    }
    let _fresh_guard = BleFreshGattGuard::enter("EC11 rotation mode")?;
    let target = open_audio_control_target_with_retry("EC11 rotation mode")?;
    write_gatt_value_with_timeout(
        &target.control,
        command.as_bytes(),
        GattWriteOption::WriteWithResponse,
        timeout,
        "EC11 rotation mode",
    )?;
    log::info!("[embedded-ble] EC11 rotation mode sent mode={mode}");
    Ok(())
}
