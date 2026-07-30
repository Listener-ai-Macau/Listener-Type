// Embedded BLE background listener / recovery free functions.
// Included into `coordinator` via `include!` for soft-budget maintainability.

fn embedded_ble_listener_last_error(inner: &Arc<Inner>) -> Option<String> {
    inner.embedded_ble_listener_last_error.lock().clone()
}

fn record_embedded_ble_listener_last_error(inner: &Arc<Inner>, err: &str) {
    *inner.embedded_ble_listener_last_error.lock() = Some(err.to_string());
}

fn clear_embedded_ble_listener_last_error(inner: &Arc<Inner>) {
    *inner.embedded_ble_listener_last_error.lock() = None;
}

fn embedded_ble_recovery_error_still_current(
    inner: &Arc<Inner>,
    err: &str,
    stage: &'static str,
) -> bool {
    if inner.embedded_ble_listener_ready.load(Ordering::SeqCst) {
        log::info!(
            "[embedded-ble] background stale pairing cleanup aborted because notify is already ready stage={stage}"
        );
        return false;
    }
    let last_error = embedded_ble_listener_last_error(inner);
    if last_error.as_deref() != Some(err) {
        log::info!(
            "[embedded-ble] background stale pairing cleanup aborted because recovery error is stale stage={stage} current_error={}",
            last_error
                .as_deref()
                .map(embedded_ble_log_preview)
                .unwrap_or_else(|| "none".to_string())
        );
        return false;
    }
    true
}

fn record_embedded_ble_session_actor_command(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
) -> u64 {
    dispatch_embedded_ble_session_actor_command_with_trace(
        inner,
        command,
        session_id,
        detail,
        true,
        |seq| seq,
    )
}

fn dispatch_embedded_ble_session_actor_command<T>(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
    handle: impl FnOnce(u64) -> T,
) -> T {
    dispatch_embedded_ble_session_actor_command_with_trace(
        inner, command, session_id, detail, true, handle,
    )
}

fn dispatch_embedded_ble_session_actor_command_with_trace<T>(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
    trace_timeline: bool,
    handle: impl FnOnce(u64) -> T,
) -> T {
    let detail = detail.into();
    let (seq, result) = {
        let mut actor = inner.embedded_ble_session_actor.lock();
        actor.next_seq = actor.next_seq.saturating_add(1);
        let seq = actor.next_seq;
        actor.history.push_back(EmbeddedBleSessionActorRecord {
            seq,
            command,
            session_id,
            detail: detail.clone(),
        });
        while actor.history.len() > EMBEDDED_BLE_SESSION_ACTOR_HISTORY_LIMIT {
            actor.history.pop_front();
        }
        let result = handle(seq);
        (seq, result)
    };
    if trace_timeline {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            command.as_str(),
            format!("seq={seq} session_id={session_id:?} {detail}"),
        );
    }
    result
}

fn should_trace_embedded_ble_pcm_capsule(
    inner: &Arc<Inner>,
    session_id: SessionId,
    after_stop: bool,
) -> bool {
    let mut actor = inner.embedded_ble_session_actor.lock();
    actor
        .pcm_capsule_trace
        .should_trace(session_id, after_stop, Instant::now())
}

#[cfg(test)]
fn embedded_ble_session_actor_history(inner: &Arc<Inner>) -> Vec<EmbeddedBleSessionActorRecord> {
    inner
        .embedded_ble_session_actor
        .lock()
        .history
        .iter()
        .cloned()
        .collect()
}

fn embedded_ble_session_actor_diagnostics(
    inner: &Arc<Inner>,
) -> Vec<EmbeddedBleSessionActorDiagnosticRecord> {
    inner
        .embedded_ble_session_actor
        .lock()
        .history
        .iter()
        .map(EmbeddedBleSessionActorRecord::diagnostic)
        .collect()
}

fn hold_embedded_ble_listener_for_pairing_confirmation(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> u64 {
    hold_embedded_ble_listener_for_pairing_confirmation_for(
        inner,
        reason,
        EMBEDDED_BLE_PAIRING_CONFIRMATION_HOLD,
    )
}

fn hold_embedded_ble_listener_for_pairing_confirmation_for(
    inner: &Arc<Inner>,
    reason: &'static str,
    duration: Duration,
) -> u64 {
    clear_embedded_ble_passive_local_reattach(inner, reason);
    let generation = inner
        .embedded_ble_pairing_hold_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let until = Instant::now() + duration;
    {
        let mut hold = inner.embedded_ble_pairing_hold_until.lock();
        *hold = Some(until);
    }
    pause_embedded_ble_listener_capture(inner, reason);
    log::info!(
        "[embedded-ble] background listener held for Windows pairing confirmation reason={reason} hold_generation={generation} hold_ms={}",
        duration.as_millis()
    );
    generation
}

fn clear_embedded_ble_pairing_confirmation_hold(inner: &Arc<Inner>, reason: &'static str) {
    let had_hold = inner
        .embedded_ble_pairing_hold_until
        .lock()
        .take()
        .is_some();
    if had_hold {
        inner
            .embedded_ble_pairing_hold_generation
            .fetch_add(1, Ordering::SeqCst);
        log::info!("[embedded-ble] Windows pairing confirmation hold cleared reason={reason}");
    }
}

fn clear_embedded_ble_passive_local_reattach(inner: &Arc<Inner>, reason: &'static str) {
    if inner
        .embedded_ble_passive_local_reattach_active
        .swap(false, Ordering::SeqCst)
    {
        log::info!("[embedded-ble] passive local Windows reattach monitor cleared reason={reason}");
    }
}

fn embedded_ble_pairing_confirmation_hold_remaining(
    inner: &Arc<Inner>,
    now: Instant,
) -> Option<Duration> {
    let mut hold = inner.embedded_ble_pairing_hold_until.lock();
    match *hold {
        Some(until) if until > now => Some(until.saturating_duration_since(now)),
        Some(_) => {
            *hold = None;
            inner
                .embedded_ble_pairing_hold_generation
                .fetch_add(1, Ordering::SeqCst);
            log::info!("[embedded-ble] Windows pairing confirmation hold expired");
            None
        }
        None => None,
    }
}

fn embedded_ble_pairing_prompt_ready(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::Paired
            | crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
    ) && !pairing.open_bluetooth_settings
        && pairing.failed_devices == 0
}

fn embedded_ble_pairing_confirmation_ready(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
    native_hid_addresses: &[u64],
) -> bool {
    embedded_ble_pairing_prompt_ready(pairing) || !native_hid_addresses.is_empty()
}

fn embedded_ble_pairing_confirmation_expiry_should_refresh_background(
    reason: &'static str,
) -> bool {
    reason != EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON
        && reason != EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON
        && reason != EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON
        && reason != EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON
        && reason != EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON
}

struct EmbeddedBlePairingRecoveryGuard {
    inner: Arc<Inner>,
    reason: &'static str,
}

impl Drop for EmbeddedBlePairingRecoveryGuard {
    fn drop(&mut self) {
        self.inner
            .embedded_ble_pairing_recovery_active
            .store(false, Ordering::SeqCst);
        log::info!(
            "[embedded-ble] pairing recovery guard released reason={}",
            self.reason
        );
    }
}

