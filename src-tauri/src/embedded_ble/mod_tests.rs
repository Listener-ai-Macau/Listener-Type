//! Path-separated unit tests for `embedded_ble` (mod.rs).
//! Loaded via `#[path = "mod_tests.rs"]` from `mod.rs`.

use super::*;
use crate::embedded_audio::{
    build_audio_data_notification, build_session_cancel_notification,
    build_session_error_notification, build_session_start_notification,
    build_session_stop_notification, SessionCollector, SessionErrorCode,
};

#[cfg(target_os = "windows")]
static DEVICE_SETTINGS_HARDWARE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "windows")]
#[test]
fn pairing_prompt_throttle_is_target_scoped_and_expires() {
    let same_name_remaining = windows_ble::pairing_prompt_suppression_remaining_for_test(
        "listenerB",
        "listenerB",
        Duration::from_secs(30),
    )
    .expect("same target should be throttled inside the prompt window");
    assert!(same_name_remaining > Duration::ZERO);

    assert!(
        windows_ble::pairing_prompt_suppression_remaining_for_test(
            "listenerB",
            "Blistener",
            Duration::from_secs(30),
        )
        .is_none(),
        "a different configured BLE name must not be suppressed by an older target"
    );
    assert!(
        windows_ble::pairing_prompt_suppression_remaining_for_test(
            "listenerB",
            "listenerB",
            Duration::from_secs(60 * 60),
        )
        .is_none(),
        "the prompt throttle must expire"
    );
}

#[cfg(target_os = "windows")]
fn usb_serial_port(
    port_name: &str,
    vid: u16,
    pid: u16,
    manufacturer: &str,
    product: &str,
) -> serialport::SerialPortInfo {
    serialport::SerialPortInfo {
        port_name: port_name.to_string(),
        port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
            vid,
            pid,
            serial_number: Some(format!("{port_name}-serial")),
            manufacturer: Some(manufacturer.to_string()),
            product: Some(product.to_string()),
        }),
    }
}

#[test]
fn terminal_detection_only_matches_stop_cancel_error() {
    assert!(!is_terminal_notification(
        &build_session_start_notification(1)
    ));
    assert!(!is_terminal_notification(
        &build_audio_data_notification(1, 0, &[1, 2]).expect("audio packet")
    ));
    assert!(is_terminal_notification(&build_session_stop_notification(
        1, 1
    )));
    assert!(is_terminal_notification(
        &build_session_cancel_notification(1, 1)
    ));
    assert!(is_terminal_notification(&build_session_error_notification(
        1,
        1,
        SessionErrorCode::LinkLost,
    )));
}

#[test]
fn invalid_notification_is_not_terminal() {
    assert!(!is_terminal_notification(b"not-vka1"));
}

#[test]
fn ec11_hardware_recovery_notice_is_not_audio_terminal() {
    assert!(is_ec11_hardware_recovery_prepare_notice(
        b"listener-ec11-recovery-prepare-v1"
    ));
    assert!(is_ec11_hardware_recovery_notice(
        b"listener-ec11-recovery-v1"
    ));
    assert_eq!(EC11_HARDWARE_RECOVERY_ACK, b"TYPE:EC11:RECOVERY:ACK\n");
    assert_eq!(
        EC11_HARDWARE_RECOVERY_PREPARE_ACK,
        b"TYPE:EC11:RECOVERY:PREPARE:ACK\n"
    );
    assert!(!is_terminal_notification(
        b"listener-ec11-recovery-prepare-v1"
    ));
    assert!(!is_terminal_notification(b"listener-ec11-recovery-v1"));
}

