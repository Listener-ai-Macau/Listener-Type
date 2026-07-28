// BLE capture event loop + active-control helpers (Windows).
// Included into `windows_ble` via `include!`.

fn send_audio_control_via_active_capture(
    bytes: &[u8],
    timeout: Duration,
    label: &str,
) -> Option<Result<(), String>> {
    let active = active_audio_control_sender()?;
    let (result_tx, result_rx) = mpsc::channel();
    let request = AudioControlRequest {
        bytes: bytes.to_vec(),
        label: label.to_string(),
        timeout,
        queued_at: Instant::now(),
        result_tx,
    };
    if active.tx.send(request).is_err() {
        clear_active_audio_control_sender(active.capture_id);
        return None;
    }
    let result = result_rx
        .recv_timeout(timeout + Duration::from_secs(1))
        .unwrap_or_else(|_| {
            Err(format!(
                "active Listener BLE audio control timed out after {} ms",
                timeout.as_millis()
            ))
        });
    if let Err(err) = &result {
        if is_transient_audio_control_write_error(err) {
            clear_active_audio_control_sender(active.capture_id);
        }
    }
    Some(result)
}

pub fn capture_notification_events(
    timeout: Duration,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    let mut on_ready = || Ok(());
    capture_notification_events_until_cancelled(
        Some(timeout),
        Arc::new(AtomicBool::new(false)),
        &mut on_ready,
        on_event,
    )
}

pub fn capture_notification_events_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    capture_notification_events_until_cancelled_impl(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
        CaptureTerminalBehavior::StopCapture,
        None,
    )
}

pub fn capture_notification_events_continuous_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    capture_notification_events_until_cancelled_impl(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
        CaptureTerminalBehavior::ContinueListening,
        None,
    )
}

pub fn capture_notification_events_continuous_with_connection_handoff_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    leave_notify_cccd_enabled_on_cancel: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    capture_notification_events_until_cancelled_impl(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
        CaptureTerminalBehavior::ContinueListening,
        Some(leave_notify_cccd_enabled_on_cancel),
    )
}