fn try_begin_embedded_ble_pairing_recovery(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Option<EmbeddedBlePairingRecoveryGuard> {
    match inner.embedded_ble_pairing_recovery_active.compare_exchange(
        false,
        true,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        Ok(_) => {
            log::info!("[embedded-ble] pairing recovery guard acquired reason={reason}");
            Some(EmbeddedBlePairingRecoveryGuard {
                inner: Arc::clone(inner),
                reason,
            })
        }
        Err(_) => {
            log::warn!(
                "[embedded-ble] pairing recovery guard busy; deferring overlapping recovery reason={reason}"
            );
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EmbeddedBleStalePairingCleanupOutcome {
    Skipped,
    RetryImmediate,
    RetrySoon,
    HoldForConfirmation,
}

fn embedded_ble_pairing_prompt_waiting_for_windows(
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
) -> bool {
    let Some(pairing) = pairing else {
        return true;
    };
    if embedded_ble_pairing_prompt_ready(pairing) {
        return false;
    }
    match pairing.status {
        crate::embedded_ble::BleDevicePairingPromptStatus::NotFound => pairing.failed_devices == 0,
        crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction => {
            pairing.open_bluetooth_settings
                || pairing.matched_devices > 0
                || pairing.failed_devices == 0
        }
        _ => false,
    }
}

fn embedded_ble_failed_recovery_pairing_should_retry_soon(
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
    recovery_pairing_probe: crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> bool {
    let Some(pairing) = pairing else {
        return false;
    };
    if !recovery_pairing_probe.visible
        || !recovery_pairing_probe.has_random_identity
        || embedded_ble_pairing_prompt_ready(pairing)
    {
        return false;
    }

    matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::NotFound
    ) || (pairing.failed_devices > 0
        && pairing.prompted_devices == 0
        && pairing.already_paired_devices == 0)
}

fn embedded_ble_pairing_recovery_accepts_link_reachable(
    reason: &'static str,
    pairing_ready: bool,
) -> bool {
    if pairing_ready {
        return true;
    }

    !matches!(
        reason,
        EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON
            | EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON
            | EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON
            | EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON
            | EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON
    )
}

fn open_windows_bluetooth_settings_for_embedded_ble_pairing(reason: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        fn wide_null(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(std::iter::once(0)).collect()
        }

        let operation = wide_null("open");
        let target = wide_null("ms-settings:bluetooth");
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(operation.as_ptr()),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            log::warn!(
                "[embedded-ble] open Windows Bluetooth settings failed reason={} shell_result={}",
                reason,
                result.0 as isize
            );
        } else {
            log::info!("[embedded-ble] opened Windows Bluetooth settings reason={reason}");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = reason;
    }
}

fn mark_embedded_ble_pairing_link_reachable(inner: &Arc<Inner>, reason: &'static str) {
    let mut wake = inner.embedded_ble_wake_recovery.lock();
    wake.status = EmbeddedBleWakeRecoveryStatus::Reconnecting;
    wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Opening;
    wake.recent_disconnect_reason = Some(format!(
        "{reason}: local recovery evidence accepted; restoring background notify"
    ));
    wake.user_guidance = "Listener 蓝牙已经重新连上，Type 正在恢复音频 notify。".to_string();
}

fn resume_embedded_ble_listener_after_pairing_recovery(
    inner: &Arc<Inner>,
    reason: &'static str,
    message: EmbeddedBleRecoveryCapsuleMessage,
    emit_reconnecting_capsule: bool,
) {
    clear_embedded_ble_passive_local_reattach(inner, reason);
    mark_embedded_ble_pairing_link_reachable(inner, reason);
    if emit_reconnecting_capsule {
        emit_embedded_ble_recovery_capsule(inner, "reconnecting", message, Some(1800));
    } else {
        log::info!(
            "[embedded-ble] EC11 Type-controlled recovery suppresses intermediate capsule until firmware Type-ready terminal confirmation"
        );
    }
    clear_embedded_ble_pairing_confirmation_hold(inner, reason);
    refresh_embedded_ble_listener(inner);
}

fn arm_embedded_ble_type_pairasync_startup_guard(inner: &Arc<Inner>) {
    let until = Instant::now() + EMBEDDED_BLE_TYPE_PAIRASYNC_STARTUP_GUARD;
    *inner.embedded_ble_type_pairasync_startup_guard_until.lock() = Some(until);
    log::info!(
        "[embedded-ble] Type PairAsync startup manual-delete guard armed duration_ms={}",
        EMBEDDED_BLE_TYPE_PAIRASYNC_STARTUP_GUARD.as_millis()
    );
}

fn embedded_ble_type_pairasync_startup_guard_active(inner: &Arc<Inner>) -> bool {
    let now = Instant::now();
    let mut guard_until = inner.embedded_ble_type_pairasync_startup_guard_until.lock();
    if guard_until.is_some_and(|until| now < until) {
        return true;
    }
    *guard_until = None;
    false
}

async fn embedded_ble_pairing_recovery_link_reachable(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> bool {
    embedded_ble_pairing_recovery_link_reachable_with_timeout(
        inner,
        reason,
        EMBEDDED_BLE_PAIRING_GATT_REBUILD_TIMEOUT,
        None,
    )
    .await
}

async fn embedded_ble_pairing_recovery_link_reachable_with_timeout(
    inner: &Arc<Inner>,
    reason: &'static str,
    timeout: Duration,
    preferred_address: Option<u64>,
) -> bool {
    if inner.shutdown.load(Ordering::SeqCst)
        || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
    {
        return false;
    }
    let result = async_runtime::spawn_blocking(move || match preferred_address {
        Some(address) => {
            crate::embedded_ble::read_embedded_audio_status_for_device(address, timeout)
        }
        None => crate::embedded_ble::read_embedded_audio_status(timeout),
    })
    .await;
    match result {
        Ok(Ok(status)) if status.connected => {
            log::info!(
                "[embedded-ble] {reason}: Listener GATT reachable during pairing recovery detail={:?}",
                status.detail
            );
            true
        }
        Ok(Ok(status)) => {
            log::info!(
                "[embedded-ble] {reason}: Listener GATT status was not connected during pairing recovery detail={:?}",
                status.detail
            );
            false
        }
        Ok(Err(err)) => {
            log::info!(
                "[embedded-ble] {reason}: Listener GATT not reachable during pairing recovery: {}",
                embedded_ble_log_preview(&err)
            );
            false
        }
        Err(err) => {
            log::warn!("[embedded-ble] {reason}: pairing recovery reachability task failed: {err}");
            false
        }
    }
}

fn start_embedded_ble_passive_local_reattach_watch(
    inner: &Arc<Inner>,
    expected_ble_name: String,
    reason: &'static str,
) {
    if inner.shutdown.load(Ordering::SeqCst)
        || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
    {
        return;
    }
    if inner
        .embedded_ble_passive_local_reattach_active
        .swap(true, Ordering::SeqCst)
    {
        log::info!(
            "[embedded-ble] passive local Windows reattach monitor already active reason={reason} target={expected_ble_name:?}"
        );
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        let monitor_started = Instant::now();
        let baseline_native_hid_addresses = match async_runtime::spawn_blocking(|| {
            // The present-only CIM query is ~3x cheaper than the full Get-PnpDevice
            // enumeration and is the correct evidence class here: a fresh local
            // re-pair always produces present PnP nodes.
            crate::embedded_ble::native_windows_hid_present_pairing_addresses()
        })
        .await
        {
            Ok(Ok(addresses)) => Some(addresses),
            Ok(Err(err)) => {
                log::debug!(
                    "[embedded-ble] passive local Windows reattach baseline HID check unavailable: {err}"
                );
                None
            }
            Err(err) => {
                log::debug!(
                    "[embedded-ble] passive local Windows reattach baseline HID task failed: {err}"
                );
                None
            }
        };
        let baseline_labels = baseline_native_hid_addresses
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|address| format!("{address:012X}"))
            .collect::<Vec<_>>();
        log::info!(
            "[embedded-ble] passive local Windows reattach monitor started reason={reason} target={expected_ble_name:?}; baseline_native_hid_addresses={baseline_labels:?}; waiting only for explicit local pairing/HID evidence"
        );
        loop {
            if inner.shutdown.load(Ordering::SeqCst)
                || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            {
                clear_embedded_ble_passive_local_reattach(&inner, reason);
                log::info!(
                    "[embedded-ble] passive local Windows reattach monitor stopped reason={reason}"
                );
                break;
            }
            if !inner
                .embedded_ble_passive_local_reattach_active
                .load(Ordering::SeqCst)
            {
                log::info!(
                    "[embedded-ble] passive local Windows reattach monitor superseded reason={reason}"
                );
                break;
            }

            let native_hid_pairing = async_runtime::spawn_blocking(|| {
                crate::embedded_ble::native_windows_hid_present_pairing_addresses()
            })
            .await;
            let native_hid_addresses = match native_hid_pairing {
                Ok(Ok(addresses)) => addresses,
                Ok(Err(err)) => {
                    log::debug!(
                        "[embedded-ble] passive local Windows reattach native HID check unavailable: {err}"
                    );
                    Vec::new()
                }
                Err(err) => {
                    log::debug!(
                        "[embedded-ble] passive local Windows reattach native HID task failed: {err}"
                    );
                    Vec::new()
                }
            };
            let fresh_native_hid_address =
                baseline_native_hid_addresses
                    .as_deref()
                    .and_then(|baseline| {
                        native_hid_addresses
                            .iter()
                            .copied()
                            .find(|address| !baseline.contains(address))
                    });
            let fresh_native_hid_evidence = fresh_native_hid_address.is_some();
            let pairing = if fresh_native_hid_evidence {
                None
            } else {
                let expected_for_query = expected_ble_name.clone();
                match async_runtime::spawn_blocking(move || {
                    crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
                })
                .await
                {
                    Ok(pairing) => Some(pairing),
                    Err(err) => {
                        log::debug!(
                            "[embedded-ble] passive local Windows reattach pairing poll unavailable: {err}"
                        );
                        None
                    }
                }
            };
            if embedded_ble_passive_local_reattach_evidence_ready(
                pairing.as_ref(),
                &native_hid_addresses,
                baseline_native_hid_addresses.as_deref(),
            ) {
                let labels = native_hid_addresses
                    .iter()
                    .map(|address| format!("{address:012X}"))
                    .collect::<Vec<_>>();
                let evidence = if fresh_native_hid_evidence {
                    "new native HID address after passive monitor baseline"
                } else {
                    "current Windows pairing plus Listener HID"
                };
                log::info!(
                    "[embedded-ble] passive local Windows reattach accepted explicit local pairing evidence={evidence} status={:?} matched={} already_paired={} native_hid_addresses={labels:?}; rebuilding GATT",
                    pairing.as_ref().map(|value| value.status),
                    pairing.as_ref().map_or(0, |value| value.matched_devices),
                    pairing.as_ref().map_or(0, |value| value.already_paired_devices)
                );
                if let Some(address) = fresh_native_hid_address {
                    *inner.embedded_ble_power_cycle_hid_observed_at.lock() = Some(Instant::now());
                    log::info!(
                        "[embedded-ble] passive local Windows reattach observed new local HID address={address:012X}; reopening background notify directly without the status-characteristic probe"
                    );
                    arm_embedded_ble_type_pairasync_startup_guard(&inner);
                    resume_embedded_ble_listener_after_pairing_recovery(
                        &inner,
                        "passive local Windows reattach new HID evidence",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        true,
                    );
                    break;
                }
                let link_reachable = embedded_ble_pairing_recovery_link_reachable(
                    &inner,
                    "passive local Windows reattach current-pairing GATT link check",
                )
                .await;
                if link_reachable {
                    // Windows may still rebuild the HID/GATT service graph after the
                    // fresh local pairing is reachable. Skip one startup stale-HID
                    // preflight during that bounded rebuild window; otherwise the
                    // old deleted address can pause notify immediately again.
                    arm_embedded_ble_type_pairasync_startup_guard(&inner);
                    resume_embedded_ble_listener_after_pairing_recovery(
                        &inner,
                        "passive local Windows reattach paired and link reachable",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        true,
                    );
                    break;
                }
            } else {
                log::debug!(
                    "[embedded-ble] passive local Windows reattach still waiting for current local pairing plus HID evidence; no GATT probe status={:?} matched={} already_paired={} native_hid_count={} baseline_hid_count={}",
                    pairing.as_ref().map(|value| value.status),
                    pairing.as_ref().map_or(0, |value| value.matched_devices),
                    pairing.as_ref().map_or(0, |value| value.already_paired_devices),
                    native_hid_addresses.len(),
                    baseline_native_hid_addresses.as_ref().map_or(0, Vec::len)
                );
            }

            // Escape hatch: after ~45s still no strict "new HID after baseline"
            // evidence. Prefer any present Listener HID (even if baseline-matched /
            // PnP lagged) and reopen notify. If HID is absent, stop and wait for
            // an explicit pairing action. A passive watcher must never race a
            // native Windows prompt with PairAsync or an adapter restart.
            if monitor_started.elapsed() >= Duration::from_secs(45) {
                log::warn!(
                    "[embedded-ble] passive local Windows reattach timed out after {} ms without strict HID evidence target={expected_ble_name:?}; checking present HID before returning to explicit-pair wait",
                    monitor_started.elapsed().as_millis()
                );
                clear_embedded_ble_passive_local_reattach(
                    &inner,
                    "passive reattach timeout returns to explicit-pair wait",
                );
                clear_embedded_ble_pairing_confirmation_hold(
                    &inner,
                    "passive reattach timeout returns to explicit-pair wait",
                );

                let present_hid = async_runtime::spawn_blocking(|| {
                    crate::embedded_ble::native_windows_hid_present_pairing_addresses()
                })
                .await;
                let present_hid_addresses = match present_hid {
                    Ok(Ok(addresses)) => addresses,
                    Ok(Err(err)) => {
                        log::warn!(
                            "[embedded-ble] passive reattach timeout present HID check failed: {err}"
                        );
                        Vec::new()
                    }
                    Err(err) => {
                        log::warn!(
                            "[embedded-ble] passive reattach timeout present HID task failed: {err}"
                        );
                        Vec::new()
                    }
                };
                if let Some(address) = present_hid_addresses.first().copied() {
                    *inner.embedded_ble_power_cycle_hid_observed_at.lock() = Some(Instant::now());
                    log::info!(
                        "[embedded-ble] passive reattach timeout found present Listener HID address={address:012X}; reopening notify without PairAsync native_hid_addresses={:?}",
                        present_hid_addresses
                            .iter()
                            .map(|value| format!("{value:012X}"))
                            .collect::<Vec<_>>()
                    );
                    arm_embedded_ble_type_pairasync_startup_guard(&inner);
                    resume_embedded_ble_listener_after_pairing_recovery(
                        &inner,
                        "passive reattach timeout present HID reopen",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        true,
                    );
                    break;
                }

                {
                    let mut wake = inner.embedded_ble_wake_recovery.lock();
                    wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
                    wake.notify_subscription_state =
                        EmbeddedBleNotifySubscriptionState::Cancelled;
                    wake.recent_disconnect_reason = Some(format!(
                        "passive local reattach timed out without present Listener HID target={expected_ble_name}"
                    ));
                    wake.user_guidance = format!(
                        "Listener 正在等待 Windows 重新配对 {expected_ble_name}。Type 不会在后台重试配对或重启蓝牙。"
                    );
                }
                log::warn!(
                    "[embedded-ble] passive reattach timeout still has no present Listener HID target={expected_ble_name:?}; background PairAsync suppressed until explicit pairing"
                );
                emit_embedded_ble_recovery_capsule(
                    &inner,
                    "reconnecting",
                    EmbeddedBleRecoveryCapsuleMessage::WaitingManualPairing,
                    Some(4200),
                );
                break;
            }

            tokio::time::sleep(EMBEDDED_BLE_PASSIVE_LOCAL_REATTACH_POLL).await;
        }
    });
}

fn embedded_ble_passive_local_reattach_evidence_ready(
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
    native_hid_addresses: &[u64],
    baseline_native_hid_addresses: Option<&[u64]>,
) -> bool {
    let paired_devices_visible = pairing.is_some_and(|value| value.already_paired_devices > 0);
    let fresh_native_hid_after_baseline = baseline_native_hid_addresses.is_some_and(|baseline| {
        native_hid_addresses
            .iter()
            .any(|address| !baseline.contains(address))
    });
    denzic_ble_pairing::reattach_evidence_ready(
        paired_devices_visible,
        !native_hid_addresses.is_empty(),
        fresh_native_hid_after_baseline,
    )
}

fn start_embedded_ble_pairing_confirmation_watch(
    inner: &Arc<Inner>,
    expected_ble_name: String,
    hold_generation: u64,
    reason: &'static str,
) {
    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        log::info!(
            "[embedded-ble] Windows pairing confirmation watch started reason={reason} hold_generation={hold_generation} target={expected_ble_name:?}"
        );
        loop {
            if inner.shutdown.load(Ordering::SeqCst)
                || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            {
                log::info!(
                    "[embedded-ble] Windows pairing confirmation watch stopped reason={reason}"
                );
                break;
            }
            if inner
                .embedded_ble_pairing_hold_generation
                .load(Ordering::SeqCst)
                != hold_generation
            {
                log::info!(
                    "[embedded-ble] Windows pairing confirmation watch superseded reason={reason} hold_generation={hold_generation}"
                );
                break;
            }
            let Some(remaining) =
                embedded_ble_pairing_confirmation_hold_remaining(&inner, Instant::now())
            else {
                if !embedded_ble_pairing_confirmation_expiry_should_refresh_background(reason) {
                    log::warn!(
                        "[embedded-ble] Windows pairing confirmation watch expired after user-controlled recovery; leaving background listener paused until explicit Windows pairing or next scheduled retry reason={reason}"
                    );
                    {
                        let mut wake = inner.embedded_ble_wake_recovery.lock();
                        wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
                        wake.notify_subscription_state =
                            EmbeddedBleNotifySubscriptionState::Cancelled;
                        wake.recent_disconnect_reason = Some(format!(
                            "{reason} hold expired without confirmed Listener GATT recovery"
                        ));
                        wake.user_guidance = format!(
                            "Listener 正在等待 Windows 重新配对 {expected_ble_name}。Type 不会自动抢回连接；如需继续使用这台电脑，请在 Windows 蓝牙里手动添加设备。"
                        );
                    }
                    start_embedded_ble_passive_local_reattach_watch(
                        &inner,
                        expected_ble_name.clone(),
                        reason,
                    );
                    break;
                }
                log::warn!(
                    "[embedded-ble] Windows pairing confirmation watch expired; refreshing background listener reason={reason}"
                );
                refresh_embedded_ble_listener(&inner);
                break;
            };

            let expected_for_query = expected_ble_name.clone();
            let query = async_runtime::spawn_blocking(move || {
                crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
            })
            .await;
            match query {
                Ok(pairing) => {
                    log::info!(
                        "[embedded-ble] Windows pairing confirmation poll status={:?} matched={} already_paired={} failed={} open_settings={} remaining_ms={}",
                        pairing.status,
                        pairing.matched_devices,
                        pairing.already_paired_devices,
                        pairing.failed_devices,
                        pairing.open_bluetooth_settings,
                        remaining.as_millis()
                    );
                    let native_hid_pairing = async_runtime::spawn_blocking(|| {
                        crate::embedded_ble::native_windows_hid_pairing_addresses()
                    })
                    .await;
                    let native_hid_addresses = match native_hid_pairing {
                        Ok(Ok(addresses)) => addresses,
                        Ok(Err(err)) => {
                            log::warn!(
                                "[embedded-ble] Windows pairing confirmation native HID check unavailable: {err}"
                            );
                            Vec::new()
                        }
                        Err(err) => {
                            log::warn!(
                                "[embedded-ble] Windows pairing confirmation native HID task failed: {err}"
                            );
                            Vec::new()
                        }
                    };
                    if !native_hid_addresses.is_empty() {
                        let labels = native_hid_addresses
                            .iter()
                            .map(|address| format!("{address:012X}"))
                            .collect::<Vec<_>>();
                        log::info!(
                            "[embedded-ble] Windows pairing confirmation accepted complete native Listener HID evidence addresses={labels:?} while paired-AEP state settles"
                        );
                    }
                    let pairing_ready =
                        embedded_ble_pairing_confirmation_ready(&pairing, &native_hid_addresses);
                    let link_reachable = if embedded_ble_pairing_recovery_accepts_link_reachable(
                        reason,
                        pairing_ready,
                    ) {
                        embedded_ble_pairing_recovery_link_reachable(
                            &inner,
                            if pairing_ready {
                                "Windows pairing confirmation watch paired link check"
                            } else {
                                "Windows pairing confirmation watch link reachable"
                            },
                        )
                        .await
                    } else {
                        log::info!(
                                "[embedded-ble] Windows pairing confirmation watch ignoring GATT reachability until Windows pairing is confirmed reason={reason} remaining_ms={}",
                                remaining.as_millis()
                            );
                        false
                    };
                    if link_reachable {
                        resume_embedded_ble_listener_after_pairing_recovery(
                            &inner,
                            if pairing_ready {
                                "Windows pairing confirmation watch paired and link reachable"
                            } else {
                                "Windows pairing confirmation watch link reachable"
                            },
                            if pairing_ready {
                                EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio
                            } else {
                                EmbeddedBleRecoveryCapsuleMessage::RestoringAudio
                            },
                            true,
                        );
                        break;
                    }
                    if pairing_ready {
                        log::info!(
                            "[embedded-ble] Windows pairing confirmation watch paired; waiting for Listener GATT to become ready remaining_ms={}",
                            remaining.as_millis()
                        );
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] Windows pairing confirmation poll task failed: {err}"
                    );
                }
            }

            tokio::time::sleep(remaining.min(EMBEDDED_BLE_PAIRING_CONFIRMATION_POLL)).await;
        }
    });
}

fn startup_ble_name_sync_reason(reason: &'static str) -> bool {
    matches!(
        reason,
        "startup_embedded_ble_power_probe" | "auto_input_source_probe"
    )
}

fn mark_startup_ble_name_sync_done(inner: &Arc<Inner>, reason: &'static str) {
    if !startup_ble_name_sync_reason(reason) {
        return;
    }
    let was_done = inner
        .embedded_ble_startup_name_sync_done
        .swap(true, Ordering::SeqCst);
    if !was_done {
        log::info!("[embedded-ble] startup BLE name sync gate opened reason={reason}");
    }
}

/// After notify is already armed, best-effort settings/power polish only.
/// Must not re-close the startup gate or cancel an active capture.
fn polish_startup_ble_settings_after_fast_open(inner: &Arc<Inner>, reason: &'static str) {
    const STARTUP_SETTINGS_POLISH_TIMEOUT: Duration = Duration::from_millis(450);
    match crate::embedded_ble::read_device_settings_status(STARTUP_SETTINGS_POLISH_TIMEOUT) {
        Ok(status) => {
            record_embedded_ble_device_settings_power_status(inner, &status, reason);
            let firmware_name = status.ble_name.trim();
            let valid = crate::types::device_ble_name_is_valid(firmware_name);
            if status.ble_name_pending_restart || !valid {
                log::info!(
                    "[embedded-ble] startup BLE settings polish skipped name reason={reason} firmware_name={firmware_name:?} pending={} valid={valid}",
                    status.ble_name_pending_restart
                );
                return;
            }
            let mut prefs = inner.prefs.get();
            if prefs.device_ble_name == firmware_name {
                crate::embedded_ble::set_configured_bluetooth_target_name(firmware_name);
                return;
            }
            let previous = prefs.device_ble_name.clone();
            prefs.device_ble_name = firmware_name.to_string();
            match inner.prefs.set(prefs.clone()) {
                Ok(()) => {
                    crate::embedded_ble::set_configured_bluetooth_target_name(firmware_name);
                    log::warn!(
                        "[embedded-ble] startup BLE settings polish updated Type target from {previous:?} to firmware name {firmware_name:?} reason={reason}"
                    );
                    if let Some(app) = inner.app.lock().clone() {
                        let _ = app.emit("prefs:changed", &prefs);
                        let _ = app.emit_to("main", "prefs:changed", &prefs);
                    }
                    // Name changed after first open — re-arm listener on new target.
                    refresh_embedded_ble_listener(inner);
                }
                Err(err) => log::warn!(
                    "[embedded-ble] startup BLE settings polish persist failed previous={previous:?} firmware_name={firmware_name:?} reason={reason}: {err}"
                ),
            }
        }
        Err(err) => log::info!(
            "[embedded-ble] startup BLE settings polish skipped reason={reason} device settings unavailable: {}",
            embedded_ble_log_preview(&err)
        ),
    }
}