#[cfg(target_os = "windows")]
#[test]
fn ec11_recovery_pre_authorization_is_reversible_until_the_firmware_disconnects() {
    let source = concat!(include_str!("mod.rs"), "\n", include_str!("windows_ble.rs"));
    let prepare_start = source
        .find("if super::is_ec11_hardware_recovery_prepare_notice(&notification) {")
        .expect("EC11 recovery pre-authorization branch should exist");
    let final_notice_start = source[prepare_start..]
        .find("if super::is_ec11_hardware_recovery_notice(&notification) {")
        .map(|offset| prepare_start + offset)
        .expect("final EC11 recovery branch should follow pre-authorization");
    let prepare = &source[prepare_start..final_notice_start];
    assert!(
        prepare.contains("write_ec11_recovery_prepare_acknowledgement()")
            && prepare.contains("EC11_HARDWARE_RECOVERY_PREPARE_TIMEOUT"),
        "the first click must only establish a bounded active-GATT pre-authorization"
    );
    assert!(
        source
            .contains("EC11 recovery pre-authorization expired without a firmware disconnect"),
        "a single click or long press must let the pre-authorization expire silently"
    );
    assert!(
        source.contains("firmware disconnect observed during EC11 pre-authorized double-click window"),
        "only the subsequent firmware disconnect may enter the existing recovery advertising arbitration"
    );
    assert!(
        source.contains("|| bytes == super::EC11_HARDWARE_RECOVERY_PREPARE_ACK"),
        "the pre-authorization acknowledgement must keep the low-latency active-control write policy"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn ec11_hardware_recovery_notice_requires_active_gatt_ack_before_pairasync_authorization() {
    let source = concat!(include_str!("mod.rs"), "\n", include_str!("windows_ble.rs"));
    let notice_start = source
        .find("if super::is_ec11_hardware_recovery_notice(&notification) {")
        .expect("EC11 recovery notice branch should exist");
    let notice_end = source[notice_start..]
        .find("let terminal = super::is_terminal_notification")
        .map(|offset| notice_start + offset)
        .expect("EC11 recovery notice branch boundary should exist");
    let notice = &source[notice_start..notice_end];
    let ack_index = notice
        .find("cleanup.write_ec11_recovery_acknowledgement()")
        .expect("EC11 recovery must acknowledge the current active GATT session");
    let deadline_index = notice
        .find("ec11_recovery_disconnect_deadline")
        .expect("only an acknowledged notice may authorize PairAsync after disconnect");
    assert!(
        ack_index < deadline_index,
        "EC11 recovery must write its active-session acknowledgement before allowing the disconnect-driven PairAsync path"
    );
    assert!(
        notice.contains("EC11 recovery acknowledgement queued via active GATT control"),
        "EC11 recovery acknowledgement must leave installed-Type log evidence before PairAsync authorization"
    );

    let ack_start = source
        .find("fn write_ec11_recovery_acknowledgement(&self)")
        .expect("EC11 acknowledgement helper should exist");
    let ack_end = source[ack_start..]
        .find("fn disable_notify(&mut self)")
        .map(|offset| ack_start + offset)
        .expect("EC11 acknowledgement helper boundary should exist");
    let ack = &source[ack_start..ack_end];
    assert!(
        ack.contains("super::EC11_HARDWARE_RECOVERY_ACK")
            && ack.contains("EC11_HARDWARE_RECOVERY_ACK_WRITE_TIMEOUT"),
        "EC11 acknowledgement must be an explicit bounded active-GATT control write"
    );
    assert!(
        source.contains("|| bytes == super::EC11_HARDWARE_RECOVERY_ACK"),
        "EC11 acknowledgement must keep the low-latency active-control write policy"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn active_capture_link_recovery_only_during_open_audio_session() {
    let mut collector = SessionCollector::default();
    assert!(!windows_ble::collector_has_active_recoverable_session(
        &collector
    ));

    collector
        .handle_notification(&build_session_start_notification(7))
        .expect("start notification");
    assert!(windows_ble::collector_has_active_recoverable_session(
        &collector
    ));

    collector
        .handle_notification(
            &build_audio_data_notification(7, 0, &[1, 2]).expect("audio notification"),
        )
        .expect("audio notification");
    assert!(windows_ble::collector_has_active_recoverable_session(
        &collector
    ));

    collector
        .handle_notification(&build_session_stop_notification(7, 1))
        .expect("stop notification");
    assert!(!windows_ble::collector_has_active_recoverable_session(
        &collector
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn active_capture_link_recovery_timeout_stays_bounded() {
    let (heartbeat_interval, recovery_timeout) =
        windows_ble::active_capture_recovery_timing_for_test();
    assert_eq!(heartbeat_interval, Duration::from_secs(8));
    assert_eq!(recovery_timeout, Duration::from_secs(5));
    assert!(recovery_timeout < heartbeat_interval);
}

#[cfg(target_os = "windows")]
#[test]
fn active_capture_recovery_advertisement_bypasses_link_timeout() {
    let source = concat!(include_str!("mod.rs"), "\n", include_str!("windows_ble.rs"));
    let helper_start = source
        .find("fn active_capture_disconnect_recovery_pairing_error")
        .expect("active capture recovery helper should exist");
    let helper_end = source[helper_start..]
        .find("fn open_notify_target_from_advertisement")
        .map(|offset| helper_start + offset)
        .expect("active capture recovery helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    assert!(
        helper.contains("recovery_swift_pair_advertisement_visible_for_persisted_address")
            && helper.contains("missing pairing must use Type automatic PairAsync recovery"),
        "only a matching recovery advertisement may turn an active capture disconnect into Type PairAsync recovery"
    );

    let disconnect_start = source
        .find("BleCaptureSignal::Disconnected(reason) =>")
        .expect("capture disconnect branch should exist");
    let disconnect_end = source[disconnect_start..]
        .find("let terminal = super::is_terminal_notification")
        .map(|offset| disconnect_start + offset)
        .expect("capture disconnect branch boundary should exist");
    let disconnect = &source[disconnect_start..disconnect_end];
    let recovery_index = disconnect
        .find("active_capture_disconnect_recovery_pairing_error")
        .expect("active capture must inspect recovery advertising before waiting");
    let timeout_index = disconnect
        .find("ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT")
        .expect("ordinary active-session recovery timeout should remain available");
    assert!(
        recovery_index < timeout_index,
        "a confirmed EC11 recovery advertisement must bypass the active-session timeout, while ordinary link loss keeps the bounded wait"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn type_heartbeat_runs_only_for_background_capture() {
    let (one_shot, background) =
        windows_ble::type_heartbeat_terminal_behavior_matrix_for_test();
    assert!(!one_shot);
    assert!(background);
}

#[cfg(target_os = "windows")]
#[test]
fn listener_ota_v1_blocks_cross_process_background_heartbeat() {
    let source = concat!(include_str!("mod.rs"), "\n", include_str!("windows_ble.rs"));
    assert!(
        source.contains("BLE_OTA_OPERATION_MUTEX_NAME")
            && source.contains("Local\\\\Denzic.Listener.Type.BleOtaOperation")
            && source.contains("acquire_ble_ota_process_mutex(\"listener_ota_v1\")"),
        "Listener OTA v1 must hold a Windows named mutex shared by headless CLI and the installed tray process"
    );
    assert!(
        source.contains("BLE OTA operation active in another process; deferring background listener before notify open")
            && source.contains("BLE OTA operation active in another process; closing idle background listener")
            && source.contains("BACKGROUND_LISTENER_DEFERRED_FOR_OTA"),
        "background capture must not keep sending Type heartbeat or report recovery failures while another process owns Listener OTA"
    );
    // Inspect the real prepare implementation, not thin public wrappers.
    let prepare_start = source
        .find("fn prepare_listener_ota_v1_transfer_impl")
        .expect("Listener OTA v1 prepare impl should exist");
    let prepare_end = source[prepare_start..]
        .find("pub(super) fn is_ota_finish_reboot_handoff_error")
        .map(|offset| prepare_start + offset)
        .expect("Listener OTA v1 prepare impl boundary should exist");
    let prepare = &source[prepare_start..prepare_end];
    assert!(
        source.contains("BLE_OTA_PREPARATION_MUTEX_NAME")
            && source.contains("Local\\\\Denzic.Listener.Type.BleOtaPreparation")
            && prepare.contains("acquire_ble_ota_preparation_mutex(\"listener_ota_v1_prepare\")")
            && prepare.contains("PreparedListenerOtaV1TransferOwnership::Exclusive")
            && source.contains("b\"TYPE:OTA\\n\"")
            && source.contains("Listener OTA v1 reconnect handoff")
            && prepare.contains("request_listener_ota_v1_active_link(None)")
            && source.contains("LISTENER_OTA_V1_HANDOFF_DISCOVERY_RETRY_DELAYS")
            && source.contains("open_listener_ota_v1_target_for_verified_active_handoff")
            && source.contains("open_ble_device_by_address(address)")
            && source.contains("[BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached]")
            && source.contains("reconnect handoff accepted")
            && prepare.contains("open_listener_ota_v1_target_after_active_link_handoff")
            && prepare.find("acquire_ble_ota_process_mutex").unwrap()
                < prepare
                    .find("request_listener_ota_v1_active_link")
                    .unwrap()
            && prepare
                .find("request_listener_ota_v1_active_link")
                .unwrap()
                < prepare.find("BleCaptureGuard::enter").unwrap(),
        "direct Listener OTA must reserve preparation, acquire the cross-process transfer lock, send TYPE:OTA, then own capture before opening BLE GATT"
    );

    let staged_prepare_start = source
        .find("fn prepare_listener_ota_v1_transfer_staged_after_active_link_hint")
        .expect("staged Listener OTA prepare helper should exist");
    let staged_prepare_end = source[staged_prepare_start..]
        .find("fn prepare_listener_ota_v1_transfer_impl")
        .map(|offset| staged_prepare_start + offset)
        .expect("staged Listener OTA prepare helper boundary should exist");
    let staged_prepare = &source[staged_prepare_start..staged_prepare_end];
    assert!(
        staged_prepare.contains("prepare_listener_ota_v1_transfer_impl(false, false, Some(target_prepare_timeout))"),
        "staged Listener OTA must leave both transfer exclusivity gates untouched while opening its target inside the caller's bounded deadline"
    );

    // Prefer the Windows impl body (mod.rs only has a thin wrapper).
    let staged_marker = "let prepared = match prepare_listener_ota_v1_transfer_staged_after_active_link_hint(";
    let staged_marker_at = source
        .find(staged_marker)
        .expect("staged Listener OTA transfer helper should exist");
    let staged_transfer_start = source[..staged_marker_at]
        .rfind("pub fn transfer_listener_ota_v1_after_active_link_hint_staged")
        .expect("staged Listener OTA transfer helper should exist");
    let staged_transfer_end = source[staged_transfer_start..]
        .find("fn listener_ota_v1_window_chunks")
        .map(|offset| staged_transfer_start + offset)
        .expect("staged Listener OTA transfer helper boundary should exist");
    let staged_transfer = &source[staged_transfer_start..staged_transfer_end];
    let target_ready = staged_transfer
        .find("target_ready.send(Ok(()))")
        .expect("staged Listener OTA must report target readiness");
    let start_signal = staged_transfer
        .find("start_transfer\n        .recv_timeout")
        .expect("staged Listener OTA must wait for the coordinator pause signal");
    let transfer = staged_transfer
        .find("prepared.transfer(")
        .expect("staged Listener OTA must transfer only after the pause signal");
    assert!(target_ready < start_signal && start_signal < transfer);

    let prepared_transfer_start = source
        .find("impl PreparedListenerOtaV1Transfer")
        .expect("prepared Listener OTA transfer implementation should exist");
    let prepared_transfer_end = source[prepared_transfer_start..]
        .find("struct ListenerOtaV1Transport")
        .map(|offset| prepared_transfer_start + offset)
        .expect("prepared Listener OTA transfer implementation boundary should exist");
    let prepared_transfer = &source[prepared_transfer_start..prepared_transfer_end];
    let staged_ownership = prepared_transfer
        .find("PreparedListenerOtaV1TransferOwnership::Staged")
        .expect("staged Listener OTA ownership branch should exist");
    let ota_lock = prepared_transfer[staged_ownership..]
        .find("acquire_ble_ota_process_mutex(\"listener_ota_v1\")")
        .map(|offset| staged_ownership + offset)
        .expect("staged Listener OTA must acquire the actual OTA mutex after pause");
    let capture = prepared_transfer[ota_lock..]
        .find("BleCaptureGuard::enter(None)")
        .map(|offset| ota_lock + offset)
        .expect("staged Listener OTA must acquire the capture gate after the OTA mutex");
    assert!(ota_lock < capture,
        "staged Listener OTA must take the actual OTA mutex and capture gate only in its post-pause transfer branch"
    );

    let ota_handoff_start = source
        .find("fn open_listener_ota_v1_target_after_active_link_handoff")
        .expect("OTA active-link handoff helper should exist");
    let ota_handoff_end = source[ota_handoff_start..]
        .find("fn open_listener_ota_v1_target()")
        .map(|offset| ota_handoff_start + offset)
        .expect("OTA active-link handoff boundary should exist");
    let ota_handoff = &source[ota_handoff_start..ota_handoff_end];
    assert!(
        ota_handoff.contains("native_windows_hid_pairing_addresses_for_startup()")
            && ota_handoff.contains("native_windows_hid_addresses.contains(&address)")
            && ota_handoff.contains(
                "open_listener_ota_v1_target_for_current_native_windows_hid_service_endpoint("
            )
            && ota_handoff.contains("current native Windows HID service-id endpoint after direct GATT miss"),
        "OTA must reach an exact current native-HID service endpoint before any generic cache fallback"
    );

    let ota_endpoint_start = source
        .find("fn open_listener_ota_v1_target_for_current_native_windows_hid_service_endpoint(")
        .expect("native HID OTA service endpoint helper should exist");
    let ota_endpoint_end = source[ota_endpoint_start..]
        .find("fn open_listener_ota_v1_target()")
        .map(|offset| ota_endpoint_start + offset)
        .expect("native HID OTA service endpoint boundary should exist");
    let ota_endpoint = &source[ota_endpoint_start..ota_endpoint_end];
    assert!(
        ota_endpoint.contains("GetDeviceSelectorFromUuid(LISTENER_OTA_V1_SERVICE_UUID)")
            && ota_endpoint.contains("addresses.contains(&address)")
            && ota_endpoint.contains(
                "open_listener_ota_v1_target_for_service_with_cache_policy(&id, true)"
            )
            && source.contains("ota_service_endpoint_cache_policy(allow_cached)"),
        "native HID OTA endpoint must select only the current address and try uncached GATT before the legacy-compatible cache"
    );

    let direct_start = source
        .find("fn open_listener_ota_v1_target_for_device_with_options(")
        .expect("verified OTA direct-open helper should exist");
    let direct_end = source[direct_start..]
        .find("fn open_listener_ota_v1_target_for_device_with_deadline(")
        .map(|offset| direct_start + offset)
        .expect("verified OTA direct-open helper boundary should exist");
    let direct = &source[direct_start..direct_end];
    assert!(
        direct.contains("ota_device_control_cache_policy(verified_active_handoff)")
            && direct.contains("for &cache_mode in cache_modes"),
        "verified OTA handoff must not fall back to stale cached device GATT handles"
    );

    let handoff_start = source
        .find("pub(super) fn request_listener_ota_v1_active_link")
        .expect("Listener OTA v1 reconnect handoff helper should exist");
    let handoff_end = source[handoff_start..]
        .find("pub(super) fn prepare_listener_ota_v1_transfer")
        .map(|offset| handoff_start + offset)
        .expect("Listener OTA v1 reconnect handoff helper boundary should exist");
    let handoff = &source[handoff_start..handoff_end];
    assert!(
        handoff.contains("send_recording_control_command(")
            && handoff.contains("TYPE:OBS:OTA:{correlation_id:016X}")
            && handoff.contains("Duration::from_millis(300)")
            && handoff.contains("ActiveControlTransientFallback::ReturnError"),
        "OTA must use a bounded optional observability context handoff before the compatible active-link command"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn listener_ota_handoff_preflight_uses_only_the_verified_fresh_gatt_path() {
    let source = concat!(include_str!("mod.rs"), "\n", include_str!("windows_ble.rs"));
    // Skip thin mod.rs wrappers; lock onto the Windows implementation body.
    let probe_marker = "acquire_ble_ota_preparation_mutex(\"listener_ota_v1_preflight\")";
    let marker_at = source
        .find(probe_marker)
        .expect("OTA handoff preflight helper should exist");
    let probe_start = source[..marker_at]
        .rfind("pub fn listener_ota_v1_gatt_probe_after_active_link_hint")
        .expect("OTA handoff preflight helper should exist");
    let probe_end = source[probe_start..]
        .find("pub fn listener_ota_v1_service_reachable_snapshot")
        .map(|offset| probe_start + offset)
        .expect("OTA handoff preflight helper boundary should exist");
    let probe = &source[probe_start..probe_end];
    assert!(
        probe.contains("acquire_ble_ota_preparation_mutex(\"listener_ota_v1_preflight\")")
            && !probe.contains("acquire_ble_ota_process_mutex")
            && !probe.contains("BleCaptureGuard::enter")
            && probe.contains("BleFreshGattGuard::enter(\"Listener OTA v1 handoff preflight\")")
            && probe.contains("open_listener_ota_v1_target_after_active_link_handoff_with_deadline"),
        "OTA preflight must reserve preparation without interrupting notify capture and use the bounded active-link GATT path"
    );

    let target_start = source
        .find("fn open_listener_ota_v1_target_after_active_link_handoff_with_deadline")
        .expect("bounded OTA handoff target helper should exist");
    let target_end = source[target_start..]
        .find("fn open_listener_ota_v1_target_for_current_native_windows_hid_service_endpoint")
        .map(|offset| target_start + offset)
        .expect("bounded OTA handoff target helper boundary should exist");
    let target = &source[target_start..target_end];
    assert!(
        target.contains("open_listener_ota_v1_target_for_verified_active_handoff_with_deadline")
            && target.contains("open_listener_ota_v1_target_with_deadline(deadline)")
            && target.contains("deadline.saturating_duration_since(Instant::now())"),
        "OTA preflight must prefer the verified active address and keep every fallback inside its deadline"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn notify_target_open_retry_classifies_windows_gatt_transients() {
    assert!(windows_ble::is_transient_notify_target_open_error(
        "BLE characteristic discovery returned status=GattCommunicationStatus(1)"
    ));
    assert!(windows_ble::is_transient_notify_target_open_error(
        "BLE characteristic discovery returned status=GattCommunicationStatus(3)"
    ));
    assert!(windows_ble::is_transient_notify_target_open_error(
        "BLE service open wait failed: HRESULT(0x800706BA)"
    ));
    assert!(windows_ble::is_transient_notify_target_open_error(
        "BLE Uncached service discovery wait failed: Some(HRESULT(0x80070016))"
    ));
    assert!(windows_ble::is_transient_notify_target_open_error(
        "BLE BluetoothCacheMode(1) service discovery wait failed: BLE BluetoothCacheMode(1) service timed out after 600 ms"
    ));
    assert!(!windows_ble::is_transient_notify_target_open_error(
        "Linda: BLE device path A4CB8FF2B512 failed: BluetoothCacheMode(0): BLE GATT session did not become active after 8000 ms; advertisement fallback failed: No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es) D4E8768AB2EE; skipping audio notify advertisement GATT fallback until Windows pairing completes"
    ));
    assert!(!windows_ble::is_transient_notify_target_open_error(
        "notify characteristic not found"
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn pnp_device_instance_normalizer_extracts_bthle_devnode() {
    assert_eq!(
        windows_ble::normalize_pnp_device_instance_id(
            r#"\\?\BTHLE#DEV_D41A50FBF35E#9&B465B9E&0&D41A50FBF35E#{0000180a-0000-1000-8000-00805f9b34fb}"#
        ),
        Some(r#"BTHLE\DEV_D41A50FBF35E\9&B465B9E&0&D41A50FBF35E"#.to_string())
    );
    assert_eq!(
        windows_ble::normalize_pnp_device_instance_id(
            r#"BTHLE\DEV_D41A50FBF35E\9&B465B9E&0&D41A50FBF35E"#
        ),
        Some(r#"BTHLE\DEV_D41A50FBF35E\9&B465B9E&0&D41A50FBF35E"#.to_string())
    );
    assert_eq!(
        windows_ble::normalize_pnp_device_instance_id(r#"USB\VID_0000&PID_0000"#),
        None
    );
}

#[cfg(target_os = "windows")]
#[test]
fn pnp_device_instance_normalizer_keeps_btledevice_and_hid_children() {
    assert_eq!(
        windows_ble::normalize_pnp_device_instance_id(
            r#"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#
        ),
        Some(
            r#"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#
                .to_string()
        )
    );
    assert_eq!(
        windows_ble::normalize_pnp_device_instance_id(
            r#"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D&COL01\A&220D8BA7&0&0000"#
        ),
        Some(
            r#"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D&COL01\A&220D8BA7&0&0000"#
                .to_string()
        )
    );
}

#[cfg(target_os = "windows")]
#[test]
fn pnp_listener_service_signature_finds_arbitrary_renamed_device_tree() {
    let service_id = r#"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3091A}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#;
    assert!(windows_ble::pnp_instance_has_listener_service_signature(
        service_id
    ));
    assert_eq!(
        windows_ble::parse_bluetooth_address_from_device_id(service_id),
        Some(0xFD2F_988D_B40D)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn pnp_cleanup_match_uses_service_signature_without_name_fallback() {
    let service_entry = windows_ble::ListenerPnpEntry {
        name: "Bluetooth LE GATT Service".to_string(),
        instance_id:
            r#"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3092A}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D\9&2E60A20C&0&004B"#
                .to_string(),
        address: Some(0xFD2F_988D_B40D),
        has_listener_service_signature: true,
        is_ble_device_root: false,
        is_listener_hid_keyboard: false,
    };
    assert!(windows_ble::listener_pnp_entry_matches_cleanup(
        &service_entry,
        false,
        false,
        false
    ));
    assert!(!windows_ble::listener_pnp_entry_matches_cleanup(
        &service_entry,
        false,
        false,
        true
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn pnp_ble_device_root_detection_is_strict() {
    assert!(windows_ble::pnp_instance_is_ble_device_root(
        r#"BTHLE\DEV_FD2F988DB40D\8&25948282&0&FD2F988DB40D"#
    ));
    assert!(!windows_ble::pnp_instance_is_ble_device_root(
        r#"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}_FD2F988DB40D\9&2E60A20C&0&004B"#
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn native_windows_hid_pairing_requires_matching_listener_root_and_keyboard() {
    let address = 0xFD2F_988D_B40D;
    let entries = vec![
        windows_ble::ListenerPnpEntry {
            name: "listener".to_string(),
            instance_id: r"BTHLE\DEV_FD2F988DB40D\8&25948282&0&FD2F988DB40D".to_string(),
            address: Some(address),
            has_listener_service_signature: false,
            is_ble_device_root: true,
            is_listener_hid_keyboard: false,
        },
        windows_ble::ListenerPnpEntry {
            name: "HID Keyboard Device".to_string(),
            instance_id: r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_FD2F988DB40D&COL01\A&220D8BA7&0&0000".to_string(),
            address: Some(address),
            has_listener_service_signature: false,
            is_ble_device_root: false,
            is_listener_hid_keyboard: true,
        },
        windows_ble::ListenerPnpEntry {
            name: "HID Keyboard Device".to_string(),
            instance_id: r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_REV&0001_14C19F48FE72&COL01\A&220D8BA7&0&0000".to_string(),
            address: Some(0x14C1_9F48_FE72),
            has_listener_service_signature: false,
            is_ble_device_root: false,
            is_listener_hid_keyboard: true,
        },
    ];
    assert_eq!(
        windows_ble::native_windows_hid_pairing_addresses_from_entries(&entries),
        vec![address]
    );
    assert!(windows_ble::pnp_instance_is_listener_hid_keyboard(
        &entries[1].instance_id
    ));
    assert!(!windows_ble::pnp_instance_is_listener_hid_keyboard(
        &entries[1].instance_id.replace("&COL01\\", "&COL02\\")
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn bthport_device_name_decoder_accepts_ascii_and_utf16() {
    assert_eq!(
        windows_ble::decode_bthport_device_name(b"Blistener\0\0"),
        "Blistener"
    );
    assert_eq!(
        windows_ble::decode_bthport_device_name(&[
            b'l', 0, b'i', 0, b's', 0, b't', 0, b'e', 0, b'n', 0, b'e', 0, b'r', 0, b'B', 0, 0,
            0,
        ]),
        "listenerB"
    );
}

#[test]
fn ble_failure_taxonomy_covers_customer_recovery_cases() {
    let cases = [
        (
            "Embedded audio BLE service not found; ensure device is paired and online",
            BleFailureKind::DeviceMissing,
            false,
        ),
        (
            "Listener BLE device asleep; press KEY4 wake key before retry",
            BleFailureKind::DeviceAsleep,
            false,
        ),
        (
            "No paired BLE device found in Windows Bluetooth pairing store",
            BleFailureKind::MissingPairing,
            false,
        ),
        (
            "No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es) A4CB8FF2B512; skipping audio notify advertisement GATT fallback until Windows pairing completes",
            BleFailureKind::MissingPairing,
            false,
        ),
        (
            "BLE idle disconnect reason=546 produced transport_not_ready before reconnect",
            BleFailureKind::LowPowerIdleDisconnect,
            true,
        ),
        (
            "BLE device connection status changed to Disconnected; transport_not_ready",
            BleFailureKind::PairedButDisconnected,
            true,
        ),
        (
            "BLE validation injected disconnect through notify wait; transport_not_ready",
            BleFailureKind::PairedButDisconnected,
            true,
        ),
        (
            "stale cached GATT path after BLE reason=546 returned transport_not_ready",
            BleFailureKind::StaleGattService,
            true,
        ),
        (
            "BLE Uncached service discovery returned status=Unreachable after timeout",
            BleFailureKind::PairedButDisconnected,
            true,
        ),
        (
            "Unknown GATT service from stale cached service table",
            BleFailureKind::StaleGattService,
            true,
        ),
        (
            "BLE Uncached service discovery wait failed: Some(HRESULT(0x80070016))",
            BleFailureKind::StaleGattService,
            true,
        ),
        (
            "BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected",
            BleFailureKind::StaleGattService,
            true,
        ),
        (
            "BLE CCCD notify write returned status=ProtocolError protocol_error=3",
            BleFailureKind::CccdProtocolError,
            true,
        ),
        (
            "DIS firmware revision missing from preflight snapshot",
            BleFailureKind::MissingDisFirmwareRevision,
            false,
        ),
        (
            "background listener already active; foreground probe skipped",
            BleFailureKind::BackgroundListenerContention,
            true,
        ),
        (
            "OTA reboot window: version confirm failed after OTA",
            BleFailureKind::OtaRebootWindow,
            true,
        ),
        (
            "Windows Bluetooth service reset needed after adapter radio error",
            BleFailureKind::WindowsBluetoothServiceResetNeeded,
            false,
        ),
    ];

    for (message, expected_kind, expected_auto) in cases {
        let classification = classify_ble_failure(message);
        assert_eq!(classification.kind, expected_kind, "{message}");
        assert_eq!(
            classification.automatic_recovery, expected_auto,
            "{message}"
        );
        assert!(!classification.user_action.is_empty());
    }
}

#[cfg(target_os = "windows")]
#[test]
fn ota_sync_uses_lightweight_response_status_path_only() {
    let mut sync = [0u8; denzic_ota_core::CONTROL_BYTES];
    sync[4] = denzic_ota_core::OP_SYNC;
    assert!(super::windows_ble::listener_ota_v1_sync_control_uses_status_write(&sync));

    for operation in [
        denzic_ota_core::OP_BEGIN,
        denzic_ota_core::OP_FINISH,
        denzic_ota_core::OP_ABORT,
    ] {
        let mut control = [0u8; denzic_ota_core::CONTROL_BYTES];
        control[4] = operation;
        assert!(
            !super::windows_ble::listener_ota_v1_sync_control_uses_status_write(&control),
            "only SYNC may use the lightweight OTA control response path"
        );
    }
}

#[cfg(target_os = "windows")]
#[test]
fn ota_finish_reboot_handoff_accepts_windows_ble_disconnect_errors() {
    assert!(super::windows_ble::is_ota_finish_reboot_handoff_error(
        "BLE OTA control finish write async error: Some(HRESULT(0x800704C7))"
    ));
    assert!(super::windows_ble::is_ota_finish_reboot_handoff_error(
        "BLE OTA control finish write async error: Some(HRESULT(0x800706BA))"
    ));
    assert!(super::windows_ble::is_ota_finish_reboot_handoff_error(
        "BLE device connection status changed to Disconnected; transport_not_ready"
    ));
    assert!(!super::windows_ble::is_ota_finish_reboot_handoff_error(
        "BLE OTA data write async error: Some(HRESULT(0x80070057))"
    ));
    assert!(!super::windows_ble::is_ota_finish_reboot_handoff_error(
        "BLE OTA control begin write returned status=GattCommunicationStatus(1)"
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn parses_bluetooth_address_from_service_instance_id() {
    assert_eq!(
        super::windows_ble::parse_bluetooth_address_from_device_id(
            r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3091A}_DCB4D91112CE"
        ),
        Some(0xDCB4_D911_12CE)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn parses_bluetooth_address_from_device_instance_id() {
    assert_eq!(
        super::windows_ble::parse_bluetooth_address_from_device_id(
            r"BTHLE\DEV_DCB4D91112CE\7&29C9821A&0&0000"
        ),
        Some(0xDCB4_D911_12CE)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn parses_bluetooth_address_from_bluetooth_le_selector_id() {
    assert_eq!(
        super::windows_ble::parse_bluetooth_address_from_device_id(
            r"BluetoothLE#BluetoothLE00:11:22:33:44:55-D4:1A:50:FB:F3:5E"
        ),
        Some(0xD41A_50FB_F35E)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn parses_bluetooth_address_from_vid_pid_service_instance_id() {
    assert_eq!(
        super::windows_ble::parse_bluetooth_address_from_device_id(
            r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3092A}_DEV_VID&0216C0_PID&05DF_REV&0001_14C19F48FE72\A&B5FDFC&D&0009"
        ),
        Some(0x14C1_9F48_FE72)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn parses_configured_bluetooth_address_hex_forms() {
    assert_eq!(
        super::windows_ble::parse_bluetooth_address_hex("D41A50FBF35E"),
        Some(0xD41A_50FB_F35E)
    );
    assert_eq!(
        super::windows_ble::parse_bluetooth_address_hex("D4:1A:50:FB:F3:5E"),
        Some(0xD41A_50FB_F35E)
    );
    assert_eq!(
        super::windows_ble::parse_bluetooth_address_hex("D4-1A-50-FB-F3-5E"),
        Some(0xD41A_50FB_F35E)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn device_settings_serial_candidates_reject_single_stlink_port() {
    let ports = vec![usb_serial_port(
        "COM13",
        0x0483,
        0x374b,
        "STMicroelectronics",
        "STLink Virtual COM Port",
    )];

    assert!(super::windows_ble::listener_usb_serial_candidates(&ports).is_empty());
}

#[cfg(target_os = "windows")]
#[test]
fn device_settings_serial_candidates_keep_listener_esp32_port() {
    let ports = vec![
        usb_serial_port(
            "COM13",
            0x0483,
            0x374b,
            "STMicroelectronics",
            "STLink Virtual COM Port",
        ),
        usb_serial_port("COM11", 0x303a, 0x1001, "Microsoft", "USB Serial Device"),
    ];

    let candidates = super::windows_ble::listener_usb_serial_candidates(&ports);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].port_name, "COM11");
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires a paired or USB-connected Listener device"]
fn device_settings_command_hardware_smoke() {
    let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
        .lock()
        .expect("device settings hardware test mutex poisoned");
    let command = std::env::var("LISTENER_DEVICE_SETTINGS_COMMAND")
        .unwrap_or_else(|_| "DEVICE:SET knob_rotation=system_volume".to_string());
    super::windows_ble::send_device_settings_command(&command, Duration::from_secs(4))
        .expect("device settings command should be acknowledged by firmware");
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires a USB-connected or paired Listener device"]
fn ble_name_recovery_hardware_smoke() {
    let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
        .lock()
        .expect("device settings hardware test mutex poisoned");
    super::windows_ble::send_recording_control_recovery(Duration::from_secs(4))
        .expect("BLE recovery control should be sent to firmware");
    std::thread::sleep(Duration::from_secs(2));
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "renames the paired Listener hardware without clearing Windows Bluetooth cache"]
fn ble_name_apply_hardware_roundtrip() {
    let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
        .lock()
        .expect("device settings hardware test mutex poisoned");
    let old_name = std::env::var("LISTENER_BLE_NAME_RENAME_OLD")
        .expect("set LISTENER_BLE_NAME_RENAME_OLD to the currently advertised BLE name");
    let target_name = std::env::var("LISTENER_BLE_NAME_RENAME_TARGET")
        .expect("set LISTENER_BLE_NAME_RENAME_TARGET to the desired BLE name");

    super::windows_ble::set_configured_bluetooth_target_name(&old_name);
    let learned_address =
        super::windows_ble::remember_current_bluetooth_target_address_for_name(
            &target_name,
            Duration::from_secs(4),
            "BLE name hardware test",
        );
    assert!(
        learned_address.is_some(),
        "hardware test should learn current Listener address before renaming old={old_name} target={target_name}"
    );
    super::windows_ble::send_device_settings_command(
        &format!("DEVICE:SET ble_name={target_name}"),
        Duration::from_secs(4),
    )
    .expect("BLE name write should be acknowledged by firmware");
    super::windows_ble::apply_pending_ble_name(Duration::from_secs(4))
        .expect("BLE name apply should refresh advertising without opening pairing recovery");
    std::thread::sleep(Duration::from_secs(2));

    super::windows_ble::set_configured_bluetooth_target_name(&target_name);
    let status = super::windows_ble::read_embedded_audio_status(Duration::from_secs(20))
        .expect("renamed Listener BLE audio status should be reachable");
    assert!(
        status.connected,
        "renamed Listener BLE status should be connected"
    );
    let settings = super::windows_ble::read_device_settings_status(Duration::from_secs(4))
        .expect("renamed Listener device settings should be readable");
    assert_eq!(settings.ble_name, target_name);
    assert!(
        !settings.ble_name_pending_restart,
        "firmware should report the refreshed BLE name as applied"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn parses_device_settings_status_line_preserves_ble_name_order() {
    let status = super::windows_ble::parse_device_settings_status_line(
        "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=external low_power_idle_ms=60000 knob_rotation=screen_brightness ble_name=\"Blistener\" ble_name_pending=0 external_power_present=1 usb_power_present=1 charging=0 charge_full=1"
    )
    .expect("parse Blistener device settings");

    assert_eq!(status.ble_name, "Blistener");
    assert_ne!(status.ble_name, "listenerB");
}

#[test]
fn parses_device_settings_revision_characteristic() {
    assert_eq!(
        parse_device_settings_revision_characteristic(
            "schema=listener.device_settings.v1;settings_revision=42"
        )
        .expect("settings revision"),
        42
    );
    assert!(parse_device_settings_revision_characteristic("settings_revision=0").is_err());
}

#[cfg(target_os = "windows")]
#[test]
fn parses_device_settings_ms_jitter_without_rounding_up_full_minute() {
    for minutes in [0, 1, 2, 3, 5, 12, 47, 1439, 1440] {
        let extra_ms_values: &[u32] = if minutes == 0 {
            &[0]
        } else {
            &[0, 1, 17_321, 59_999]
        };
        for extra_ms in extra_ms_values {
            let jittered_ms = minutes * 60_000 + extra_ms;
            let status = super::windows_ble::parse_device_settings_status_line(&format!(
                "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=battery low_power_idle_ms={jittered_ms} plugged_low_power_idle_ms={jittered_ms} battery_low_power_idle_ms={jittered_ms} plugged_low_power_enabled=1 auto_shutdown_ms={jittered_ms} plugged_auto_shutdown_ms=0 battery_auto_shutdown_ms={jittered_ms} knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=0 external_power_present=0 usb_power_present=0 charging=0 charge_full=0"
            ))
            .expect("parse device settings with millisecond jitter");

            assert_eq!(status.low_power_idle_minutes, minutes);
            assert_eq!(status.plugged_low_power_idle_minutes, minutes);
            assert_eq!(status.battery_low_power_idle_minutes, minutes);
            assert_eq!(status.plugged_auto_shutdown_minutes, 0);
            assert_eq!(status.battery_auto_shutdown_minutes, minutes);
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn parses_device_settings_status_line() {
    let status = super::windows_ble::parse_device_settings_status_line(
        "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK plugged_brightness=80 battery_brightness=50 active_power=external active_brightness=80 led_status=70 led_key=65 led_ec11=60 led_edge=55 compact_set=1 low_power_idle_ms=60000 plugged_low_power_idle_ms=120000 battery_low_power_idle_minutes=3 plugged_low_power_enabled=1 voice_auto_start=1 voice_auto_stop=1 low_power_idle_mode=power_mode auto_shutdown_ms=1800000 plugged_auto_shutdown_ms=0 battery_auto_shutdown_minutes=45 auto_shutdown_mode=power_mode knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=1 ble_name_apply=restart_ble_or_reboot loaded_from_nvs=1 external_power_present=1 usb_power_present=1 charging=0 charge_full=1 valid_ranges=brightness_0_100,led_zone_brightness_0_100,low_power_idle_ms_0_86400000"
    )
    .expect("parse device settings");
    assert_eq!(status.brightness_percent, 80);
    assert_eq!(status.plugged_brightness_percent, 80);
    assert_eq!(status.battery_brightness_percent, 50);
    assert_eq!(status.status_led_brightness_percent, 70);
    assert_eq!(status.key_led_brightness_percent, 65);
    assert_eq!(status.knob_led_brightness_percent, 60);
    assert_eq!(status.edge_led_brightness_percent, 55);
    assert!(status.led_zone_brightness_supported);
    assert!(status.compact_set_supported);
    assert_eq!(status.low_power_idle_minutes, 2);
    assert_eq!(status.plugged_low_power_idle_minutes, 2);
    assert_eq!(status.battery_low_power_idle_minutes, 3);
    assert!(status.plugged_low_power_enabled);
    assert!(status.voice_auto_start_enabled);
    assert!(status.voice_auto_stop_enabled);
    assert_eq!(status.plugged_auto_shutdown_minutes, 0);
    assert_eq!(status.battery_auto_shutdown_minutes, 45);
    assert_eq!(status.knob_rotation_action, "screen_brightness");
    assert_eq!(status.ble_name, "listener-dev");
    assert!(status.ble_name_pending_restart);
    assert!(status.external_power_present);
    assert!(status.usb_power_present);
    assert!(!status.charging);
    assert!(status.charge_full);
}

#[cfg(target_os = "windows")]
#[test]
fn parses_legacy_device_settings_status_without_led_zone_brightness() {
    let status = super::windows_ble::parse_device_settings_status_line(
        "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=external low_power_idle_ms=60000 knob_rotation=screen_brightness ble_name=\"listener-dev\" ble_name_pending=0 external_power_present=1 usb_power_present=1 charging=0 charge_full=1"
    )
    .expect("parse legacy device settings");
    assert_eq!(status.brightness_percent, 100);
    assert_eq!(status.plugged_brightness_percent, 100);
    assert_eq!(status.battery_brightness_percent, 100);
    assert_eq!(status.status_led_brightness_percent, 50);
    assert_eq!(status.key_led_brightness_percent, 80);
    assert_eq!(status.knob_led_brightness_percent, 100);
    assert_eq!(status.edge_led_brightness_percent, 100);
    assert_eq!(status.plugged_low_power_idle_minutes, 1);
    assert_eq!(status.battery_low_power_idle_minutes, 1);
    assert!(!status.plugged_low_power_enabled);
    assert!(!status.led_zone_brightness_supported);
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires a USB-connected Listener device"]
fn device_settings_status_refresh_hardware_smoke() {
    let _guard = DEVICE_SETTINGS_HARDWARE_TEST_LOCK
        .lock()
        .expect("device settings hardware test mutex poisoned");
    let status = super::windows_ble::read_device_settings_status(Duration::from_secs(4))
        .expect("device settings status should be read from firmware");
    assert!(status.brightness_percent <= 100);
    assert!(status.plugged_brightness_percent <= 100);
    assert!(status.battery_brightness_percent <= 100);
    assert!(!status.ble_name.is_empty());
    assert!(status.status_led_brightness_percent <= 100);
    assert!(status.key_led_brightness_percent <= 100);
    assert!(status.knob_led_brightness_percent <= 100);
    assert!(status.edge_led_brightness_percent <= 100);
}

#[cfg(target_os = "windows")]
#[test]
fn parses_zero_low_power_idle_as_disabled() {
    let status = super::windows_ble::parse_device_settings_status_line(
        "~DEVICE:SETTINGS schema=listener.device_settings.v1 result=OK active_power=external low_power_idle_ms=0 plugged_low_power_idle_ms=0 battery_low_power_idle_ms=0 plugged_low_power_enabled=0 knob_rotation=system_volume ble_name=\"listener-dev\" ble_name_pending=0 external_power_present=1 usb_power_present=1 charging=0 charge_full=0"
    )
    .expect("parse zero low-power device settings");

    assert_eq!(status.low_power_idle_minutes, 0);
    assert_eq!(status.plugged_low_power_idle_minutes, 0);
    assert_eq!(status.battery_low_power_idle_minutes, 0);
    assert!(!status.plugged_low_power_enabled);
}

#[test]
fn crc32_matches_standard_vector() {
    assert_eq!(denzic_ota_core::crc32_ieee(b"123456789"), 0xcbf4_3926);
    assert_eq!(format_crc32(0xcbf4_3926), "0xcbf43926");
}