fn capture_notification_events_until_cancelled_impl(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
    terminal_behavior: CaptureTerminalBehavior,
    leave_notify_cccd_enabled_on_cancel: Option<Arc<AtomicBool>>,
) -> Result<(), String> {
    let _cancel_scope = NotifyCaptureCancelScope::install(&cancel_requested);
    if notify_capture_cancel_requested() {
        return Err(notify_capture_cancelled_error("notify capture open"));
    }
    let capture_guard = BleCaptureGuard::enter(idle_timeout)?;
    let capture_id = capture_guard.session_id();
    if terminal_behavior == CaptureTerminalBehavior::ContinueListening
        && ble_ota_process_mutex_busy()
    {
        log::info!(
            "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; deferring background listener before notify open"
        );
        return Err(background_listener_deferred_for_ota_error());
    }
    let target = open_notify_target_with_retry(capture_id)?;
    let characteristic = target.characteristic.clone();
    let (tx, rx) = mpsc::channel::<BleCaptureSignal>();
    let (control_tx, control_rx) = mpsc::channel::<AudioControlRequest>();
    let notification_tx = tx.clone();
    let notification_log_count = Arc::new(AtomicUsize::new(0));
    let notification_log_count_for_handler = Arc::clone(&notification_log_count);
    let handler = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
        move |_sender, args| {
            if let Some(args) = args {
                if let Ok(buffer) = args.CharacteristicValue() {
                    if let Ok(bytes) = buffer_to_vec(&buffer) {
                        let log_index =
                            notification_log_count_for_handler.fetch_add(1, Ordering::Relaxed);
                        if log_index < CAPTURE_NOTIFICATION_INFO_LOG_LIMIT {
                            let prefix_len = bytes.len().min(4);
                            log::info!(
                                "[embedded-ble] capture #{capture_id}: notification #{} bytes={} prefix={:02X?}",
                                log_index + 1,
                                bytes.len(),
                                &bytes[..prefix_len]
                            );
                        }
                        let _ = notification_tx.send(BleCaptureSignal::Notification(bytes));
                    }
                }
            }
            Ok(())
        },
    );

    let mut cleanup = NotifyCleanup::new(capture_id, target);
    if cleanup.target.control.is_some() {
        cleanup.set_audio_control_registration(ActiveAudioControlRegistration::install(
            capture_id, control_tx,
        ));
    } else {
        log::warn!(
            "[embedded-ble] capture #{capture_id}: audio control unavailable for active capture"
        );
    }
    let token = characteristic
        .ValueChanged(&handler)
        .map_err(|err| format!("BLE ValueChanged handler registration failed: {err}"))?;
    cleanup.set_token(token);
    log::info!("[embedded-ble] capture #{capture_id}: ValueChanged handler registered");
    let connection_token = cleanup.target.device.as_ref().and_then(|device| {
        register_device_connection_status_handler(capture_id, device, tx.clone())
    });
    if let Some(token) = connection_token {
        cleanup.set_connection_status_token(token);
    }
    let session_token = cleanup.target.session.as_ref().and_then(|session| {
        register_gatt_session_status_handler(capture_id, session, tx.clone())
    });
    if let Some(token) = session_token {
        cleanup.set_session_status_token(token);
    }
    #[cfg(debug_assertions)]
    register_validation_disconnect_injection_handler(
        capture_id,
        tx.clone(),
        Arc::clone(&cancel_requested),
    );

    if notify_cccd_prereset_required(terminal_behavior) {
        log::info!("[embedded-ble] capture #{capture_id}: resetting notify CCCD before enable");
        match write_cccd_with_timeout(
            &characteristic,
            GattClientCharacteristicConfigurationDescriptorValue::None,
            Duration::from_secs(2),
        ) {
            Ok(status) => {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: notify CCCD reset status={status:?}"
                )
            }
            Err(err) => {
                if let Some(recovery_error) =
                    cccd_notify_recovery_pairing_error(&err, cleanup.target.bluetooth_address)
                {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: notify CCCD reset hit recovery pairing window; entering Type PairAsync recovery before notify-enable retries: {err}"
                    );
                    return Err(recovery_error);
                }
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: notify CCCD reset skipped: {err}"
                )
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    } else {
        log::info!(
            "[embedded-ble] capture #{capture_id}: continuous listener enables notify CCCD without pre-reset"
        );
    }
    log::info!("[embedded-ble] capture #{capture_id}: enabling notify CCCD");
    let status = write_cccd_notify_with_retry(
        capture_id,
        "capture",
        &characteristic,
        CCCD_ENABLE_TIMEOUT,
        cleanup.target.bluetooth_address,
    )?;
    if status != GattCommunicationStatus::Success {
        return Err(format!("BLE CCCD notify write returned status={status:?}"));
    }
    log::info!("[embedded-ble] capture #{capture_id}: notify CCCD enabled");
    crate::startup_evidence::record_startup_stage("notify_cccd_enabled");
    let type_heartbeat_enabled =
        type_heartbeat_enabled_for_terminal_behavior(terminal_behavior);
    let mut next_type_heartbeat = None;
    // Notify subscription alone is not the recovery terminal state. The
    // firmware keeps its pairing/LED recovery window open until it has
    // accepted TYPE:READY (or a later Type heartbeat after a retry).
    let mut type_ready_confirmed = false;
    let type_ready_command = type_ready_command_bytes();
    if type_heartbeat_enabled && ble_ota_process_mutex_busy() {
        log::info!(
            "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; closing background listener before Type heartbeat ready"
        );
        cleanup.disable_notify();
        return Err(background_listener_deferred_for_ota_error());
    }
    match cleanup.write_type_heartbeat(&type_ready_command, "Type heartbeat ready") {
        Ok(()) => {
            crate::startup_evidence::record_startup_stage("type_ready_written");
            cleanup.mark_type_heartbeat_open();
            if let Some(address) = cleanup.target.bluetooth_address {
                persist_successful_notify_target_address(address, "Type heartbeat ready");
            }
            if let Err(err) = cleanup.write_type_heartbeat(
                b"TYPE:AUDIO:LOSSLESS_RICE:3\n",
                "Type lossless audio capability",
            ) {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: lossless audio capability was not acknowledged; firmware will retain raw PCM: {err}"
                );
            } else {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: lossless audio capability announced"
                );
            }
            on_ready()?;
            type_ready_confirmed = true;
            if type_heartbeat_enabled {
                next_type_heartbeat = Some(Instant::now() + TYPE_HEARTBEAT_INTERVAL);
            }
        }
        Err(err) => {
            if type_heartbeat_enabled {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: Type heartbeat ready failed; keeping notify open for retry: {err}"
                );
                next_type_heartbeat = Some(Instant::now() + TYPE_HEARTBEAT_INTERVAL);
            } else {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: Type ready failed; waiting for audio notifications anyway: {err}"
                );
            }
        }
    }

    let deadline = idle_timeout.map(|timeout| Instant::now() + timeout);
    let mut collector = crate::embedded_audio::SessionCollector::default();
    let mut stop_drain_deadline: Option<Instant> = None;
    let mut link_recovery_deadline: Option<Instant> = None;
    let mut link_recovery_reason: Option<String> = None;
    let mut ec11_recovery_prepare_disconnect_deadline: Option<Instant> = None;
    let mut ec11_recovery_disconnect_deadline: Option<Instant> = None;
    let mut consecutive_type_heartbeat_failures = 0u32;
    loop {
        cleanup.drain_audio_control_requests(&control_rx);
        let now = Instant::now();
        if type_heartbeat_enabled
            && !collector_has_active_recoverable_session(&collector)
            && ble_ota_process_mutex_busy()
        {
            log::info!(
                "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; closing idle background listener"
            );
            cleanup.disable_notify();
            return Err(background_listener_deferred_for_ota_error());
        }
        if let Some(due) = next_type_heartbeat {
            if now >= due {
                if let Err(err) = cleanup.write_type_heartbeat(b"TYPE:HB\n", "Type heartbeat") {
                    consecutive_type_heartbeat_failures =
                        consecutive_type_heartbeat_failures.saturating_add(1);
                    let reason = format!("{err}; BLE audio/control response missing");
                    if collector_has_active_recoverable_session(&collector) {
                        let stats = collector.stats();
                        if link_recovery_deadline.is_none() {
                            link_recovery_deadline =
                                Some(now + ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT);
                            link_recovery_reason = Some(reason.clone());
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: {reason}; waiting for audio notify recovery (session_id={:?}, packets={}, timeout_ms={})",
                                stats.session_id,
                                stats.received_packet_count,
                                ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                            );
                        } else {
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: additional heartbeat failure while waiting for recovery: {reason}"
                            );
                        }
                    } else {
                        let log_message =
                            format!("[embedded-ble] capture #{capture_id}: {reason}; keeping idle notify open for heartbeat retry");
                        if consecutive_type_heartbeat_failures == 1 {
                            log::info!("{log_message}");
                        } else {
                            log::warn!(
                                "{log_message}; consecutive_failures={consecutive_type_heartbeat_failures}"
                            );
                        }
                    }
                } else {
                    if consecutive_type_heartbeat_failures > 0 {
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: Type heartbeat recovered after {consecutive_type_heartbeat_failures} failure(s)"
                        );
                    }
                    consecutive_type_heartbeat_failures = 0;
                    cleanup.mark_type_heartbeat_open();
                    if !type_ready_confirmed {
                        on_ready()?;
                        type_ready_confirmed = true;
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: Type ready terminal confirmation recovered through heartbeat"
                        );
                    }
                }
                next_type_heartbeat = Some(now + TYPE_HEARTBEAT_INTERVAL);
            }
        }
        if cancel_requested.load(Ordering::SeqCst) {
            log::info!(
                "[embedded-ble] capture #{capture_id}: cancelled by caller; closing notify"
            );
            cleanup.finish_after_caller_cancel(
                terminal_behavior,
                leave_notify_cccd_enabled_on_cancel
                    .as_ref()
                    .is_some_and(|handoff| handoff.load(Ordering::SeqCst)),
            );
            return Ok(());
        }
        if deadline.is_some_and(|deadline| now >= deadline) {
            cleanup.log_embedded_audio_status_snapshot("capture timeout");
            return Err(format!(
                "BLE embedded audio capture timed out after {} ms",
                idle_timeout
                    .expect("deadline exists when timeout is reported")
                    .as_millis()
            ));
        }
        if stop_drain_deadline.is_some_and(|drain_deadline| now >= drain_deadline) {
            let stats = collector.stats();
            let reason = super::stop_drain_timeout_reason(&stats);
            log::warn!("[embedded-ble] {reason}");
            // Continuous background: a short post-STOP drain miss must not TYPE:BYE /
            // CCCD-off. Finalize the local collector and keep the notify subscription.
            if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: stop-drain timeout while continuous listening; keeping notify open (session_id={:?}, received={}, missing={})",
                    stats.session_id,
                    stats.received_packet_count,
                    stats.missing_packet_count
                );
                collector.reset();
                stop_drain_deadline = None;
                continue;
            }
            cleanup.disable_notify();
            return if crate::embedded_audio::transport_v1::stop_drain_expired_finalizes(
                &collector,
            ) {
                Ok(())
            } else {
                Err(reason)
            };
        }
        if link_recovery_deadline.is_some_and(|recovery_deadline| now >= recovery_deadline) {
            let reason = link_recovery_reason
                .as_deref()
                .unwrap_or("BLE link recovery timed out");
            let message = format!(
                "BLE embedded audio capture link recovery timed out after {} ms: {reason}",
                ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
            );
            log::warn!("[embedded-ble] capture #{capture_id}: {message}");
            cleanup.disable_notify();
            return Err(message);
        }
        if ec11_recovery_prepare_disconnect_deadline
            .is_some_and(|prepare_deadline| now >= prepare_deadline)
        {
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization expired without a firmware disconnect"
            );
            ec11_recovery_prepare_disconnect_deadline = None;
        }
        if ec11_recovery_disconnect_deadline
            .is_some_and(|disconnect_deadline| now >= disconnect_deadline)
        {
            let message = format!(
                "Listener EC11 hardware recovery notice did not receive the expected firmware disconnect within {} ms",
                EC11_HARDWARE_RECOVERY_DISCONNECT_TIMEOUT.as_millis()
            );
            log::warn!("[embedded-ble] capture #{capture_id}: {message}");
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
            return Err(message);
        }
        let receive_timeout = [
            stop_drain_deadline,
            deadline,
            link_recovery_deadline,
            ec11_recovery_prepare_disconnect_deadline,
            ec11_recovery_disconnect_deadline,
        ]
        .into_iter()
        .flatten()
        .chain(next_type_heartbeat)
        .map(|deadline| deadline.saturating_duration_since(now))
        .min()
        .unwrap_or(RECEIVE_POLL_INTERVAL)
        .min(RECEIVE_POLL_INTERVAL);
        let signal = match rx.recv_timeout(receive_timeout) {
            Ok(signal) => signal,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                if cancel_requested.load(Ordering::SeqCst) {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: cancelled by caller; closing notify"
                    );
                    cleanup.finish_after_caller_cancel(
                        terminal_behavior,
                        leave_notify_cccd_enabled_on_cancel
                            .as_ref()
                            .is_some_and(|handoff| handoff.load(Ordering::SeqCst)),
                    );
                    return Ok(());
                }
                if stop_drain_deadline.is_some_and(|drain_deadline| now >= drain_deadline) {
                    let stats = collector.stats();
                    let reason = super::stop_drain_timeout_reason(&stats);
                    log::warn!("[embedded-ble] {reason}");
                    if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: stop-drain timeout while continuous listening; keeping notify open (session_id={:?}, received={}, missing={})",
                            stats.session_id,
                            stats.received_packet_count,
                            stats.missing_packet_count
                        );
                        collector.reset();
                        stop_drain_deadline = None;
                        continue;
                    }
                    cleanup.disable_notify();
                    return if crate::embedded_audio::transport_v1::stop_drain_expired_finalizes(
                        &collector,
                    ) {
                        Ok(())
                    } else {
                        Err(reason)
                    };
                }
                if link_recovery_deadline
                    .is_some_and(|recovery_deadline| now >= recovery_deadline)
                {
                    let reason = link_recovery_reason
                        .as_deref()
                        .unwrap_or("BLE link recovery timed out");
                    let message = format!(
                        "BLE embedded audio capture link recovery timed out after {} ms: {reason}",
                        ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                    );
                    log::warn!("[embedded-ble] capture #{capture_id}: {message}");
                    cleanup.disable_notify();
                    return Err(message);
                }
                continue;
            }
            Err(err) => {
                return Err(format!(
                    "BLE embedded audio notification wait failed: {err}"
                ));
            }
        };
        let notification = match signal {
            BleCaptureSignal::Notification(notification) => {
                if link_recovery_deadline.take().is_some() {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: link recovered after active-session disconnect: {}",
                        link_recovery_reason
                            .take()
                            .unwrap_or_else(|| "unknown".to_string())
                    );
                }
                notification
            }
            BleCaptureSignal::Disconnected(reason) => {
                if ec11_recovery_disconnect_deadline.take().is_some() {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: firmware disconnect observed after EC11 recovery notice; releasing the retained GATT session for recovery arbitration"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
                    return Err(
                        "Listener EC11 hardware recovery notice received before pairing reset; Type observed the firmware disconnect and must scan the matching recovery advertisement and run automatic PairAsync recovery".to_string(),
                    );
                }
                if ec11_recovery_prepare_disconnect_deadline.take().is_some() {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: firmware disconnect observed during EC11 pre-authorized double-click window; recovery remains gated on matching advertising"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
                    return Err(
                        "Listener EC11 hardware recovery notice received before pairing reset via pre-authorization; Type must scan the matching recovery advertisement before automatic PairAsync recovery".to_string(),
                    );
                }
                if collector_has_active_recoverable_session(&collector) {
                    if let Some(recovery_error) =
                        active_capture_disconnect_recovery_pairing_error(&reason)
                    {
                        let stats = collector.stats();
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: recovery advertising proves the active session cannot resume; entering Type PairAsync recovery without the active-session wait (session_id={:?}, packets={})",
                            stats.session_id,
                            stats.received_packet_count,
                        );
                        cleanup.disable_notify();
                        return Err(recovery_error);
                    }
                    let stats = collector.stats();
                    if link_recovery_deadline.is_none() {
                        link_recovery_deadline =
                            Some(Instant::now() + ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT);
                        link_recovery_reason = Some(reason.clone());
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: {reason}; keeping notify open for active session recovery (session_id={:?}, packets={}, timeout_ms={})",
                            stats.session_id,
                            stats.received_packet_count,
                            ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                        );
                    } else {
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: additional active-session disconnect while waiting for recovery: {reason}"
                        );
                    }
                    continue;
                }
                log::warn!("[embedded-ble] capture #{capture_id}: {reason}");
                cleanup.disable_notify();
                return Err(reason);
            }
        };
        if super::is_ec11_hardware_recovery_prepare_notice(&notification) {
            log::info!(
                "[embedded-ble] capture #{capture_id}: received EC11 recovery pre-authorization during the double-click window"
            );
            if let Err(err) = cleanup.write_ec11_recovery_prepare_acknowledgement() {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization acknowledgement was not queued: {err}"
                );
                continue;
            }
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization acknowledgement queued via active GATT control"
            );
            ec11_recovery_prepare_disconnect_deadline =
                Some(Instant::now() + EC11_HARDWARE_RECOVERY_PREPARE_TIMEOUT);
            continue;
        }
        if super::is_ec11_hardware_recovery_notice(&notification) {
            log::warn!(
                "[embedded-ble] capture #{capture_id}: received EC11 hardware recovery notice; retaining the GATT session until the firmware disconnect completes"
            );
            if let Err(err) = cleanup.write_ec11_recovery_acknowledgement() {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: EC11 recovery notice acknowledgement was not queued; retaining the existing GATT session without PairAsync authorization: {err}"
                );
                continue;
            }
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery acknowledgement queued via active GATT control"
            );
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            ec11_recovery_disconnect_deadline =
                Some(Instant::now() + EC11_HARDWARE_RECOVERY_DISCONNECT_TIMEOUT);
            continue;
        }
        let notification = crate::audio_transport_codec::normalize_listener_audio_notification(
            &notification,
        )
        .map_err(|err| {
            format!(
                "[embedded-ble] capture #{capture_id}: lossless audio notification rejected: {err}"
            )
        })?;
        let terminal = super::is_terminal_notification(&notification);
        let local_event = collector.handle_notification(&notification).ok();
        on_event(crate::embedded_ble::BleNotificationEvent {
            notification,
            terminal,
        })?;
        if matches!(
            local_event,
            Some(crate::embedded_audio::SessionEvent::Cancelled { .. })
                | Some(crate::embedded_audio::SessionEvent::Error { .. })
        ) {
            if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: terminal cancel/error while continuous listening; keeping notify open for the next session"
                );
                collector.reset();
                stop_drain_deadline = None;
                continue;
            }
            cleanup.disable_notify();
            return Ok(());
        }
        if matches!(
            local_event,
            Some(crate::embedded_audio::SessionEvent::Stopped { .. })
        ) {
            stop_drain_deadline = Some(Instant::now() + super::STOP_DRAIN_TIMEOUT);
        }
        if collector.has_successful_complete_session() {
            if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                let stats = collector.stats();
                log::info!(
                    "[embedded-ble] capture #{capture_id}: complete session received; keeping notify open for background listener (session_id={:?}, pcm_bytes={}, packets={})",
                    stats.session_id,
                    stats.received_pcm_bytes,
                    stats.received_packet_count
                );
                collector.reset();
                stop_drain_deadline = None;
                continue;
            }
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            cleanup.disable_notify();
            return Ok(());
        }
        if collector.terminal_received() && stop_drain_deadline.is_some() {
            stop_drain_deadline = Some(Instant::now() + super::STOP_DRAIN_TIMEOUT);
        }
    }
}