fn sync_device_ble_name_from_firmware_settings(inner: &Arc<Inner>, reason: &'static str) -> bool {
    // Auto input-source probe still needs settings before choosing EmbeddedBle;
    // keep a 2s ceiling there. Already-selected EmbeddedBle uses the fast path.
    let timeout = if reason == "auto_input_source_probe" {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(800)
    };
    let synced = match crate::embedded_ble::read_device_settings_status(timeout) {
        Ok(status) => {
            record_embedded_ble_device_settings_power_status(inner, &status, reason);
            let firmware_name = status.ble_name.trim();
            let valid = crate::types::device_ble_name_is_valid(firmware_name);
            if status.ble_name_pending_restart || !valid {
                log::info!(
                    "[embedded-ble] startup BLE name sync skipped reason={reason} firmware_name={firmware_name:?} pending={} valid={valid}",
                    status.ble_name_pending_restart
                );
                false
            } else {
                let mut prefs = inner.prefs.get();
                if prefs.device_ble_name == firmware_name {
                    crate::embedded_ble::set_configured_bluetooth_target_name(firmware_name);
                    false
                } else {
                    let previous = prefs.device_ble_name.clone();
                    prefs.device_ble_name = firmware_name.to_string();
                    match inner.prefs.set(prefs.clone()) {
                        Ok(()) => {
                            crate::embedded_ble::set_configured_bluetooth_target_name(
                                firmware_name,
                            );
                            log::warn!(
                                "[embedded-ble] startup BLE name sync updated Type target from {previous:?} to firmware name {firmware_name:?} reason={reason}"
                            );
                            if let Some(app) = inner.app.lock().clone() {
                                let _ = app.emit("prefs:changed", &prefs);
                                let _ = app.emit_to("main", "prefs:changed", &prefs);
                            }
                            true
                        }
                        Err(err) => {
                            log::warn!(
                                "[embedded-ble] startup BLE name sync persist failed previous={previous:?} firmware_name={firmware_name:?} reason={reason}: {err}"
                            );
                            false
                        }
                    }
                }
            }
        }
        Err(err) => {
            log::info!(
                "[embedded-ble] startup BLE name sync skipped reason={reason} device settings unavailable: {}",
                embedded_ble_log_preview(&err)
            );
            false
        }
    };
    mark_startup_ble_name_sync_done(inner, reason);
    synced
}

fn record_embedded_ble_device_settings_power_status(
    inner: &Arc<Inner>,
    status: &crate::embedded_ble::DeviceSettingsStatus,
    reason: &'static str,
) {
    let usb_powered = status.external_power_present
        || status.usb_power_present
        || status.charging
        || status.charge_full;
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    snapshot.usb_powered = Some(usb_powered);
    log::info!(
        "[embedded-ble] cached device settings power state reason={reason} usb_powered={usb_powered} external_power={} usb_power={} charging={} charge_full={}",
        status.external_power_present,
        status.usb_power_present,
        status.charging,
        status.charge_full
    );
}

fn refresh_embedded_ble_listener(inner: &Arc<Inner>) {
    refresh_embedded_ble_listener_with_options(inner, false, false, false);
}

fn refresh_embedded_ble_listener_after_firmware_ota(inner: &Arc<Inner>) {
    refresh_embedded_ble_listener_with_options(inner, false, true, true);
}

fn refresh_embedded_ble_listener_after_failed_firmware_ota(inner: &Arc<Inner>) {
    refresh_embedded_ble_listener_with_options(inner, false, true, false);
}

fn refresh_embedded_ble_listener_for_device_key_wake(inner: &Arc<Inner>) {
    if embedded_ble_listener_capture_active(inner) {
        log::info!(
            "[embedded-ble] device-key Idle wake joined active notify recovery without replacing the capture"
        );
        return;
    }
    refresh_embedded_ble_listener_with_options(inner, true, false, false);
}

fn refresh_embedded_ble_listener_with_options(
    inner: &Arc<Inner>,
    device_key_idle_wake: bool,
    firmware_ota_recovery: bool,
    firmware_ota_recovery_capsule: bool,
) {
    if !inner
        .embedded_ble_startup_name_sync_done
        .load(Ordering::SeqCst)
    {
        log::info!(
            "[embedded-ble] background listener refresh deferred until startup BLE name sync completes"
        );
        return;
    }
    if !recording_gate::try_admit_arc(
        inner,
        recording_gate::RecordIntent::BackgroundListener,
        "background listener refresh",
    ) {
        return;
    }
    if inner
        .embedded_ble_passive_local_reattach_active
        .load(Ordering::SeqCst)
    {
        // Do not hard-block refresh forever. Owner EC11 double-click / 一键修复 needs
        // a real listener restart even when an earlier startup preflight parked us in
        // passive reattach (HID Unknown while BLE root still shows OK).
        log::warn!(
            "[embedded-ble] background listener refresh clearing passive local reattach so notify can rebuild"
        );
        clear_embedded_ble_passive_local_reattach(
            inner,
            "background listener refresh supersedes passive reattach",
        );
        clear_embedded_ble_pairing_confirmation_hold(
            inner,
            "background listener refresh supersedes passive reattach",
        );
    }
    if let Some(remaining) = embedded_ble_pairing_confirmation_hold_remaining(inner, Instant::now())
    {
        cancel_embedded_ble_listener_capture(inner, "Windows pairing confirmation hold", false);
        log::info!(
            "[embedded-ble] background listener refresh skipped while waiting for Windows pairing confirmation remaining_ms={}",
            remaining.as_millis()
        );
        return;
    }
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    if device_key_idle_wake {
        inner
            .embedded_ble_device_key_wake_generation
            .store(generation, Ordering::SeqCst);
    }
    if firmware_ota_recovery {
        inner
            .embedded_ble_ota_recovery_generation
            .store(generation, Ordering::SeqCst);
        inner.embedded_ble_ota_recovery_capsule_generation.store(
            if firmware_ota_recovery_capsule {
                generation
            } else {
                0
            },
            Ordering::SeqCst,
        );
    } else {
        inner
            .embedded_ble_ota_recovery_capsule_generation
            .store(0, Ordering::SeqCst);
    }
    cancel_embedded_ble_listener_capture(inner, "refresh", false);
    if std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
        .ok()
        .as_deref()
        == Some("1")
    {
        log::info!(
            "[embedded-ble] background listener disabled by LISTENER_TYPE_DISABLE_BACKGROUND_BLE"
        );
        clear_embedded_ble_listener_last_error(inner);
        return;
    }
    let source = inner.prefs.get().dictation_input_source;
    if source != DictationInputSource::EmbeddedBle {
        log::info!("[embedded-ble] background listener disabled (source={source:?})");
        clear_embedded_ble_listener_last_error(inner);
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        embedded_ble_background_listener_loop(inner, generation).await;
    });
}

fn embedded_ble_wake_recovery_snapshot(inner: &Arc<Inner>) -> EmbeddedBleWakeRecoverySnapshot {
    inner.embedded_ble_wake_recovery.lock().clone()
}

fn embedded_ble_background_listener_disabled_by_env() -> bool {
    std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
        .ok()
        .is_some_and(|value| value == "1")
}

fn should_auto_select_embedded_ble_input_source(
    prefs: &crate::types::UserPreferences,
    firmware: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
) -> bool {
    !prefs.dictation_input_source_user_overridden
        && prefs.dictation_input_source != DictationInputSource::EmbeddedBle
        && firmware.connected
}

fn auto_select_embedded_ble_input_source_from_snapshot(
    inner: &Arc<Inner>,
    firmware: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
) -> bool {
    let mut prefs = inner.prefs.get();
    if !should_auto_select_embedded_ble_input_source(&prefs, firmware) {
        log::info!(
            "[embedded-ble] auto input source selection skipped source={:?} connected={} detail={:?}",
            prefs.dictation_input_source,
            firmware.connected,
            firmware.detail.as_deref()
        );
        return false;
    }

    prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    if let Err(err) = inner.prefs.set(prefs.clone()) {
        log::warn!("[embedded-ble] auto input source selection persist failed: {err}");
        return false;
    }

    log::info!(
        "[embedded-ble] auto selected Listener BLE input source hardware={:?} firmware={:?}",
        firmware.hardware_revision,
        firmware.firmware_version
    );
    if let Some(app) = inner.app.lock().clone() {
        let _ = app.emit("prefs:changed", &prefs);
        let _ = app.emit_to("main", "prefs:changed", &prefs);
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(err) = crate::refresh_tray_microphone_menu(&app_for_main) {
                log::warn!(
                    "[tray] refresh after embedded BLE auto input source selection failed: {err}"
                );
            }
        });
    }
    refresh_embedded_ble_listener(inner);
    sync_device_knob_rotation_action_to_firmware(inner, "auto_embedded_ble_input_source");
    true
}

fn record_embedded_ble_firmware_power_snapshot(
    inner: &Arc<Inner>,
    firmware: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    reason: &'static str,
) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    if firmware.usb_powered.is_some() {
        snapshot.usb_powered = firmware.usb_powered;
    }
    if firmware.battery_percent.is_some() {
        snapshot.battery_percent = firmware.battery_percent;
    }
    log::info!(
        "[embedded-ble] cached firmware power state reason={reason} usb_powered={:?} battery_percent={:?} detail={:?}",
        snapshot.usb_powered,
        snapshot.battery_percent,
        firmware.detail.as_deref(),
    );
}

fn record_embedded_ble_reconnect_attempt(inner: &Arc<Inner>, reason: &str) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    snapshot.status = EmbeddedBleWakeRecoveryStatus::Reconnecting;
    snapshot.user_guidance =
        "正在重连 Listener BLE 并恢复音频 notify；如果设备离线，请按 KEY4/唤醒键。".to_string();
    snapshot.reconnect_attempts = snapshot.reconnect_attempts.saturating_add(1);
    snapshot.consecutive_reconnect_failures =
        snapshot.consecutive_reconnect_failures.saturating_add(1);
    snapshot.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Opening;
    snapshot.last_attempt_at = Some(now_rfc3339());
    log::info!(
        "[embedded-ble] wake recovery attempt #{} consecutive_failures={} reason={reason}",
        snapshot.reconnect_attempts,
        snapshot.consecutive_reconnect_failures
    );
}

fn record_embedded_ble_notify_ready(inner: &Arc<Inner>) -> bool {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    let recent_disconnect_reason = snapshot.recent_disconnect_reason.clone();
    let notify_was_recovering = matches!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Lost | EmbeddedBleNotifySubscriptionState::Failed
    );
    let previous_status = snapshot.status.clone();
    let previous_notify_state = snapshot.notify_subscription_state.clone();
    let usb_powered = snapshot.usb_powered;
    let battery_percent = snapshot.battery_percent;
    let reconnect_attempts = snapshot.reconnect_attempts;
    let consecutive_reconnect_failures = snapshot.consecutive_reconnect_failures;
    let recent_disconnect_failure = recent_disconnect_reason
        .as_deref()
        .map(crate::embedded_ble::classify_ble_failure);
    let recent_disconnect_low_power_idle = recent_disconnect_reason
        .as_deref()
        .is_some_and(is_embedded_ble_low_power_idle_candidate);
    let recovered = recent_disconnect_reason.is_some() || notify_was_recovering;
    let emit_recovered_capsule = recovered
        && recent_disconnect_reason
            .as_deref()
            .map(|reason| {
                should_emit_embedded_ble_recovered_capsule_for_reason(
                    reason,
                    usb_powered,
                    reconnect_attempts,
                )
            })
            .unwrap_or(true);
    log::info!(
        "[embedded-ble] notify ready recovery decision recovered={} emit_recovered_capsule={} previous_status={:?} previous_notify_state={:?} notify_was_recovering={} reconnect_attempts={} consecutive_failures={} usb_powered={:?} battery_percent={:?} recent_disconnect_kind={:?} recent_disconnect_automatic_recovery={} recent_disconnect_low_power_idle={} recent_disconnect_reason={}",
        recovered,
        emit_recovered_capsule,
        previous_status,
        previous_notify_state,
        notify_was_recovering,
        reconnect_attempts,
        consecutive_reconnect_failures,
        usb_powered,
        battery_percent,
        recent_disconnect_failure.as_ref().map(|failure| failure.kind),
        recent_disconnect_failure
            .as_ref()
            .is_some_and(|failure| failure.automatic_recovery),
        recent_disconnect_low_power_idle,
        recent_disconnect_reason
            .as_deref()
            .map(embedded_ble_log_preview)
            .unwrap_or_else(|| "-".to_string()),
    );
    snapshot.status = EmbeddedBleWakeRecoveryStatus::Ready;
    snapshot.user_guidance = "Listener BLE 已连接，音频 notify 已订阅。".to_string();
    snapshot.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Subscribed;
    snapshot.last_ready_at = Some(now_rfc3339());
    snapshot.consecutive_reconnect_failures = 0;
    if recovered {
        snapshot.recent_disconnect_reason = None;
    }
    crate::startup_evidence::record_background_notify_ready();
    emit_recovered_capsule
}

fn firmware_mode_for_device_knob_rotation_action(action: DeviceKnobRotationAction) -> &'static str {
    match action {
        DeviceKnobRotationAction::SystemVolume => "system_volume",
        DeviceKnobRotationAction::ScreenBrightness => "screen_brightness",
        DeviceKnobRotationAction::Disabled => "disabled",
    }
}

fn sync_device_knob_rotation_action_to_firmware(inner: &Arc<Inner>, reason: &'static str) {
    let prefs = inner.prefs.get();
    let action = prefs.device_knob_rotation_action;
    let mode = firmware_mode_for_device_knob_rotation_action(action);
    let ec11_fast_recording = prefs.dictation_input_source == DictationInputSource::EmbeddedBle
        && prefs.device_custom_keys.knob.action == DeviceCustomKeyAction::Dictation;
    async_runtime::spawn_blocking(move || {
        let command = format!(
            "DEVICE:SET knob_rotation={mode} e11r={}",
            if ec11_fast_recording { 1 } else { 0 }
        );
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut attempt = 0u32;
        let last_err = loop {
            attempt = attempt.saturating_add(1);
            match crate::embedded_ble::send_device_settings_command_via_active_capture_only(
                &command,
                Duration::from_secs(2),
                "device knob rotation sync",
            ) {
                Ok(()) => {
                    log::info!(
                        "[device-knob] synced knob_rotation and EC11 fast-recording settings via active capture mode={mode} fast_recording={} reason={reason} attempts={attempt}",
                        ec11_fast_recording as u8,
                    );
                    return;
                }
                Err(err) => {
                    if Instant::now() >= deadline {
                        break err;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        };
        log::warn!(
            "[device-knob] knob_rotation/EC11 fast-recording active-capture sync deferred reason={reason} mode={mode} fast_recording={} attempts={attempt} last_error={}",
            ec11_fast_recording as u8,
            last_err
        );
    });
}

fn record_embedded_ble_listener_cancelled(inner: &Arc<Inner>, reason: &str) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    snapshot.status = EmbeddedBleWakeRecoveryStatus::Idle;
    snapshot.user_guidance = "Listener BLE 后台监听已暂停。".to_string();
    snapshot.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
    snapshot.recent_disconnect_reason = Some(reason.to_string());
}

fn record_embedded_ble_recovery_failure(inner: &Arc<Inner>, err: &str) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    let failure = crate::embedded_ble::classify_ble_failure(err);
    snapshot.status = match failure.kind {
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
        | crate::embedded_ble::BleFailureKind::PairedButDisconnected => {
            EmbeddedBleWakeRecoveryStatus::Reconnecting
        }
        crate::embedded_ble::BleFailureKind::DeviceAsleep
        | crate::embedded_ble::BleFailureKind::DeviceMissing => {
            EmbeddedBleWakeRecoveryStatus::NeedsWakeKey
        }
        _ if is_embedded_ble_wake_or_sleep_error(err) => {
            EmbeddedBleWakeRecoveryStatus::NeedsWakeKey
        }
        _ => EmbeddedBleWakeRecoveryStatus::Failed,
    };
    snapshot.user_guidance =
        embedded_ble_wake_guidance_for_error_with_power(err, snapshot.usb_powered);
    snapshot.recent_disconnect_reason = Some(err.to_string());
    snapshot.notify_subscription_state = if is_embedded_ble_cancelled_error(err) {
        EmbeddedBleNotifySubscriptionState::Cancelled
    } else if err.to_ascii_lowercase().contains("notify")
        || err.to_ascii_lowercase().contains("cccd")
        || err.to_ascii_lowercase().contains("subscription")
    {
        EmbeddedBleNotifySubscriptionState::Failed
    } else {
        EmbeddedBleNotifySubscriptionState::Lost
    };
    log::warn!(
        "[embedded-ble] recovery failure recorded kind={:?} automatic_recovery={} retryable={} status={:?} notify_state={:?} usb_powered={:?} battery_percent={:?} guidance={} err={}",
        failure.kind,
        failure.automatic_recovery,
        failure.retryable,
        snapshot.status,
        snapshot.notify_subscription_state,
        snapshot.usb_powered,
        snapshot.battery_percent,
        snapshot.user_guidance,
        embedded_ble_log_preview(err),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddedBleRecoveryCapsuleMessage {
    RestoringAudio,
    WaitingWindowsPairing,
    WaitingManualPairing,
    RebuildingPairing,
    CleaningPairing,
    LocalPairingRestoringAudio,
    WaitingTypePairing,
    AudioRecovered,
}

impl EmbeddedBleRecoveryCapsuleMessage {
    fn text(self) -> &'static str {
        match self {
            Self::RestoringAudio => "正在恢复 Listener 音频",
            Self::WaitingWindowsPairing => "等待 Windows 配对",
            Self::WaitingManualPairing => "等待手动配对",
            Self::RebuildingPairing => "正在重建 Listener 配对",
            Self::CleaningPairing => "正在清理旧配对",
            Self::LocalPairingRestoringAudio => "本机配对完成，恢复音频",
            Self::WaitingTypePairing => "等待本机配对",
            Self::AudioRecovered => "Listener 音频已恢复",
        }
    }
}

fn emit_embedded_ble_recovery_capsule(
    inner: &Arc<Inner>,
    state_label: &str,
    message: EmbeddedBleRecoveryCapsuleMessage,
    idle_after_ms: Option<u64>,
) {
    let state = if state_label == "reconnected" {
        CapsuleState::Done
    } else {
        CapsuleState::Reconnecting
    };
    emit_capsule(inner, state, 0.0, 0, Some(message.text().to_string()), None);
    log::info!("[embedded-ble] recovery capsule state={state_label} emitted=true");
    if let Some(delay_ms) = idle_after_ms {
        schedule_capsule_idle(inner, delay_ms, None);
    }
}

fn embedded_ble_wake_guidance_for_error(err: &str) -> String {
    embedded_ble_wake_guidance_for_error_with_power(err, None)
}

fn embedded_ble_wake_guidance_for_error_with_power(err: &str, usb_powered: Option<bool>) -> String {
    if is_embedded_ble_cancelled_error(err) {
        return "Listener BLE 连接已暂停；请稍后重试。".to_string();
    }
    let failure = crate::embedded_ble::classify_ble_failure(err);
    match failure.kind {
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
            if embedded_ble_usb_power_allows_low_power_idle(usb_powered) =>
        {
            return "Listener BLE 因离线状态断开，正在重连音频 notify；若设备离线，请按 KEY4/唤醒键。".to_string();
        }
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect => {
            return "Listener BLE 正在重连音频 notify；当前未确认处于离线状态场景，若持续失败请重新连接或导出诊断。".to_string();
        }
        crate::embedded_ble::BleFailureKind::PairedButDisconnected => {
            return "Listener BLE 连接临时中断，Type 正在自动重连音频 notify；请保持设备唤醒。"
                .to_string();
        }
        crate::embedded_ble::BleFailureKind::MissingPairing
        | crate::embedded_ble::BleFailureKind::StaleGattService => {
            return "Listener BLE 配对或 GATT 缓存需要恢复。请在 Windows 蓝牙中重新连接或重新配对后重试。".to_string();
        }
        crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded
        | crate::embedded_ble::BleFailureKind::AccessDenied => {
            return "Windows 蓝牙暂时不可用。请打开 Windows 蓝牙设置，确认 Listener 已连接后重试。"
                .to_string();
        }
        crate::embedded_ble::BleFailureKind::DeviceAsleep
        | crate::embedded_ble::BleFailureKind::DeviceMissing => {
            return EMBEDDED_BLE_WAKE_GUIDANCE_MESSAGE.to_string();
        }
        _ => {}
    }
    if is_embedded_ble_wake_or_sleep_error(err) {
        return EMBEDDED_BLE_WAKE_GUIDANCE_MESSAGE.to_string();
    }
    "Listener BLE 暂时不可用。请重新连接设备，或按 KEY4/唤醒键后重试；仍失败可导出诊断。"
        .to_string()
}

fn embedded_ble_recording_control_guidance(err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    if lower.contains("audio control")
        || lower.contains("characteristic")
        || lower.contains("no characteristics")
        || lower.contains("element not found")
        || lower.contains("not found")
    {
        return format!(
            "设备键已触发，但当前固件或 Windows GATT 缓存没有 BLE 录音控制特征：{err}。请刷支持录音控制的新固件，或在 Windows 蓝牙中删除 Listener 后重新配对。"
        );
    }

    format!(
        "设备键录音控制失败：{err}。{}",
        embedded_ble_wake_guidance_for_error(err)
    )
}

fn is_embedded_ble_wake_or_sleep_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("not found")
        || lower.contains("no subscribable")
        || lower.contains("not recover")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("unreachable")
        || lower.contains("device is unreachable")
        || lower.contains("disconnected")
        || lower.contains("asleep")
        || lower.contains("deep sleep")
        || lower.contains("wake key")
        || lower.contains("key4")
        || lower.contains("transport_not_ready")
        || lower.contains("transport not ready")
        || lower.contains("reason=546")
        || lower.contains("reason: 546")
        || lower.contains("reason 546")
        || lower.contains("low-power idle")
        || lower.contains("low power idle")
        || lower.contains("idle disconnect")
}

fn is_embedded_ble_cancelled_error(err: &str) -> bool {
    err.contains("后台监听已取消") || err.to_ascii_lowercase().contains("cancel")
}

fn now_rfc3339() -> String {
    DateTime::<Utc>::from(std::time::SystemTime::now()).to_rfc3339()
}

fn mark_translation_modifier_seen(inner: &Arc<Inner>) {
    let phase = inner.state.lock().phase;
    if matches!(phase, SessionPhase::Starting | SessionPhase::Listening) {
        inner
            .translation_modifier_seen
            .store(true, Ordering::SeqCst);
        log::info!("[coord] translation modifier seen during {phase:?}");
    }
}

fn embedded_ble_listener_generation_is_current(inner: &Arc<Inner>, generation: u64) -> bool {
    !inner.shutdown.load(Ordering::SeqCst)
        && inner
            .embedded_ble_listener_generation
            .load(Ordering::SeqCst)
            == generation
        && recording_gate::admit_arc(inner, recording_gate::RecordIntent::BackgroundListener)
            .is_allow()
        && inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
}

fn take_embedded_ble_device_key_wake_preflight_bypass(inner: &Arc<Inner>, generation: u64) -> bool {
    inner
        .embedded_ble_device_key_wake_generation
        .compare_exchange(generation, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

fn take_embedded_ble_ota_recovery_preflight_bypass(inner: &Arc<Inner>, generation: u64) -> bool {
    inner
        .embedded_ble_ota_recovery_generation
        .compare_exchange(generation, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

fn take_embedded_ble_ota_recovery_capsule(inner: &Arc<Inner>, generation: u64) -> bool {
    inner
        .embedded_ble_ota_recovery_capsule_generation
        .compare_exchange(generation, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

async fn embedded_ble_background_listener_loop(inner: Arc<Inner>, generation: u64) {
    log::info!("[embedded-ble] background listener started generation={generation}");
    crate::startup_evidence::record_startup_stage("background_listener_started");
    let mut retry_delay = EMBEDDED_BLE_RETRY_BASE_DELAY;
    let mut last_stale_cleanup_at: Option<Instant> = None;
    let mut startup_pairing_preflight_checked = false;
    let mut firmware_ota_recovery = false;
    loop {
        if !embedded_ble_listener_generation_is_current(&inner, generation) {
            break;
        }
        if let Some(remaining) =
            embedded_ble_pairing_confirmation_hold_remaining(&inner, Instant::now())
        {
            log::info!(
                "[embedded-ble] background listener waiting for Windows pairing confirmation generation={generation} remaining_ms={}",
                remaining.as_millis()
            );
            tokio::time::sleep(remaining.min(EMBEDDED_BLE_RETRY_LONG_DELAY)).await;
            continue;
        }
        if !startup_pairing_preflight_checked {
            startup_pairing_preflight_checked = true;
            if take_embedded_ble_device_key_wake_preflight_bypass(&inner, generation) {
                log::info!(
                    "[embedded-ble] device-key Idle wake bypassed slow manual-delete preflight generation={generation}; physical HID input proves the local pairing remains installed"
                );
            } else if take_embedded_ble_ota_recovery_preflight_bypass(&inner, generation) {
                firmware_ota_recovery = true;
                log::info!(
                    "[embedded-ble] confirmed firmware OTA recovery bypassed repeated Windows HID pairing preflight generation={generation}; the pre-OTA active link and post-OTA service probe prove this trusted local path"
                );
            } else if maybe_hold_embedded_ble_startup_without_current_native_pairing(&inner).await {
                continue;
            }
            // The PnP preflight runs in a blocking task. A device-key wake may have
            // superseded this loop while that task was still enumerating Windows.
            if !embedded_ble_listener_generation_is_current(&inner, generation) {
                break;
            }
        }

        let cancel_capture = install_embedded_ble_listener_cancel(&inner, generation);
        let leave_notify_cccd_enabled_on_cancel =
            embedded_ble_listener_cccd_handoff_flag(&inner, &cancel_capture);
        record_embedded_ble_reconnect_attempt(&inner, "background_listener_loop");
        match submit_embedded_audio_ble_stream_background(
            &inner,
            Arc::clone(&cancel_capture),
            leave_notify_cccd_enabled_on_cancel,
        )
        .await
        {
            Ok(result) => {
                log::info!(
                    "[embedded-ble] background session completed pcm_bytes={} missing_packets={}",
                    result.reconstructed_pcm_bytes,
                    result.stats.missing_packet_count
                );
                clear_embedded_ble_listener_last_error(&inner);
                retry_delay = EMBEDDED_BLE_RETRY_BASE_DELAY;
            }
            Err(err) => {
                clear_embedded_ble_listener_cancel(&inner, &cancel_capture);
                if inner.shutdown.load(Ordering::SeqCst)
                    || inner
                        .embedded_ble_listener_generation
                        .load(Ordering::SeqCst)
                        != generation
                    || recording_gate::admit_arc(
                        &inner,
                        recording_gate::RecordIntent::BackgroundListener,
                    )
                    .is_deny()
                    || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
                {
                    break;
                }
                if crate::embedded_ble::is_background_listener_deferred_for_ota_error(&err) {
                    clear_embedded_ble_listener_last_error(&inner);
                    retry_delay = EMBEDDED_BLE_RETRY_OTA_DEFER_DELAY;
                    log::info!(
                        "[embedded-ble] background listener deferred while firmware OTA is active; retrying in {} ms",
                        retry_delay.as_millis()
                    );
                    tokio::time::sleep(retry_delay).await;
                    continue;
                }
                if is_embedded_ble_idle_timeout_error(&err) {
                    clear_embedded_ble_listener_last_error(&inner);
                    retry_delay = next_embedded_ble_background_retry_delay(&err, retry_delay);
                } else {
                    record_embedded_ble_listener_last_error(&inner, &err);
                    record_embedded_ble_recovery_failure(&inner, &err);
                    if !firmware_ota_recovery
                        && maybe_hold_embedded_ble_after_lost_native_pairing(&inner, &err).await
                    {
                        log::info!(
                            "[embedded-ble] background listener stopped after current native Windows pairing disappeared during link recovery"
                        );
                        break;
                    }
                    let stale_cleanup_outcome = if firmware_ota_recovery {
                        log::warn!(
                            "[embedded-ble] OTA recovery generation={generation} preserving Windows pairing and firmware bond after transient reconnect error; retrying bonded GATT without stale-pair cleanup err={}",
                            embedded_ble_log_preview(&err),
                        );
                        EmbeddedBleStalePairingCleanupOutcome::RetrySoon
                    } else {
                        maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
                            &inner,
                            &err,
                            &mut last_stale_cleanup_at,
                        )
                        .await
                    };
                    if inner
                        .embedded_ble_passive_local_reattach_active
                        .load(Ordering::SeqCst)
                    {
                        log::info!(
                            "[embedded-ble] background listener stopped while passive local Windows reattach monitor owns recovery"
                        );
                        break;
                    }
                    if stale_cleanup_outcome == EmbeddedBleStalePairingCleanupOutcome::Skipped
                        && should_emit_embedded_ble_background_recovery_capsule(&inner, &err)
                    {
                        emit_embedded_ble_recovery_capsule(
                            &inner,
                            "reconnecting",
                            EmbeddedBleRecoveryCapsuleMessage::RestoringAudio,
                            Some(1800),
                        );
                    } else if is_embedded_ble_automatic_recovery_error(&err) {
                        log::info!(
                            "[embedded-ble] background recovery capsule suppressed by pairing/recovery decision outcome={:?}",
                            stale_cleanup_outcome
                        );
                    }
                    let stale_cleanup_retry_soon =
                        stale_cleanup_outcome == EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
                    let stale_cleanup_retry_immediate = stale_cleanup_outcome
                        == EmbeddedBleStalePairingCleanupOutcome::RetryImmediate;
                    let stale_cleanup_holding = stale_cleanup_outcome
                        == EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
                    let stale_cleanup_skipped =
                        stale_cleanup_outcome == EmbeddedBleStalePairingCleanupOutcome::Skipped;
                    let stale_cleanup_backoff = stale_cleanup_holding
                        || (stale_cleanup_skipped
                            && should_throttle_embedded_ble_background_stale_pairing_cleanup(
                                &err,
                                &embedded_ble_wake_recovery_snapshot(&inner),
                                last_stale_cleanup_at,
                                Instant::now(),
                            ));
                    if stale_cleanup_backoff && stale_cleanup_skipped {
                        log::warn!(
                            "[embedded-ble] background stale pairing cleanup cooldown active; using long retry backoff err={}",
                            embedded_ble_log_preview(&err),
                        );
                    }
                    retry_delay = if stale_cleanup_retry_immediate {
                        Duration::ZERO
                    } else if stale_cleanup_retry_soon {
                        EMBEDDED_BLE_RETRY_LONG_DELAY
                    } else if stale_cleanup_backoff {
                        EMBEDDED_BLE_RETRY_OFFLINE_DELAY
                    } else {
                        next_embedded_ble_background_retry_delay(&err, retry_delay)
                    };
                }
                log::warn!(
                    "[embedded-ble] background listen retrying in {} ms after: {err}",
                    retry_delay.as_millis()
                );
                tokio::time::sleep(retry_delay).await;
                continue;
            }
        }
        clear_embedded_ble_listener_cancel(&inner, &cancel_capture);
    }
    log::info!("[embedded-ble] background listener stopped generation={generation}");
}

async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
    inner: &Arc<Inner>,
    err: &str,
    last_cleanup_at: &mut Option<Instant>,
) -> EmbeddedBleStalePairingCleanupOutcome {
    let snapshot = embedded_ble_wake_recovery_snapshot(inner);
    let now = Instant::now();
    let recovery_pairing_probe = maybe_probe_embedded_ble_recovery_pairing_advertisement(
        inner,
        err,
        &snapshot,
        *last_cleanup_at,
        now,
    )
    .await;
    if !embedded_ble_recovery_error_still_current(inner, err, "after_recovery_advertisement_probe")
    {
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    let recovery_pairing_window_visible = recovery_pairing_probe.visible;
    let pairing_confirmation_hold_active =
        embedded_ble_pairing_confirmation_hold_remaining(inner, now).is_some();
    let hardware_ec11_recovery_notice = embedded_ble_hardware_ec11_recovery_notice_observed(err);
    let active_capture_type_recovery =
        recovery_pairing_advertisement_already_observed_during_active_capture(err);
    let type_observed_recovery_advertisement = !pairing_confirmation_hold_active
        && (recovery_pairing_advertisement_already_observed_during_notify_open(err)
            || (hardware_ec11_recovery_notice && recovery_pairing_probe.visible));
    let stale_cleanup_candidate = should_attempt_embedded_ble_background_stale_pairing_cleanup(
        err,
        &snapshot,
        *last_cleanup_at,
        now,
    );
    let direct_gatt_instability_recovery = recovery_pairing_window_visible
        && should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            err,
            &snapshot,
            *last_cleanup_at,
            now,
        );
    let visible_recovery_allows_cleanup = recovery_pairing_probe_allows_immediate_stale_cleanup(
        err,
        &recovery_pairing_probe,
        direct_gatt_instability_recovery,
    );
    let recovery_advertisement_allows_cleanup =
        recovery_pairing_advertisement_allows_immediate_stale_cleanup(err);
    let should_query_pairing_preflight = !type_observed_recovery_advertisement
        && (recovery_pairing_window_visible
            || stale_cleanup_candidate
            || visible_recovery_allows_cleanup
            || recovery_advertisement_allows_cleanup);
    let usb_ble_name_synced = should_query_pairing_preflight
        && sync_device_ble_name_from_firmware_settings(
            inner,
            "background_stale_pairing_usb_name_probe",
        );
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let pairing_before_cleanup = if should_query_pairing_preflight {
        let expected_ble_name = expected_ble_name.clone();
        let pairing = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::query_listener_pairing(Some(expected_ble_name.as_str()))
        })
        .await;
        match pairing {
            Ok(pairing) => {
                log::info!(
                    "[embedded-ble] background stale pairing cleanup preflight Windows pairing status={:?} matched={} already_paired={} failed={} open_settings={} direct_gatt_instability_recovery={direct_gatt_instability_recovery}",
                    pairing.status,
                    pairing.matched_devices,
                    pairing.already_paired_devices,
                    pairing.failed_devices,
                    pairing.open_bluetooth_settings,
                );
                Some(pairing)
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] background stale pairing cleanup preflight pairing query failed; continuing conservative recovery: {err}"
                );
                None
            }
        }
    } else {
        None
    };
    let native_windows_hid_pairing_visible = if should_query_pairing_preflight {
        match async_runtime::spawn_blocking(|| {
            crate::embedded_ble::native_windows_hid_pairing_addresses()
        })
        .await
        {
            Ok(Ok(addresses)) if !addresses.is_empty() => {
                log::info!(
                    "[embedded-ble] background stale pairing cleanup found native Windows HID pairing evidence addresses={addresses:?}; evaluating whether it is current or stale"
                );
                true
            }
            Ok(Ok(_)) => false,
            Ok(Err(err)) => {
                log::warn!(
                    "[embedded-ble] background native Windows HID pairing evidence unavailable; preserving manual-delete safety: {err}"
                );
                false
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] background native Windows HID pairing evidence task failed; preserving manual-delete safety: {err}"
                );
                false
            }
        }
    } else {
        false
    };
    let native_windows_hid_pairing_blocks_pairasync =
        native_windows_hid_pairing_blocks_type_pairasync(
            native_windows_hid_pairing_visible,
            &recovery_pairing_probe,
            pairing_before_cleanup.as_ref(),
        );
    if native_windows_hid_pairing_visible && !native_windows_hid_pairing_blocks_pairasync {
        log::warn!(
            "[embedded-ble] recovery advertisement proves native Windows HID evidence is stale; allowing bounded Type PairAsync cleanup"
        );
    }
    if !embedded_ble_recovery_error_still_current(inner, err, "after_pairing_preflight") {
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    // Seeing a recovery advertisement after a link loss does not prove that
    // Type initiated recovery. Windows manual delete creates the same signal.
    let stale_native_hid_recovery =
        native_windows_hid_pairing_visible && !native_windows_hid_pairing_blocks_pairasync;
    let type_controlled_recovery =
        type_observed_recovery_advertisement || stale_native_hid_recovery;
    let ec11_type_controlled_recovery = type_controlled_recovery && hardware_ec11_recovery_notice;
    let recovery_advertisement_type_owned_cleanup = type_controlled_recovery;
    let noisy_cccd_stale_cache_type_owned_cleanup =
        pairing_before_cleanup.as_ref().is_some_and(|pairing| {
            noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
                err,
                stale_cleanup_candidate,
                pairing,
            )
        });
    let type_owned_stale_cache_cleanup =
        recovery_advertisement_type_owned_cleanup || noisy_cccd_stale_cache_type_owned_cleanup;
    let manual_unpair_hold = !native_windows_hid_pairing_blocks_pairasync
        && !usb_ble_name_synced
        && pairing_before_cleanup.as_ref().is_some_and(|pairing| {
            should_hold_embedded_ble_background_recovery_after_manual_unpair(
                pairing,
                direct_gatt_instability_recovery,
                type_owned_stale_cache_cleanup,
            )
        });
    let local_stale_cache_recovery_allows_cleanup = recovery_pairing_window_visible
        && pairing_before_cleanup.as_ref().is_some_and(|pairing| {
            !manual_unpair_hold
                && (pairing.already_paired_devices > 0
                    || pairing.matched_devices > 0
                    || pairing.failed_devices > 0)
        });
    let automatic_cleanup_allowed = !native_windows_hid_pairing_blocks_pairasync
        && embedded_ble_background_pairasync_is_authorized(
            manual_unpair_hold,
            type_observed_recovery_advertisement,
            visible_recovery_allows_cleanup,
            stale_cleanup_candidate,
            noisy_cccd_stale_cache_type_owned_cleanup,
            local_stale_cache_recovery_allows_cleanup,
            usb_ble_name_synced,
        );
    let mut device_control_recovery =
        crate::device_control_platform::BackgroundPairingRecovery::begin(
            type_controlled_recovery,
            manual_unpair_hold,
            automatic_cleanup_allowed,
        );
    let device_control_decision = device_control_recovery.decision();
    log::info!(
        "[embedded-ble] device-control recovery transaction execute={} replayed={} result={:?} error={:?} type_controlled={} manual_unpair={} authorized={}",
        device_control_decision.execute,
        device_control_decision.replayed,
        device_control_decision.result,
        device_control_decision.error,
        type_controlled_recovery,
        manual_unpair_hold,
        automatic_cleanup_allowed,
    );
    if noisy_cccd_stale_cache_type_owned_cleanup {
        log::warn!(
            "[embedded-ble] noisy CCCD stale Windows cache evidence allows Type automatic PairAsync recovery even though recovery advertisement scan may have missed err={}",
            embedded_ble_log_preview(err),
        );
    }
    if recovery_pairing_probe.has_random_identity {
        if visible_recovery_allows_cleanup || local_stale_cache_recovery_allows_cleanup {
            log::warn!(
                "[embedded-ble] random-identity recovery advertisement visible with stale Windows cache evidence; entering Windows pairing cleanup err={}",
                embedded_ble_log_preview(err),
            );
        } else {
            log::warn!(
                "[embedded-ble] random-identity recovery advertisement visible; waiting for user-controlled Windows pairing instead of retrying stale GATT address err={}",
                embedded_ble_log_preview(err),
            );
        }
    }
    if recovery_pairing_window_visible
        && !direct_gatt_instability_recovery
        && !automatic_cleanup_allowed
        && !manual_unpair_hold
    {
        if native_windows_hid_pairing_blocks_pairasync {
            log::info!(
                "[embedded-ble] native Windows HID pairing remains installed; retrying direct GATT without pairing cleanup"
            );
            return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
        }
        *last_cleanup_at = Some(now);
        log::warn!(
            "[embedded-ble] recovery pairing advertisement visible from hardware/user action; holding background listener without automatic PairAsync err={}",
            embedded_ble_log_preview(err),
        );
        let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
            inner,
            EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON,
        );
        start_embedded_ble_pairing_confirmation_watch(
            inner,
            expected_ble_name.clone(),
            hold_generation,
            EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON,
        );
        {
            let mut wake = inner.embedded_ble_wake_recovery.lock();
            wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
            wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
            wake.recent_disconnect_reason = Some(format!(
                "Listener recovery advertisement is visible; Type is waiting for explicit Windows pairing target={expected_ble_name}"
            ));
            wake.user_guidance = format!(
                "Listener 已进入重新配对状态。Type 不会自动抢回连接；如需继续使用这台电脑，请在 Windows 蓝牙里手动添加 {expected_ble_name}。"
            );
        }
        emit_embedded_ble_recovery_capsule(
            inner,
            "reconnecting",
            EmbeddedBleRecoveryCapsuleMessage::WaitingWindowsPairing,
            Some(4200),
        );
        return EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
    }
    if manual_unpair_hold {
        *last_cleanup_at = Some(now);
        hold_embedded_ble_for_manual_windows_unpair(inner, &expected_ble_name).await;
        return EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
    }
    if !automatic_cleanup_allowed {
        if recovery_pairing_window_visible {
            log::info!(
                "[embedded-ble] recovery pairing advertisement visible after transient link loss; retrying direct audio GATT before clearing Windows pairing cache err={}",
                embedded_ble_log_preview(err),
            );
            return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
        }
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    if !device_control_recovery.may_execute() {
        log::error!(
            "[embedded-ble] device-control rejected an otherwise authorized PairAsync recovery; refusing to bypass the transaction result={:?} error={:?}",
            device_control_decision.result,
            device_control_decision.error,
        );
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    if crate::embedded_ble::listener_pairing_maintenance_active() {
        log::warn!(
            "[embedded-ble] background stale pairing cleanup deferred because Listener pairing/cache maintenance is already active"
        );
        return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
    }
    *last_cleanup_at = Some(now);

    let recovery_reason: &'static str = if direct_gatt_instability_recovery {
        EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON
    } else {
        EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON
    };
    let Some(_pairing_recovery_guard) =
        try_begin_embedded_ble_pairing_recovery(inner, recovery_reason)
    else {
        return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
    };

    log::warn!(
        "[embedded-ble] background stale pairing cleanup triggered reconnect_attempts={} consecutive_failures={} notify_state={:?} usb_powered={:?} recovery_pairing_window_visible={} direct_gatt_instability_recovery={} err={}",
        snapshot.reconnect_attempts,
        snapshot.consecutive_reconnect_failures,
        snapshot.notify_subscription_state,
        snapshot.usb_powered,
        recovery_pairing_window_visible,
        direct_gatt_instability_recovery,
        embedded_ble_log_preview(err),
    );
    if ec11_type_controlled_recovery {
        log::info!(
            "[embedded-ble] EC11 Type-controlled recovery suppresses pairing-progress capsule until firmware Type-ready terminal confirmation"
        );
    } else {
        emit_embedded_ble_recovery_capsule(
            inner,
            "reconnecting",
            if direct_gatt_instability_recovery {
                EmbeddedBleRecoveryCapsuleMessage::RebuildingPairing
            } else {
                EmbeddedBleRecoveryCapsuleMessage::CleaningPairing
            },
            Some(2600),
        );
    }
    if direct_gatt_instability_recovery {
        let recovery = async_runtime::spawn_blocking(|| {
            crate::embedded_ble::send_recording_control_recovery(Duration::from_secs(5))
        })
        .await;
        match recovery {
            Ok(Ok(())) => log::warn!(
                "[embedded-ble] background direct GATT instability sent Listener recovery pairing command"
            ),
            Ok(Err(err)) => log::warn!(
                "[embedded-ble] background direct GATT instability recovery command failed before Windows pairing cleanup: {}",
                embedded_ble_log_preview(&err),
            ),
            Err(err) => log::warn!(
                "[embedded-ble] background direct GATT instability recovery command task failed: {err}"
            ),
        }
        tokio::time::sleep(EMBEDDED_BLE_TYPE_RECOVERY_PAIRING_SETTLE).await;
    }
    if !embedded_ble_recovery_error_still_current(inner, err, "before_pairing_cleanup") {
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    let hold_generation =
        hold_embedded_ble_listener_for_pairing_confirmation(inner, recovery_reason);

    let pairing_expected_name = expected_ble_name.clone();
    let observed_recovery_addresses = if type_controlled_recovery {
        recovery_pairing_addresses_for_cleanup(err, &recovery_pairing_probe)
    } else {
        Vec::new()
    };
    let pairing_recovery_addresses = if type_controlled_recovery {
        recovery_pairing_addresses_for_direct_pairing(err, &recovery_pairing_probe)
    } else {
        Vec::new()
    };
    let pairing = async_runtime::spawn_blocking(move || {
        if type_controlled_recovery {
            let cleanup_names = vec![pairing_expected_name.clone()];
            let direct_pairing_uses_fresh_recovery_address = !pairing_recovery_addresses.is_empty();
            let unpair = if direct_pairing_uses_fresh_recovery_address {
                crate::embedded_ble::unpair_listener_pairing_for_known_addresses(
                    &cleanup_names,
                    &observed_recovery_addresses,
                )
            } else {
                crate::embedded_ble::unpair_listener_devices_for_known_addresses(
                    &cleanup_names,
                    &observed_recovery_addresses,
                )
            };
            log::warn!(
                "[embedded-ble] background Type controlled-recovery known-address cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={} fresh_direct_pairing={} active_capture={active_capture_type_recovery} stale_native_hid_recovery={stale_native_hid_recovery} cleanup_addresses={observed_recovery_addresses:?} pairing_addresses={pairing_recovery_addresses:?}",
                unpair.status,
                unpair.matched_devices,
                unpair.unpaired_devices,
                unpair.already_unpaired_devices,
                unpair.failed_devices,
                unpair.needs_user_action,
                direct_pairing_uses_fresh_recovery_address,
            );
            let pairing = crate::embedded_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
                Some(pairing_expected_name.as_str()),
                &pairing_recovery_addresses,
            );
            if !direct_pairing_uses_fresh_recovery_address
                || embedded_ble_pairing_prompt_ready(&pairing)
            {
                return pairing;
            }

            log::warn!(
                "[embedded-ble] fresh-address direct PairAsync did not complete after pairing-only cleanup; clearing only the exact BTHPORT cache before one final direct PairAsync status={:?} matched={} prompted={} failed={}",
                pairing.status,
                pairing.matched_devices,
                pairing.prompted_devices,
                pairing.failed_devices,
            );
            let fallback_unpair = crate::embedded_ble::clear_listener_bthport_cache_for_known_addresses(
                &cleanup_names,
                &observed_recovery_addresses,
            );
            log::warn!(
                "[embedded-ble] fresh-address direct PairAsync exact BTHPORT cache cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={}",
                fallback_unpair.status,
                fallback_unpair.matched_devices,
                fallback_unpair.unpaired_devices,
                fallback_unpair.already_unpaired_devices,
                fallback_unpair.failed_devices,
                fallback_unpair.needs_user_action,
            );
            crate::embedded_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
                Some(pairing_expected_name.as_str()),
                &pairing_recovery_addresses,
            )
        } else {
            crate::embedded_ble::prompt_listener_pairing_after_type_recovery(Some(
                pairing_expected_name.as_str(),
            ))
        }
    })
    .await;
    match pairing {
        Ok(pairing) => {
            log::warn!(
                "[embedded-ble] background Type recovery PairAsync result status={:?} matched={} prompted={} already_paired={} failed={} open_settings={}",
                pairing.status,
                pairing.matched_devices,
                pairing.prompted_devices,
                pairing.already_paired_devices,
                pairing.failed_devices,
                pairing.open_bluetooth_settings,
            );

            if embedded_ble_pairing_prompt_ready(&pairing) {
                let terminal = device_control_recovery.complete_pairing();
                log::info!(
                    "[embedded-ble] device-control recovery PairAsync terminal result={:?} error={:?}",
                    terminal.result,
                    terminal.error,
                );
                arm_embedded_ble_type_pairasync_startup_guard(inner);
                if type_controlled_recovery {
                    log::info!(
                        "[embedded-ble] background Type controlled-recovery PairAsync paired; reopening notify immediately for GATT/notify validation active_capture={active_capture_type_recovery} stale_native_hid_recovery={stale_native_hid_recovery}"
                    );
                    resume_embedded_ble_listener_after_pairing_recovery(
                        inner,
                        "background Type recovery PairAsync paired; reopening notify for GATT validation",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        !ec11_type_controlled_recovery,
                    );
                    return EmbeddedBleStalePairingCleanupOutcome::RetryImmediate;
                }

                if embedded_ble_pairing_recovery_link_reachable(
                    inner,
                    "background Type recovery PairAsync paired link check",
                )
                .await
                {
                    resume_embedded_ble_listener_after_pairing_recovery(
                        inner,
                        "background Type recovery PairAsync paired and link reachable",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        true,
                    );
                    return EmbeddedBleStalePairingCleanupOutcome::RetryImmediate;
                }

                start_embedded_ble_pairing_confirmation_watch(
                    inner,
                    expected_ble_name.clone(),
                    hold_generation,
                    recovery_reason,
                );
                {
                    let mut wake = inner.embedded_ble_wake_recovery.lock();
                    wake.status = EmbeddedBleWakeRecoveryStatus::Reconnecting;
                    wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Opening;
                    wake.recent_disconnect_reason = Some(format!(
                        "background Type recovery PairAsync completed; waiting for Listener GATT/notify rebuild; previous error: {}",
                        embedded_ble_log_preview(err),
                    ));
                    wake.user_guidance =
                        "Type 已完成本机自动配对，正在等待 Windows BLE GATT/notify 恢复。"
                            .to_string();
                }
                emit_embedded_ble_recovery_capsule(
                    inner,
                    "reconnecting",
                    EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                    Some(2600),
                );
                return EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
            }

            let terminal = device_control_recovery
                .fail_without_reclaim(denzic_device_control_v1_core::ErrorCategory::Ownership);
            log::info!(
                "[embedded-ble] device-control recovery terminal result={:?} error={:?}; holding without reclaim",
                terminal.result,
                terminal.error,
            );

            start_embedded_ble_pairing_confirmation_watch(
                inner,
                expected_ble_name.clone(),
                hold_generation,
                recovery_reason,
            );
            {
                let mut wake = inner.embedded_ble_wake_recovery.lock();
                wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
                wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
                wake.recent_disconnect_reason = Some(format!(
                    "background Type recovery PairAsync did not complete status={:?} direct_gatt_instability_recovery={}; if another host paired first, this Type instance must stop instead of stealing it back; previous error: {}",
                    pairing.status,
                    direct_gatt_instability_recovery,
                    embedded_ble_log_preview(err),
                ));
                wake.user_guidance = format!(
                    "Type 没有完成本机自动配对 {expected_ble_name}。如果你已在另一台电脑用 Windows 弹窗连上，这是预期；否则请保持设备可配对后再重试。"
                );
            }
            emit_embedded_ble_recovery_capsule(
                inner,
                "reconnecting",
                EmbeddedBleRecoveryCapsuleMessage::WaitingTypePairing,
                Some(4200),
            );
            EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation
        }
        Err(err) => {
            let terminal = device_control_recovery
                .fail_without_reclaim(denzic_device_control_v1_core::ErrorCategory::Host);
            log::info!(
                "[embedded-ble] device-control recovery task terminal result={:?} error={:?}",
                terminal.result,
                terminal.error,
            );
            log::warn!("[embedded-ble] background Type recovery PairAsync task failed: {err}");
            clear_embedded_ble_pairing_confirmation_hold(
                inner,
                "background Type recovery PairAsync task failed",
            );
            EmbeddedBleStalePairingCleanupOutcome::RetrySoon
        }
    }
}

async fn maybe_probe_embedded_ble_recovery_pairing_advertisement(
    inner: &Arc<Inner>,
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
    let should_probe = should_probe_embedded_ble_recovery_pairing_advertisement(
        err,
        snapshot,
        last_cleanup_at,
        now,
    );
    if !should_probe {
        return crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default();
    }
    if recovery_pairing_advertisement_already_observed_during_notify_open(err) {
        log::info!(
            "[embedded-ble] notify open already observed Listener recovery advertising; skipping duplicate recovery advertisement scan"
        );
        return crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
            visible: true,
            // The notify-open path proves that matching recovery advertising is present but does
            // not carry the address type. Treat it as random to retain the conservative
            // multi-host failure behavior; pairing preflight still decides ownership.
            has_random_identity: true,
            addresses: listener_recovery_addresses_from_error(err),
        };
    }
    if embedded_ble_hardware_ec11_recovery_notice_observed(err) {
        log::info!(
            "[embedded-ble] EC11 hardware recovery notice arrived before pairing reset; scanning recovery advertising before any GATT failure timeout"
        );
    }

    let expected_ble_name = inner.prefs.get().device_ble_name;
    match async_runtime::spawn_blocking(move || {
        crate::embedded_ble::listener_recovery_pairing_advertisement_probe(
            Some(&expected_ble_name),
            EMBEDDED_BLE_RECOVERY_PAIRING_ADV_SCAN_TIMEOUT,
        )
    })
    .await
    {
        Ok(probe) => probe,
        Err(err) => {
            log::warn!("[embedded-ble] recovery pairing advertisement probe task failed: {err}");
            crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default()
        }
    }
}

fn should_probe_embedded_ble_recovery_pairing_advertisement(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    if embedded_ble_hardware_ec11_recovery_notice_observed(err) {
        return true;
    }
    if last_cleanup_at.is_some_and(|last| {
        now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
    }) {
        return false;
    }
    if snapshot.reconnect_attempts == 0 {
        return false;
    }
    if !matches!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Failed
            | EmbeddedBleNotifySubscriptionState::Opening
            | EmbeddedBleNotifySubscriptionState::Lost
            | EmbeddedBleNotifySubscriptionState::Unknown
    ) {
        return false;
    }
    let failure = crate::embedded_ble::classify_ble_failure(err);
    if matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::DeviceMissing
    ) || (matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::CccdProtocolError
    ) && is_embedded_ble_noisy_cccd_failure(err))
    {
        return true;
    }

    should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
        err,
        snapshot,
        last_cleanup_at,
        now,
    )
}

fn recovery_pairing_advertisement_allows_immediate_stale_cleanup(err: &str) -> bool {
    let failure = crate::embedded_ble::classify_ble_failure(err);
    matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::DeviceMissing
            | crate::embedded_ble::BleFailureKind::StaleGattService
    ) || (matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::CccdProtocolError
    ) && is_embedded_ble_noisy_cccd_failure(err))
}

fn recovery_pairing_advertisement_already_observed_during_notify_open(err: &str) -> bool {
    err.contains("Listener recovery Swift Pair advertisement visible for notify CCCD address")
        || err.contains("Listener recovery Swift Pair advertisement visible for persisted address")
        || recovery_pairing_advertisement_already_observed_during_active_capture(err)
}

fn embedded_ble_hardware_ec11_recovery_notice_observed(err: &str) -> bool {
    err.contains(EMBEDDED_BLE_EC11_HARDWARE_RECOVERY_NOTICE)
}

fn recovery_pairing_advertisement_already_observed_during_active_capture(err: &str) -> bool {
    err.contains("Listener recovery Swift Pair advertisement visible for active capture address")
}

fn listener_recovery_addresses_from_error(err: &str) -> Vec<u64> {
    let mut addresses = Vec::new();
    for token in err.split(|ch: char| !ch.is_ascii_hexdigit()) {
        if token.len() != 12 {
            continue;
        }
        if let Ok(address) = u64::from_str_radix(token, 16) {
            if address != 0 && !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    addresses
}

fn recovery_pairing_addresses_for_cleanup(
    err: &str,
    probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> Vec<u64> {
    let mut addresses = listener_recovery_addresses_from_error(err);
    for address in probe.addresses.iter().copied() {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

fn recovery_pairing_addresses_for_direct_pairing(
    err: &str,
    probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> Vec<u64> {
    if !probe.addresses.is_empty() {
        return probe.addresses.clone();
    }
    listener_recovery_addresses_from_error(err)
}

fn recovery_pairing_probe_allows_immediate_stale_cleanup(
    err: &str,
    probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
    direct_gatt_instability_recovery: bool,
) -> bool {
    probe.visible
        && direct_gatt_instability_recovery
        && (probe.has_random_identity
            || recovery_pairing_advertisement_allows_immediate_stale_cleanup(err))
}

fn noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
    err: &str,
    stale_cleanup_candidate: bool,
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    stale_cleanup_candidate
        && is_embedded_ble_noisy_cccd_failure(err)
        && matches!(
            pairing.status,
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
        )
        && pairing.open_bluetooth_settings
        && pairing.matched_devices > 0
        && pairing.failed_devices > 0
}

fn native_windows_hid_pairing_blocks_type_pairasync(
    native_windows_hid_pairing_visible: bool,
    recovery_pairing_probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
) -> bool {
    if !native_windows_hid_pairing_visible {
        return false;
    }

    // A random recovery advertisement plus a Windows record that cannot be
    // reopened is a stale local pairing key, not a live HID takeover. The
    // physical double-click is allowed to rebuild that key through PairAsync.
    let stale_native_hid_evidence = recovery_pairing_probe.visible
        && recovery_pairing_probe.has_random_identity
        && pairing.is_some_and(|pairing| {
            pairing.already_paired_devices == 0
                && pairing.matched_devices > 0
                && pairing.failed_devices > 0
                && matches!(
                    pairing.status,
                    crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
                )
                && pairing.open_bluetooth_settings
        });
    !stale_native_hid_evidence
}

fn should_hold_embedded_ble_background_recovery_after_manual_unpair(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
    direct_gatt_instability_recovery: bool,
    recovery_advertisement_type_owned_cleanup: bool,
) -> bool {
    if direct_gatt_instability_recovery || recovery_advertisement_type_owned_cleanup {
        return false;
    }
    if pairing.already_paired_devices > 0 {
        return false;
    }
    if is_explicit_manual_windows_delete_pairing_state(pairing) {
        return true;
    }
    if pairing.matched_devices > 0 || pairing.failed_devices > 0 {
        return false;
    }
    matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::NotFound
            | crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
    )
}

fn is_explicit_manual_windows_delete_pairing_state(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    pairing.already_paired_devices == 0
        && matches!(
            pairing.status,
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
        )
        && pairing.open_bluetooth_settings
        && pairing.failed_devices > 0
}

async fn hold_embedded_ble_for_manual_windows_unpair(inner: &Arc<Inner>, expected_ble_name: &str) {
    log::warn!(
        "[embedded-ble] background stale pairing cleanup suppressed automatic PairAsync because Windows no longer reports a paired Listener; treating this as user/manual pairing removal target={expected_ble_name:?}"
    );
    let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
        inner,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
    let firmware_recovery = async_runtime::spawn_blocking(|| {
        crate::embedded_ble::send_recording_control_manual_pairing(Duration::from_secs(3))
    })
    .await;
    match firmware_recovery {
        Ok(Ok(())) => {
            log::warn!(
                "[embedded-ble] manual Windows unpair sent Listener manual-pairing recovery cue without Windows PairAsync"
            );
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
        Ok(Err(err)) => log::warn!(
            "[embedded-ble] manual Windows unpair Listener recovery command failed; still suppressing Windows PairAsync: {}",
            embedded_ble_log_preview(&err),
        ),
        Err(err) => log::warn!(
            "[embedded-ble] manual Windows unpair Listener recovery command task failed; still suppressing Windows PairAsync: {err}"
        ),
    }
    start_embedded_ble_pairing_confirmation_watch(
        inner,
        expected_ble_name.to_string(),
        hold_generation,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
    {
        let mut wake = inner.embedded_ble_wake_recovery.lock();
        wake.status = EmbeddedBleWakeRecoveryStatus::Failed;
        wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
        wake.recent_disconnect_reason = Some(format!(
            "Windows pairing was removed manually; Type opened Listener pairing window and suppressed automatic PairAsync target={expected_ble_name}"
        ));
        wake.user_guidance = format!(
            "Windows 已删除 {expected_ble_name} 的配对。Type 已让 Listener 进入可重新配对状态，但不会自动抢回连接；如果还要在这台电脑使用，请在 Windows 蓝牙里手动添加设备。"
        );
    }
    emit_embedded_ble_recovery_capsule(
        inner,
        "reconnecting",
        EmbeddedBleRecoveryCapsuleMessage::WaitingManualPairing,
        Some(4200),
    );
}

fn embedded_ble_lost_current_native_pairing_should_pause(
    err: &str,
    native_hid_addresses: &[u64],
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    is_embedded_ble_link_loss_error(err)
        && embedded_ble_current_native_pairing_is_missing(native_hid_addresses, pairing)
}

fn embedded_ble_current_native_pairing_is_missing(
    native_hid_addresses: &[u64],
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    // Owner 2026-07-28 machine gate: after Type restart / ghost dual-address state,
    // Windows AEP often returns NeedsUserAction with matched_devices>0 and
    // already_paired=0 while HID present probe is still empty. Treating that as
    // "user deleted pairing" holds the background listener for 180s and surfaces
    // as 接不上 Type. Only hold when HID present evidence AND any matched pairing
    // entry are both gone.
    native_hid_addresses.is_empty()
        && pairing.already_paired_devices == 0
        && pairing.matched_devices == 0
}

fn hold_embedded_ble_for_missing_native_pairing(inner: &Arc<Inner>, reason: &str) {
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
        inner,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
    log::warn!(
        "[embedded-ble] current native Windows Listener HID is absent; releasing stale GATT and waiting for explicit local re-pair hold_generation={hold_generation} target={expected_ble_name:?} reason={}",
        embedded_ble_log_preview(reason),
    );
    {
        let mut wake = inner.embedded_ble_wake_recovery.lock();
        wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
        wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
        wake.recent_disconnect_reason = Some(format!(
            "current native Windows Listener HID disappeared; waiting for explicit local re-pair target={expected_ble_name}"
        ));
        wake.user_guidance = format!(
            "Listener 正在等待 Windows 重新配对 {expected_ble_name}。Type 已释放旧蓝牙连接，不会自动抢回设备。"
        );
    }
    emit_embedded_ble_recovery_capsule(
        inner,
        "reconnecting",
        EmbeddedBleRecoveryCapsuleMessage::WaitingManualPairing,
        Some(4200),
    );
    start_embedded_ble_passive_local_reattach_watch(
        inner,
        expected_ble_name,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
}

fn hold_embedded_ble_for_stale_native_hid_recovery(
    inner: &Arc<Inner>,
    reason: &str,
    native_hid_addresses: Vec<u64>,
    pairing: crate::embedded_ble::BleDevicePairingPromptResult,
) {
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
        inner,
        EMBEDDED_BLE_STALE_NATIVE_HID_RECOVERY_WAIT_REASON,
    );
    log::warn!(
        "[embedded-ble] stale native Windows HID detected; waiting for a real Listener recovery advertisement before bounded local PairAsync hold_generation={hold_generation} target={expected_ble_name:?} reason={}",
        embedded_ble_log_preview(reason),
    );
    {
        let mut wake = inner.embedded_ble_wake_recovery.lock();
        wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
        wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
        wake.recent_disconnect_reason = Some(format!(
            "stale native Windows HID is waiting for Listener recovery advertising target={expected_ble_name}"
        ));
        wake.user_guidance =
            "Listener 正在等待设备重新进入恢复广播；检测到后会自动恢复本机蓝牙连接。".to_string();
    }
    emit_embedded_ble_recovery_capsule(
        inner,
        "reconnecting",
        EmbeddedBleRecoveryCapsuleMessage::WaitingTypePairing,
        Some(4200),
    );
    start_embedded_ble_stale_native_hid_recovery_watch(
        inner,
        expected_ble_name,
        hold_generation,
        native_hid_addresses,
        pairing,
    );
}

fn start_embedded_ble_stale_native_hid_recovery_watch(
    inner: &Arc<Inner>,
    expected_ble_name: String,
    hold_generation: u64,
    native_hid_addresses: Vec<u64>,
    pairing: crate::embedded_ble::BleDevicePairingPromptResult,
) {
    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        log::info!(
            "[embedded-ble] stale native HID recovery advertisement watch started target={expected_ble_name:?} hold_generation={hold_generation}"
        );
        if inner.shutdown.load(Ordering::SeqCst)
            || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            || inner
                .embedded_ble_pairing_hold_generation
                .load(Ordering::SeqCst)
                != hold_generation
        {
            return;
        }

        // Keep one watcher alive for the bounded pairing window. Restarting four-second
        // scans introduced a blind gap after a physical EC11 double-click, even though the
        // startup preflight had already proved this PC owns only a stale local HID record.
        let expected_for_scan = expected_ble_name.clone();
        let recovery_pairing_probe = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::listener_recovery_pairing_advertisement_probe(
                Some(&expected_for_scan),
                EMBEDDED_BLE_PAIRING_CONFIRMATION_HOLD,
            )
        })
        .await;
        let Ok(recovery_pairing_probe) = recovery_pairing_probe else {
            log::warn!(
                "[embedded-ble] stale native HID recovery advertisement watch task failed; keeping passive ownership"
            );
            return;
        };
        if !recovery_pairing_probe.visible
            || inner.shutdown.load(Ordering::SeqCst)
            || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            || inner
                .embedded_ble_pairing_hold_generation
                .load(Ordering::SeqCst)
                != hold_generation
        {
            return;
        }
        if !startup_stale_native_hid_recovery_is_authorized(
            &native_hid_addresses,
            &pairing,
            &recovery_pairing_probe,
        ) {
            log::info!(
                "[embedded-ble] recovery advertising is visible but cached stale-HID ownership proof is incomplete; preserving passive ownership native_hid_count={} status={:?} matched={} already_paired={} failed={}",
                native_hid_addresses.len(),
                pairing.status,
                pairing.matched_devices,
                pairing.already_paired_devices,
                pairing.failed_devices,
            );
            return;
        }

        let observed_addresses = recovery_pairing_probe
            .addresses
            .iter()
            .map(|address| format!("{address:012X}"))
            .collect::<Vec<_>>();
        let recovery_error = format!(
            "Listener recovery Swift Pair advertisement visible for persisted address {} after stale native HID recovery watch; missing pairing must use Type automatic PairAsync recovery before declaring notify ready",
            observed_addresses.join(",")
        );
        log::warn!(
            "[embedded-ble] stale native HID recovery watch used startup stale-pairing proof and active recovery advertising; starting bounded Type PairAsync recovery recovery_addresses={observed_addresses:?}"
        );
        clear_embedded_ble_pairing_confirmation_hold(
            &inner,
            "stale native HID recovery advertisement observed",
        );
        record_embedded_ble_listener_last_error(&inner, &recovery_error);
        let mut cleanup_at = None;
        let outcome = maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
            &inner,
            &recovery_error,
            &mut cleanup_at,
        )
        .await;
        log::info!(
            "[embedded-ble] stale native HID recovery watch Type recovery outcome={outcome:?}"
        );
    });
}

async fn maybe_hold_embedded_ble_after_lost_native_pairing(inner: &Arc<Inner>, err: &str) -> bool {
    if !is_embedded_ble_link_loss_error(err)
        || embedded_ble_type_pairasync_startup_guard_active(inner)
    {
        return false;
    }
    let native_hid_addresses = match async_runtime::spawn_blocking(|| {
        crate::embedded_ble::native_windows_hid_pairing_addresses()
    })
    .await
    {
        Ok(Ok(addresses)) => addresses,
        Ok(Err(query_err)) => {
            log::debug!(
                "[embedded-ble] current native Windows HID check unavailable during link recovery: {query_err}"
            );
            return false;
        }
        Err(query_err) => {
            log::debug!(
                "[embedded-ble] current native Windows HID task failed during link recovery: {query_err}"
            );
            return false;
        }
    };
    if !native_hid_addresses.is_empty() {
        return false;
    }

    let expected_ble_name = inner.prefs.get().device_ble_name;
    let expected_for_query = expected_ble_name.clone();
    let pairing = match async_runtime::spawn_blocking(move || {
        crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
    })
    .await
    {
        Ok(result) => result,
        Err(query_err) => {
            log::debug!(
                "[embedded-ble] Windows pairing query task failed during link recovery: {query_err}"
            );
            return false;
        }
    };
    if !embedded_ble_lost_current_native_pairing_should_pause(err, &native_hid_addresses, &pairing)
    {
        return false;
    }

    log::info!(
        "[embedded-ble] link recovery found no current native HID or paired Listener status={:?} matched={} already_paired={}; suppressing stale GATT retry",
        pairing.status,
        pairing.matched_devices,
        pairing.already_paired_devices,
    );
    hold_embedded_ble_for_missing_native_pairing(inner, err);
    true
}

async fn maybe_hold_embedded_ble_startup_without_current_native_pairing(
    inner: &Arc<Inner>,
) -> bool {
    if embedded_ble_type_pairasync_startup_guard_active(inner) {
        log::info!(
            "[embedded-ble] startup manual-delete pairing preflight deferred while Type PairAsync services rebuild"
        );
        return false;
    }
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let started_at = Instant::now();
    let native_hid_addresses = match async_runtime::spawn_blocking(|| {
        crate::embedded_ble::native_windows_hid_present_pairing_addresses_for_startup()
    })
    .await
    {
        Ok(Ok(addresses)) => addresses,
        Ok(Err(err)) => {
            log::warn!(
                "[embedded-ble] startup native Windows HID pairing evidence unavailable; preserving persisted GATT path: {err}"
            );
            return false;
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] startup native Windows HID pairing evidence task failed; preserving persisted GATT path: {err}"
            );
            return false;
        }
    };
    crate::startup_evidence::record_startup_stage("native_hid_pnp_ready");
    if !native_hid_addresses.is_empty() {
        let active_addresses = native_hid_addresses.clone();
        let active_connection = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::native_windows_hid_pairing_active_connection(&active_addresses)
        })
        .await;
        crate::startup_evidence::record_startup_stage("native_hid_active_connection_finished");
        match active_connection {
            Ok(Ok(Some(address))) => {
                log::info!(
                    "[embedded-ble] startup native Windows HID active connection allows persisted GATT reopen address={address:012X}; ignoring incomplete paired-device enumeration"
                );
                return false;
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => log::warn!(
                "[embedded-ble] startup native Windows HID active-connection probe unavailable; keeping manual-delete preflight: {err}"
            ),
            Err(err) => log::warn!(
                "[embedded-ble] startup native Windows HID active-connection probe task failed; keeping manual-delete preflight: {err}"
            ),
        }
    }
    let expected_for_query = expected_ble_name.clone();
    let query = async_runtime::spawn_blocking(move || {
        crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
    })
    .await;
    let pairing = match query {
        Ok(pairing) => pairing,
        Err(err) => {
            log::warn!(
                "[embedded-ble] startup manual-delete pairing preflight task failed; preserving persisted GATT fast path: {err}"
            );
            return false;
        }
    };
    log::info!(
        "[embedded-ble] startup manual-delete pairing preflight status={:?} matched={} already_paired={} failed={} open_settings={} elapsed_ms={}",
        pairing.status,
        pairing.matched_devices,
        pairing.already_paired_devices,
        pairing.failed_devices,
        pairing.open_bluetooth_settings,
        started_at.elapsed().as_millis(),
    );
    if !native_hid_addresses.is_empty() {
        let labels = native_hid_addresses
            .iter()
            .map(|address| format!("{address:012X}"))
            .collect::<Vec<_>>();
        if pairing.already_paired_devices > 0 {
            log::info!(
                "[embedded-ble] startup native Windows HID/current pairing evidence allows persisted GATT reopen addresses={labels:?} elapsed_ms={}",
                started_at.elapsed().as_millis(),
            );
            return false;
        }
        // Owner 2026-07-28: after failed OTA, Windows often reports NeedsUserAction
        // with matched>0/already_paired=0 while HID is still present. Treating that as
        // "stale HID wait for recovery ad" holds notify 180s and shows find-Type LED.
        // Matched AEP + present HID means bond cache is not gone — reopen GATT.
        if pairing.matched_devices > 0 {
            log::info!(
                "[embedded-ble] startup native Windows HID present with matched={} already_paired=0 status={:?}; allowing persisted GATT reopen (incomplete AEP, not stale unpair) addresses={labels:?} elapsed_ms={}",
                pairing.matched_devices,
                pairing.status,
                started_at.elapsed().as_millis(),
            );
            return false;
        }
        let expected_for_scan = expected_ble_name.clone();
        let recovery_pairing_probe = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::listener_recovery_pairing_advertisement_probe(
                Some(&expected_for_scan),
                EMBEDDED_BLE_RECOVERY_PAIRING_ADV_SCAN_TIMEOUT,
            )
        })
        .await
        .unwrap_or_else(|err| {
            log::warn!(
                "[embedded-ble] startup stale-HID recovery advertisement probe task failed: {err}"
            );
            crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default()
        });
        if startup_stale_native_hid_recovery_is_authorized(
            &native_hid_addresses,
            &pairing,
            &recovery_pairing_probe,
        ) {
            let observed_addresses = recovery_pairing_probe
                .addresses
                .iter()
                .map(|address| format!("{address:012X}"))
                .collect::<Vec<_>>();
            let recovery_error = format!(
                "Listener recovery Swift Pair advertisement visible for persisted address {} after startup stale native HID pairing; missing pairing must use Type automatic PairAsync recovery before declaring notify ready",
                observed_addresses.join(",")
            );
            log::warn!(
                "[embedded-ble] startup stale native HID pairing matches active Listener recovery advertising; entering bounded local Type PairAsync recovery native_hid_addresses={labels:?} recovery_addresses={observed_addresses:?}"
            );
            record_embedded_ble_listener_last_error(inner, &recovery_error);
            let mut startup_cleanup_at = None;
            let outcome = maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
                inner,
                &recovery_error,
                &mut startup_cleanup_at,
            )
            .await;
            log::info!("[embedded-ble] startup stale native HID recovery outcome={outcome:?}");
            return outcome == EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
        }
        log::warn!(
            "[embedded-ble] startup stale native HID has no matching recovery advertisement; preserving user/external ownership native_hid_addresses={labels:?}"
        );
        hold_embedded_ble_for_stale_native_hid_recovery(
            inner,
            "startup found stale native Windows HID without current Listener recovery advertising",
            native_hid_addresses,
            pairing,
        );
        return true;
    }
    if !embedded_ble_current_native_pairing_is_missing(&native_hid_addresses, &pairing) {
        if pairing.matched_devices > 0 && pairing.already_paired_devices == 0 {
            log::info!(
                "[embedded-ble] startup Windows pairing enumeration matched={} already_paired=0 status={:?}; allowing persisted GATT reopen (not a manual unpair)",
                pairing.matched_devices,
                pairing.status
            );
        }
        return false;
    }
    log::warn!(
        "[embedded-ble] startup current native Windows pairing is absent; blocking persisted GATT reopen target={expected_ble_name:?}"
    );
    hold_embedded_ble_for_missing_native_pairing(
        inner,
        "startup found no current native Windows Listener HID or paired device",
    );
    true
}

fn startup_stale_native_hid_recovery_is_authorized(
    native_hid_addresses: &[u64],
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
    recovery_pairing_probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> bool {
    !native_hid_addresses.is_empty()
        && recovery_pairing_probe.visible
        && !native_windows_hid_pairing_blocks_type_pairasync(
            true,
            recovery_pairing_probe,
            Some(pairing),
        )
}

fn embedded_ble_background_pairasync_is_authorized(
    manual_unpair_hold: bool,
    type_observed_recovery_advertisement: bool,
    visible_recovery_allows_cleanup: bool,
    stale_cleanup_candidate: bool,
    noisy_cccd_stale_cache_type_owned_cleanup: bool,
    local_stale_cache_recovery_allows_cleanup: bool,
    firmware_name_changed: bool,
) -> bool {
    // A manual Windows removal is explicit user ownership. Background heuristics
    // must never turn it into an automatic re-pair of the old computer.
    !manual_unpair_hold
        && (type_observed_recovery_advertisement
            || visible_recovery_allows_cleanup
            || stale_cleanup_candidate
            || noisy_cccd_stale_cache_type_owned_cleanup
            || local_stale_cache_recovery_allows_cleanup
            || firmware_name_changed)
}

fn should_attempt_embedded_ble_background_stale_pairing_cleanup(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    if !is_embedded_ble_background_stale_pairing_cleanup_candidate(err, snapshot) {
        return false;
    }
    if last_cleanup_at.is_some_and(|last| {
        now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
    }) {
        return false;
    }
    true
}

fn should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    if last_cleanup_at.is_some_and(|last| {
        now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
    }) {
        return false;
    }
    if snapshot.consecutive_reconnect_failures
        < EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD
    {
        return false;
    }
    if !matches!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Failed
            | EmbeddedBleNotifySubscriptionState::Opening
            | EmbeddedBleNotifySubscriptionState::Lost
            | EmbeddedBleNotifySubscriptionState::Unknown
    ) {
        return false;
    }
    if !is_embedded_ble_link_loss_error(err) || is_embedded_ble_low_power_idle_candidate(err) {
        return false;
    }
    let failure = crate::embedded_ble::classify_ble_failure(err);
    matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::PairedButDisconnected
            | crate::embedded_ble::BleFailureKind::Unknown
    )
}

fn should_throttle_embedded_ble_background_stale_pairing_cleanup(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    is_embedded_ble_background_stale_pairing_cleanup_candidate(err, snapshot)
        && last_cleanup_at.is_some_and(|last| {
            now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
        })
}

fn is_embedded_ble_background_stale_pairing_cleanup_candidate(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
) -> bool {
    if snapshot.usb_powered == Some(false) {
        return false;
    }
    if !matches!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Failed
            | EmbeddedBleNotifySubscriptionState::Opening
            | EmbeddedBleNotifySubscriptionState::Lost
            | EmbeddedBleNotifySubscriptionState::Unknown
    ) {
        return false;
    }

    let failure = crate::embedded_ble::classify_ble_failure(err);
    let reconnect_attempt_threshold = match failure.kind {
        crate::embedded_ble::BleFailureKind::CccdProtocolError
            if is_embedded_ble_noisy_cccd_failure(err) =>
        {
            EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD
        }
        crate::embedded_ble::BleFailureKind::StaleGattService
        | crate::embedded_ble::BleFailureKind::MissingPairing => {
            EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD
        }
        _ => return false,
    };

    snapshot.consecutive_reconnect_failures >= reconnect_attempt_threshold
}

fn next_embedded_ble_background_retry_delay(err: &str, current: Duration) -> Duration {
    if is_embedded_ble_idle_timeout_error(err) {
        return EMBEDDED_BLE_RETRY_BASE_DELAY;
    }

    if is_embedded_ble_link_loss_error(err) {
        return EMBEDDED_BLE_RETRY_FAST_DELAY;
    }

    if is_embedded_ble_noisy_cccd_failure(err) {
        return EMBEDDED_BLE_RETRY_NOISY_CCCD_DELAY;
    }

    if is_embedded_ble_background_offline_backoff_error(err) {
        return EMBEDDED_BLE_RETRY_OFFLINE_DELAY;
    }

    if is_embedded_ble_transient_reopen_error(err) {
        return EMBEDDED_BLE_RETRY_LONG_DELAY
            .max(current)
            .min(EMBEDDED_BLE_RETRY_MAX_DELAY);
    }

    current
        .saturating_mul(2)
        .clamp(EMBEDDED_BLE_RETRY_BASE_DELAY, EMBEDDED_BLE_RETRY_MAX_DELAY)
}

fn is_embedded_ble_automatic_recovery_error(err: &str) -> bool {
    is_embedded_ble_link_loss_error(err)
        || crate::embedded_ble::classify_ble_failure(err).automatic_recovery
}

fn is_embedded_ble_background_offline_backoff_error(err: &str) -> bool {
    denzic_ble_windows::failure::is_ble_offline_backoff_error(err)
}

fn is_embedded_ble_noisy_cccd_failure(err: &str) -> bool {
    denzic_ble_windows::failure::is_ble_noisy_cccd_failure(
        err,
        &crate::embedded_ble::LISTENER_BLE_FAILURE_HINTS,
    )
}

fn embedded_ble_usb_power_allows_low_power_idle(usb_powered: Option<bool>) -> bool {
    usb_powered == Some(false)
}

fn embedded_ble_log_preview(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.chars().take(240).collect()
}

fn is_embedded_ble_low_power_idle_candidate(err: &str) -> bool {
    matches!(
        crate::embedded_ble::classify_ble_failure(err).kind,
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
    )
}

fn should_emit_embedded_ble_background_recovery_capsule(inner: &Arc<Inner>, err: &str) -> bool {
    let failure = crate::embedded_ble::classify_ble_failure(err);
    let link_loss = is_embedded_ble_link_loss_error(err);
    let automatic_recovery = link_loss || failure.automatic_recovery;
    let offline_backoff = is_embedded_ble_background_offline_backoff_error(err);
    let noisy_cccd = is_embedded_ble_noisy_cccd_failure(err);
    let low_power_idle = matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
    );
    let snapshot = inner.embedded_ble_wake_recovery.lock();
    let reconnect_attempts = snapshot.reconnect_attempts;
    let usb_powered = snapshot.usb_powered;
    drop(snapshot);
    let low_power_recovery_capsule_allowed = !low_power_idle || usb_powered == Some(true);
    let decision = automatic_recovery
        && !offline_backoff
        && !noisy_cccd
        && reconnect_attempts <= 1
        && low_power_recovery_capsule_allowed;
    let decision_reason = if !automatic_recovery {
        "not_automatic_recovery"
    } else if offline_backoff {
        "offline_backoff"
    } else if noisy_cccd {
        "noisy_cccd_retry"
    } else if reconnect_attempts > 1 {
        "repeat_attempt_suppressed"
    } else if low_power_idle && usb_powered != Some(true) {
        "low_power_idle_power_unknown_or_battery_suppressed"
    } else {
        "emit"
    };
    log::info!(
        "[embedded-ble] background recovery capsule decision emit={} reason={} kind={:?} automatic_recovery={} link_loss={} offline_backoff={} noisy_cccd={} reconnect_attempts={} usb_powered={:?} low_power_idle={} err={}",
        decision,
        decision_reason,
        failure.kind,
        failure.automatic_recovery,
        link_loss,
        offline_backoff,
        noisy_cccd,
        reconnect_attempts,
        usb_powered,
        low_power_idle,
        embedded_ble_log_preview(err),
    );
    decision
}

fn should_emit_embedded_ble_recovered_capsule_for_reason(
    reason: &str,
    usb_powered: Option<bool>,
    reconnect_attempts: u32,
) -> bool {
    let normalized = reason.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "refresh" | "shutdown" | "test" | "test cleanup"
    ) {
        return false;
    }
    let failure = crate::embedded_ble::classify_ble_failure(reason);
    let automatic_recovery = is_embedded_ble_link_loss_error(reason) || failure.automatic_recovery;
    let repeated_automatic_recovery = automatic_recovery && reconnect_attempts > 1;
    // 对齐 should_emit_embedded_ble_background_recovery_capsule：仅真正的链路恢复
    // (link loss / automatic recovery) 才弹"已恢复"胶囊。所有权冲突等非链路原因
    // (例如"当前已有听写会话在运行，暂不能提交嵌入式音频")并不是链路掉线后的恢复,
    // 若每次 notify-ready 都弹,在重连循环里会变成胶囊刷屏——故对非链路原因一律不弹。
    automatic_recovery
        && !repeated_automatic_recovery
        && (!is_embedded_ble_low_power_idle_candidate(reason) || usb_powered == Some(true))
}

fn is_embedded_ble_link_loss_error(err: &str) -> bool {
    denzic_ble_windows::failure::is_ble_link_loss_error(err)
}

fn is_embedded_ble_transient_reopen_error(err: &str) -> bool {
    denzic_ble_windows::failure::is_ble_transient_reopen_error(err)
}

fn is_embedded_ble_idle_timeout_error(err: &str) -> bool {
    err.contains("BLE embedded audio capture timed out")
}

fn embedded_ble_listener_capture_active(inner: &Arc<Inner>) -> bool {
    inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|cancel| !cancel.load(Ordering::SeqCst))
}

fn embedded_ble_listener_capture_ready(inner: &Arc<Inner>) -> bool {
    embedded_ble_listener_capture_active(inner)
        && inner.embedded_ble_listener_ready.load(Ordering::SeqCst)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EmbeddedBleForegroundProbeMode {
    /// Capture is live and TYPE:READY — never tear it down for a UI/health probe.
    ReuseReadyBackground,
    /// Capture exists but is not ready yet — refresh may unstick a hung open.
    RefreshBackgroundListener,
    StartBackgroundListener,
    ForegroundProbe,
}

fn embedded_ble_foreground_probe_mode(inner: &Arc<Inner>) -> EmbeddedBleForegroundProbeMode {
    if embedded_ble_listener_capture_ready(inner) {
        EmbeddedBleForegroundProbeMode::ReuseReadyBackground
    } else if embedded_ble_listener_capture_active(inner) {
        EmbeddedBleForegroundProbeMode::RefreshBackgroundListener
    } else if embedded_ble_background_listener_expected(inner) {
        EmbeddedBleForegroundProbeMode::StartBackgroundListener
    } else {
        EmbeddedBleForegroundProbeMode::ForegroundProbe
    }
}

fn embedded_ble_background_listener_expected(inner: &Arc<Inner>) -> bool {
    std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
        .ok()
        .as_deref()
        != Some("1")
        && inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
}

async fn wait_for_embedded_ble_listener_ready(
    inner: &Arc<Inner>,
    timeout: Duration,
) -> Result<(), String> {
    if !embedded_ble_background_listener_expected(inner)
        && !embedded_ble_listener_capture_active(inner)
    {
        return Ok(());
    }
    let deadline = Instant::now() + timeout;
    loop {
        if embedded_ble_listener_capture_ready(inner) {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let last_error = inner.embedded_ble_listener_last_error.lock().clone();
            let suffix = last_error
                .as_deref()
                .map(|err| format!("; last error: {err}"))
                .unwrap_or_default();
            return Err(format!(
                "Listener BLE notify subscription did not recover within {} ms after foreground probe{suffix}",
                timeout.as_millis()
            ));
        }
        // Create the waiter before the second ready check: if CCCD completes in
        // this narrow interval, either the state check succeeds or Notify keeps
        // the wake-up permit. This preserves the timeout diagnostic without a
        // polling-sized delay after the actual TYPE:READY edge.
        let ready_notification = inner.embedded_ble_listener_ready_notification.notified();
        if embedded_ble_listener_capture_ready(inner) {
            return Ok(());
        }
        tokio::select! {
            _ = ready_notification => {}
            _ = tokio::time::sleep(remaining) => {
                let last_error = inner.embedded_ble_listener_last_error.lock().clone();
                let suffix = last_error
                    .as_deref()
                    .map(|err| format!("; last error: {err}"))
                    .unwrap_or_default();
                return Err(format!(
                    "Listener BLE notify subscription did not recover within {} ms after foreground probe{suffix}",
                    timeout.as_millis()
                ));
            }
        }
    }
}

/// Like [`wait_for_embedded_ble_listener_ready`], but requires ready to hold so a
/// one-shot edge (post-OTA CCCD race / immediate disconnect) does not report success.
async fn wait_for_embedded_ble_listener_ready_stable(
    inner: &Arc<Inner>,
    timeout: Duration,
    stable_for: Duration,
) -> Result<(), String> {
    if !embedded_ble_background_listener_expected(inner)
        && !embedded_ble_listener_capture_active(inner)
    {
        return Ok(());
    }
    let deadline = Instant::now() + timeout;
    loop {
        let remaining_for_edge = deadline.saturating_duration_since(Instant::now());
        if remaining_for_edge.is_zero() {
            let last_error = inner.embedded_ble_listener_last_error.lock().clone();
            let suffix = last_error
                .as_deref()
                .map(|err| format!("; last error: {err}"))
                .unwrap_or_default();
            return Err(format!(
                "Listener BLE notify subscription did not stay ready within {} ms after OTA{suffix}",
                timeout.as_millis()
            ));
        }
        wait_for_embedded_ble_listener_ready(inner, remaining_for_edge).await?;
        let hold_deadline = Instant::now() + stable_for;
        let mut held = true;
        while Instant::now() < hold_deadline {
            if !embedded_ble_listener_capture_ready(inner) {
                held = false;
                log::info!(
                    "[embedded-ble] post-OTA TYPE:READY edge lost during {} ms hold; waiting again",
                    stable_for.as_millis()
                );
                break;
            }
            let slice = hold_deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50));
            if slice.is_zero() {
                break;
            }
            tokio::time::sleep(slice).await;
        }
        if held && embedded_ble_listener_capture_ready(inner) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let last_error = inner.embedded_ble_listener_last_error.lock().clone();
            let suffix = last_error
                .as_deref()
                .map(|err| format!("; last error: {err}"))
                .unwrap_or_default();
            return Err(format!(
                "Listener BLE notify subscription did not stay ready within {} ms after OTA{suffix}",
                timeout.as_millis()
            ));
        }
    }
}

async fn wait_for_embedded_ble_listener_inactive(inner: &Arc<Inner>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !embedded_ble_listener_capture_active(inner)
            && !crate::embedded_ble::notify_capture_session_active()
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(EMBEDDED_BLE_PROBE_RECOVERY_POLL).await;
    }
}

fn mark_embedded_ble_listener_ready(inner: &Arc<Inner>, cancel: &Arc<AtomicBool>) {
    if inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|active| Arc::ptr_eq(active, cancel))
        && !cancel.load(Ordering::SeqCst)
    {
        inner
            .embedded_ble_listener_ready
            .store(true, Ordering::SeqCst);
        inner
            .embedded_ble_listener_ready_notification
            .notify_waiters();
        clear_embedded_ble_listener_last_error(inner);
        let recovered = record_embedded_ble_notify_ready(inner);
        let generation = inner
            .embedded_ble_listener_generation
            .load(Ordering::SeqCst);
        let firmware_ota_recovered =
            take_embedded_ble_ota_recovery_capsule(inner, generation);
        if let Some(hid_observed_at) = inner.embedded_ble_power_cycle_hid_observed_at.lock().take()
        {
            let elapsed = hid_observed_at.elapsed();
            let elapsed_ms = elapsed.as_millis();
            let target_ms = EMBEDDED_BLE_POWER_CYCLE_AUDIO_RECOVERY_TARGET.as_millis();
            log::info!(
                "[embedded-ble] power-cycle audio notify recovery elapsed_ms={elapsed_ms} target_ms={target_ms} met={}",
                elapsed <= EMBEDDED_BLE_POWER_CYCLE_AUDIO_RECOVERY_TARGET
            );
        }
        record_embedded_ble_session_actor_command(
            inner,
            EmbeddedBleSessionActorCommand::NotifyReady,
            None,
            "background notify subscription ready",
        );
        if recovered || firmware_ota_recovered {
            log::info!(
                "[embedded-ble] audio recovered capsule trigger general_recovery={} firmware_ota_recovery={} generation={}",
                recovered,
                firmware_ota_recovered,
                generation
            );
            emit_embedded_ble_recovery_capsule(
                inner,
                "reconnected",
                EmbeddedBleRecoveryCapsuleMessage::AudioRecovered,
                Some(1400),
            );
        }
        flush_pending_device_key_ble_action(inner, "notify_ready");
        log::info!("[embedded-ble] background listener notify ready");
        // After a live notify identity is proven, prune same-name Windows ghosts
        // so the next OTA/reboot reconnect does not time out dead HID roots first.
        maybe_prune_listener_ghost_pairings_after_notify_ready(inner);
    }
}

fn maybe_prune_listener_ghost_pairings_after_notify_ready(inner: &Arc<Inner>) {
    let keep = crate::embedded_ble::current_notify_keep_address();
    let Some(keep) = keep else {
        return;
    };
    let expected_name = inner.prefs.get().device_ble_name;
    // Immediate PnP/BTHPORT ghost prune after TYPE:READY has bounced the live GATT
    // link (logs: notify ready → prune removes old "listener" root → Disconnected →
    // capsule "接不上 Type" while firmware LED still shows type_ready). Always settle
    // first, and cooldown so reconnect flaps do not prune every generation.
    const GHOST_PRUNE_SETTLE: Duration = Duration::from_secs(8);
    const GHOST_PRUNE_COOLDOWN: Duration = Duration::from_secs(180);
    static LAST_GHOST_PRUNE_AT: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(last) = LAST_GHOST_PRUNE_AT.lock() {
            if let Some(at) = *last {
                if at.elapsed() < GHOST_PRUNE_COOLDOWN {
                    log::info!(
                        "[embedded-ble] skipping ghost pairing prune keep={keep:012X}: cooldown {} ms remaining",
                        GHOST_PRUNE_COOLDOWN
                            .saturating_sub(at.elapsed())
                            .as_millis()
                    );
                    return;
                }
            }
        }
        log::info!(
            "[embedded-ble] deferring ghost pairing prune {} ms after TYPE:READY keep={keep:012X}",
            GHOST_PRUNE_SETTLE.as_millis()
        );
        std::thread::sleep(GHOST_PRUNE_SETTLE);
        // OTA exclusive transfer can start during the settle window. Pruning
        // PnP/BTHPORT mid-transfer races GATT BEGIN/WWR (logs: dual-lane start →
        // ghost-prune → ota_gatt_transfer_failed ~10s). Skip while OTA owns BLE.
        if inner.embedded_ble_ota_active.load(Ordering::SeqCst) {
            log::info!(
                "[embedded-ble] skipping ghost pairing prune keep={keep:012X}: firmware OTA exclusive transfer active"
            );
            return;
        }
        let mut names = vec![expected_name];
        for fallback in ["listener", "Blistener"] {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(fallback)) {
                names.push(fallback.to_string());
            }
        }
        let _ = crate::embedded_ble::prune_listener_ghost_pairings_keeping(&names, &[keep]);
        if let Ok(mut last) = LAST_GHOST_PRUNE_AT.lock() {
            *last = Some(Instant::now());
        }
    });
}

fn install_embedded_ble_listener_cancel(inner: &Arc<Inner>, generation: u64) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let leave_notify_cccd_enabled_on_cancel = Arc::new(AtomicBool::new(false));
    inner
        .embedded_ble_listener_ready
        .store(false, Ordering::SeqCst);
    let previous = {
        let mut slot = inner.embedded_ble_listener_cancel.lock();
        slot.replace(Arc::clone(&cancel))
    };
    *inner
        .embedded_ble_listener_leave_cccd_enabled_on_cancel
        .lock() = Some((
        Arc::clone(&cancel),
        Arc::clone(&leave_notify_cccd_enabled_on_cancel),
    ));
    if let Some(previous) = previous {
        previous.store(true, Ordering::SeqCst);
        log::warn!(
            "[embedded-ble] replaced an active background capture cancel flag (generation={generation})"
        );
    }
    log::info!("[embedded-ble] background capture armed generation={generation}");
    cancel
}

fn embedded_ble_listener_cccd_handoff_flag(
    inner: &Arc<Inner>,
    cancel: &Arc<AtomicBool>,
) -> Arc<AtomicBool> {
    inner
        .embedded_ble_listener_leave_cccd_enabled_on_cancel
        .lock()
        .as_ref()
        .filter(|(active_cancel, _)| Arc::ptr_eq(active_cancel, cancel))
        .map(|(_, handoff)| Arc::clone(handoff))
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
}

fn clear_embedded_ble_listener_cancel(inner: &Arc<Inner>, cancel: &Arc<AtomicBool>) {
    let cleared = {
        let mut slot = inner.embedded_ble_listener_cancel.lock();
        if slot
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, cancel))
        {
            *slot = None;
            true
        } else {
            false
        }
    };
    if cleared {
        let mut handoff_slot = inner
            .embedded_ble_listener_leave_cccd_enabled_on_cancel
            .lock();
        if handoff_slot
            .as_ref()
            .is_some_and(|(active_cancel, _)| Arc::ptr_eq(active_cancel, cancel))
        {
            *handoff_slot = None;
        }
        inner
            .embedded_ble_listener_ready
            .store(false, Ordering::SeqCst);
        log::info!("[embedded-ble] background capture cancel flag cleared");
    }
}

fn cancel_embedded_ble_listener_capture(
    inner: &Arc<Inner>,
    reason: &str,
    leave_notify_cccd_enabled_on_cancel: bool,
) {
    inner
        .embedded_ble_listener_ready
        .store(false, Ordering::SeqCst);
    let previous = inner.embedded_ble_listener_cancel.lock().take();
    if let Some(cancel) = previous {
        if leave_notify_cccd_enabled_on_cancel {
            if let Some((active_cancel, handoff)) = inner
                .embedded_ble_listener_leave_cccd_enabled_on_cancel
                .lock()
                .as_ref()
            {
                if Arc::ptr_eq(active_cancel, &cancel) {
                    handoff.store(true, Ordering::SeqCst);
                    log::info!(
                        "[embedded-ble] confirmed BLE-name change will leave old notify CCCD enabled for firmware disconnect handoff"
                    );
                }
            }
        }
        record_embedded_ble_session_actor_command(
            inner,
            EmbeddedBleSessionActorCommand::NotifyCleanupDelay,
            None,
            format!("reason={reason}"),
        );
        cancel.store(true, Ordering::SeqCst);
        record_embedded_ble_listener_cancelled(inner, reason);
        log::info!("[embedded-ble] requested active background capture stop ({reason})");
    }
}

fn pause_embedded_ble_listener_capture(inner: &Arc<Inner>, reason: &str) {
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    log::info!("[embedded-ble] paused background listener generation={generation} ({reason})");
    cancel_embedded_ble_listener_capture(inner, reason, false);
}

fn pause_embedded_ble_listener_capture_for_ota(inner: &Arc<Inner>) {
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    log::info!(
        "[embedded-ble] paused background listener generation={generation} (firmware OTA transfer)"
    );
    // TYPE:OTA was sent on the live notify link immediately before this handoff.
    // Preserve the bonded CCCD and suppress TYPE:BYE so firmware retains the
    // encrypted Type lease while Windows opens the OTA characteristics.
    cancel_embedded_ble_listener_capture(inner, "firmware OTA transfer", true);
}



fn pause_embedded_ble_listener_capture_for_ble_name_apply_handoff(inner: &Arc<Inner>) {
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    log::info!(
        "[embedded-ble] paused background listener generation={generation} (BLE name apply handoff)"
    );
    cancel_embedded_ble_listener_capture(inner, "BLE name apply handoff", true);
}
