use super::dictation::abort_recording_with_error;
use super::*;
use crate::types::{DictationInputSource, HotkeyMode, HotkeyTrigger};
use once_cell::sync::Lazy;

macro_rules! include_str {
    ("coordinator.rs") => {
        concat!(
            std::include_str!("coordinator.rs"),
            "\n",
            std::include_str!("coordinator/hotkey_device_runtime.rs"),
            "\n",
            std::include_str!("coordinator/embedded_ble_runtime.rs")
        )
        .replace("\r\n", "\n")
    };
    ("embedded_ble.rs") => {
        concat!(
            std::include_str!("embedded_ble/mod.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/mod.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/ota_transfer.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/pairing.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/pnp_cache.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/recording_control.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/capture_events.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/notify_open.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/unpair.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/ota_open.rs"),
            "\n",
            std::include_str!("embedded_ble/windows_ble/gatt_open.rs")
        )
        .replace("\r\n", "\n")
    };
}

static ENV_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

fn temp_path_for_test(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    let suffix = format!("{}-{}", std::process::id(), Uuid::new_v4());
    let file_name = match path.extension().and_then(|value| value.to_str()) {
        Some(ext) => format!("listener-type-{stem}-{suffix}.{ext}"),
        None => format!("listener-type-{stem}-{suffix}"),
    };
    std::env::temp_dir().join(file_name)
}

fn session_id(n: u128) -> SessionId {
    Uuid::from_u128(n)
}

#[test]
fn device_custom_keys_do_not_register_parallel_global_hotkeys() {
    let coordinator = Coordinator::new();

    coordinator.start_device_custom_key_hotkey_listeners();
    coordinator.update_device_custom_key_hotkey_bindings();

    assert!(
        coordinator
            .inner
            .device_key_hotkeys
            .iter()
            .all(|slot| slot.lock().is_none()),
        "device-reserved fallback keys must use the direct low-level hook only"
    );
}

#[test]
fn embedded_ble_startup_syncs_firmware_name_before_listener_refresh() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("pub fn auto_select_embedded_ble_input_source_in_background")
        .expect("startup BLE helper should exist");
    let end = source[start..]
        .find("pub fn request_shutdown")
        .map(|offset| start + offset)
        .expect("startup BLE helper boundary should exist");
    let body = &source[start..end];
    let existing_source_start = body
        .find("if prefs.dictation_input_source == DictationInputSource::EmbeddedBle")
        .expect("existing embedded BLE source branch should exist");
    let existing_source_end = body[existing_source_start..]
        .find("return;")
        .map(|offset| existing_source_start + offset)
        .expect("existing embedded BLE source branch should return after refresh");
    let existing_source_body = &body[existing_source_start..existing_source_end];
    // Existing EmbeddedBle source uses a reconnect fast-path: open notify on the
    // persisted target first, then polish power/name. Full firmware name sync
    // still runs on the auto-select probe path (non-overridden other sources).
    let mark_done_index = existing_source_body
        .find("mark_startup_ble_name_sync_done")
        .expect("existing EmbeddedBle startup must open the name-sync gate");
    let refresh_index = existing_source_body
        .find("refresh_embedded_ble_listener")
        .expect("existing EmbeddedBle startup must refresh listener");
    let polish_index = existing_source_body
        .find("polish_startup_ble_settings_after_fast_open")
        .expect("existing EmbeddedBle startup must polish settings after fast open");
    assert!(
        mark_done_index < refresh_index && refresh_index < polish_index,
        "startup EmbeddedBle reconnect must mark name-sync done, open listener, then polish"
    );
    assert!(
        body.contains("sync_device_ble_name_from_firmware_settings"),
        "startup BLE helper must still have a firmware BLE name sync path for auto-select probe"
    );
    assert!(
        !existing_source_body.contains("firmware_ota_device_snapshot"),
        "an already-selected Listener BLE source must start the notify listener without a startup OTA GATT snapshot"
    );
    assert!(
        source.contains("fn record_embedded_ble_device_settings_power_status"),
        "startup should cache power context from DEVICE:SETTINGS instead of the OTA service"
    );
}

#[test]
fn shutdown_sends_type_bye_before_background_listener_cancel() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("pub fn request_shutdown")
        .expect("shutdown helper should exist");
    let end = source[start..]
        .find("pub fn start_hotkey_listener")
        .map(|offset| start + offset)
        .expect("shutdown helper boundary should exist");
    let body = &source[start..end];
    let bye_index = body
        .find("send_recording_control_type_bye")
        .expect("shutdown must send Type heartbeat bye");
    let cancel_index = body
        .find("cancel_embedded_ble_listener_capture")
        .expect("shutdown must cancel the background listener");
    let release_index = body
        .find("wait_for_embedded_ble_listener_shutdown_release")
        .expect("shutdown must wait for the real WinRT notify owner");

    assert!(
        body.contains("Duration::from_millis(250)"),
        "explicit tray quit must use a bounded fast bye, not the normal BLE reconnect window"
    );
    assert!(
        bye_index < cancel_index,
        "Type exit must clear firmware TYPE_READY before tearing down the background listener"
    );
    assert!(
        cancel_index < release_index,
        "Type exit must request cancellation before waiting for the notify owner"
    );
    assert!(
        body.contains("Duration::from_secs(3)"),
        "shutdown release wait must stay bounded"
    );
}

#[test]
fn embedded_ble_refresh_waits_for_startup_name_sync_gate() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("\nfn refresh_embedded_ble_listener")
        .map(|offset| offset + 1)
        .expect("refresh helper should exist");
    let end = source[start..]
        .find("fn embedded_ble_wake_recovery_snapshot")
        .map(|offset| start + offset)
        .expect("refresh helper boundary should exist");
    let body = &source[start..end];
    let gate_index = body
        .find("embedded_ble_startup_name_sync_done")
        .expect("refresh helper must check startup BLE name sync gate");
    let generation_index = body
        .find("fetch_add")
        .expect("refresh helper should bump listener generation");

    assert!(
        gate_index < generation_index,
        "refresh must not spawn the background BLE listener before startup name sync opens the gate"
    );
}

#[test]
fn device_knob_rotation_sync_uses_active_capture_without_fresh_gatt_fallback() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn sync_device_knob_rotation_action_to_firmware")
        .expect("device knob rotation sync helper should exist");
    let end = source[start..]
        .find("fn record_embedded_ble_listener_cancelled")
        .map(|offset| start + offset)
        .expect("device knob rotation sync helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("send_device_settings_command_via_active_capture_only"),
        "startup/settings-save knob sync must use the existing BLE audio-control sender"
    );
    assert!(
        !body.contains("send_device_settings_command(&command"),
        "knob sync must not open a competing fresh GATT settings path while notify is connecting"
    );
    assert!(
        !body.contains("send_ec11_rotation_mode"),
        "knob sync must not fall back to legacy EC11 control during BLE startup"
    );
}

#[tokio::test]
async fn extra_asr_hotwords_env_splits_and_enables_phrases() {
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var(
        EXTRA_ASR_HOTWORDS_ENV,
        "打开设置, 新建文件;撤销操作|打开设置\nCompanion",
    );

    let mut hotwords = vec![DictionaryHotword {
        phrase: "打开设置".to_string(),
        enabled: false,
    }];
    append_extra_asr_hotwords(&mut hotwords);

    let enabled: Vec<&str> = hotwords
        .iter()
        .filter(|entry| entry.enabled)
        .map(|entry| entry.phrase.as_str())
        .collect();
    assert_eq!(
        enabled,
        vec!["打开设置", "新建文件", "撤销操作", "Companion"]
    );

    std::env::remove_var(EXTRA_ASR_HOTWORDS_ENV);
}

#[test]
fn external_app_path_validation_rejects_command_text() {
    assert!(validate_external_app_path("code").is_err());
    assert!(validate_external_app_path("cmd /C start notepad").is_err());
}

#[test]
fn external_app_path_validation_rejects_script_files() {
    #[cfg(target_os = "windows")]
    let path = temp_path_for_test("device-key-script.cmd");
    #[cfg(target_os = "macos")]
    let path = temp_path_for_test("device-key-script.command");
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let path = temp_path_for_test("device-key-script.sh");

    std::fs::write(&path, b"echo unsafe").unwrap();
    let result = validate_external_app_path(&path.display().to_string());
    let _ = std::fs::remove_file(&path);

    assert!(result.is_err());
}

#[test]
fn external_app_path_validation_accepts_platform_app_entry() {
    #[cfg(target_os = "windows")]
    {
        let path = temp_path_for_test("device-key-app.exe");
        std::fs::write(&path, b"").unwrap();
        let result = validate_external_app_path(&path.display().to_string());
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok());
    }

    #[cfg(target_os = "macos")]
    {
        let path = temp_path_for_test("DeviceKeyTest.app");
        std::fs::create_dir(&path).unwrap();
        let result = validate_external_app_path(&path.display().to_string());
        let _ = std::fs::remove_dir(&path);
        assert!(result.is_ok());
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let path = temp_path_for_test("device-key-app.desktop");
        std::fs::write(&path, b"[Desktop Entry]\nType=Application\nName=Test\n").unwrap();
        let result = validate_external_app_path(&path.display().to_string());
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok());
    }
}

fn force_microphone_input_for_test(coordinator: &Coordinator) {
    let mut prefs = coordinator.inner.prefs.get();
    prefs.dictation_input_source = DictationInputSource::Microphone;
    coordinator.inner.prefs.replace_for_tests(prefs);
}

fn open_startup_ble_name_sync_gate_for_test(coordinator: &Coordinator) {
    coordinator
        .inner
        .embedded_ble_startup_name_sync_done
        .store(true, Ordering::SeqCst);
}

fn force_embedded_ble_input_for_test(coordinator: &Coordinator) {
    let mut prefs = coordinator.inner.prefs.get();
    prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    coordinator.inner.prefs.replace_for_tests(prefs);
    open_startup_ble_name_sync_gate_for_test(coordinator);
}

fn firmware_snapshot_for_auto_input_test(
    connected: bool,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected,
        hardware_revision: connected.then(|| "keyboard-v2".to_string()),
        firmware_version: connected.then(|| "v-test".to_string()),
        capabilities: connected
            .then(|| vec![crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY.to_string()])
            .unwrap_or_default(),
        battery_percent: connected.then_some(91),
        usb_powered: connected.then_some(true),
        detail: (!connected).then(|| "No paired Listener device".to_string()),
    }
}

#[test]
fn embedded_ble_auto_input_source_prefers_connected_device() {
    let mut prefs = crate::types::UserPreferences {
        dictation_input_source: DictationInputSource::Microphone,
        ..crate::types::UserPreferences::default()
    };
    assert!(should_auto_select_embedded_ble_input_source(
        &prefs,
        &firmware_snapshot_for_auto_input_test(true)
    ));

    prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    assert!(!should_auto_select_embedded_ble_input_source(
        &prefs,
        &firmware_snapshot_for_auto_input_test(true)
    ));

    prefs.dictation_input_source = DictationInputSource::Microphone;
    assert!(!should_auto_select_embedded_ble_input_source(
        &prefs,
        &firmware_snapshot_for_auto_input_test(false)
    ));

    prefs.dictation_input_source_user_overridden = true;
    assert!(!should_auto_select_embedded_ble_input_source(
        &prefs,
        &firmware_snapshot_for_auto_input_test(true)
    ));
}

#[test]
fn embedded_ble_pairing_prompt_waits_after_windows_user_action_failure() {
    let pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 1,
        open_bluetooth_settings: true,
        details: vec!["Windows custom pairing returned status=Failed".to_string()],
    };

    assert!(embedded_ble_pairing_prompt_waiting_for_windows(Some(
        &pairing
    )));
    assert!(!embedded_ble_failed_recovery_pairing_should_retry_soon(
        Some(&pairing),
        crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
            visible: true,
            has_random_identity: false,
            addresses: Vec::new(),
        }
    ));
    assert!(!embedded_ble_pairing_prompt_ready(&pairing));
}

#[test]
fn embedded_ble_random_identity_pairing_failure_retries_soon() {
    let pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 1,
        open_bluetooth_settings: true,
        details: vec!["Windows custom pairing returned status=Failed".to_string()],
    };

    assert!(embedded_ble_failed_recovery_pairing_should_retry_soon(
        Some(&pairing),
        crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
            visible: true,
            has_random_identity: true,
            addresses: Vec::new(),
        }
    ));
}

#[test]
fn recovery_pairing_cleanup_keeps_scanned_address_when_error_has_none() {
    let probe = crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
        visible: true,
        has_random_identity: true,
        addresses: vec![0xA4CB_8FF2_B512],
    };

    assert_eq!(
        recovery_pairing_addresses_for_cleanup(
            "BLE device connection status changed to Disconnected; transport_not_ready",
            &probe,
        ),
        vec![0xA4CB_8FF2_B512],
        "a physical recovery advertisement must carry its observed address into local stale-cache cleanup even when the prior transport error did not spell out an address"
    );
}

#[test]
fn embedded_ble_pairing_prompt_ready_does_not_wait_for_windows() {
    let pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 1,
        failed_devices: 0,
        open_bluetooth_settings: false,
        details: vec!["Listener is paired".to_string()],
    };

    assert!(embedded_ble_pairing_prompt_ready(&pairing));
    assert!(!embedded_ble_pairing_prompt_waiting_for_windows(Some(
        &pairing
    )));
}

#[test]
fn device_key_dictation_debounce_matches_hotkey_edge_debounce() {
    let coordinator = Coordinator::new();
    let mapping = DeviceCustomKeyMapping {
        action: DeviceCustomKeyAction::Dictation,
        ..DeviceCustomKeyMapping::default()
    };

    assert_eq!(HOTKEY_DEBOUNCE, Duration::from_millis(250));
    assert!(!device_key_action_debounced(
        &coordinator.inner,
        DeviceCustomKeyId::Key3,
        DeviceCustomKeyGesture::SingleClick,
        &mapping
    ));
    assert!(device_key_action_debounced(
        &coordinator.inner,
        DeviceCustomKeyId::Key3,
        DeviceCustomKeyGesture::SingleClick,
        &mapping
    ));

    coordinator.inner.device_key_last_dispatch_at.lock().insert(
        (DeviceCustomKeyGesture::SingleClick, DeviceCustomKeyId::Key3),
        Instant::now() - HOTKEY_DEBOUNCE - Duration::from_millis(1),
    );
    assert!(!device_key_action_debounced(
        &coordinator.inner,
        DeviceCustomKeyId::Key3,
        DeviceCustomKeyGesture::SingleClick,
        &mapping
    ));
}

#[test]
fn device_key_ble_recording_control_feedback_uses_active_session() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    let session = current_device_key_recording_control_session(&coordinator.inner);
    assert_eq!(session, Some((session_id, SessionPhase::Listening)));
    assert_eq!(
        emit_device_key_recording_control_capsule(
            &coordinator.inner,
            session,
            DictationUiState::Transcribing,
            CapsuleState::Reconnecting,
            "正在发送设备录音停止控制...".to_string(),
        ),
        Some(session_id)
    );

    coordinator.inner.state.lock().phase = SessionPhase::Idle;
    let idle_session = current_device_key_recording_control_session(&coordinator.inner);
    assert_eq!(idle_session, None);
    assert_eq!(
        emit_device_key_recording_control_capsule(
            &coordinator.inner,
            idle_session,
            DictationUiState::Recording,
            CapsuleState::Reconnecting,
            "正在发送设备录音控制，等待 Listener 音频...".to_string(),
        ),
        None
    );
}

#[test]
fn physical_start_promotes_only_an_active_hidden_automatic_candidate() {
    assert!(should_promote_hidden_automatic_candidate(
        DeviceKeyBleRecordingControlDecision::Start,
        true,
    ));
    assert!(!should_promote_hidden_automatic_candidate(
        DeviceKeyBleRecordingControlDecision::Start,
        false,
    ));
    assert!(!should_promote_hidden_automatic_candidate(
        DeviceKeyBleRecordingControlDecision::Stop {
            session_id: new_session_id(),
            phase: SessionPhase::Listening,
        },
        true,
    ));
}

#[test]
fn device_key_ble_recording_control_ignores_starting_retry() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Starting;
        state.started_at = Instant::now() - Duration::from_millis(375);
        state.cancelled = false;
    }

    match device_key_ble_recording_control_decision(&coordinator.inner) {
        DeviceKeyBleRecordingControlDecision::IgnoreStarting {
            session_id: actual_session_id,
            elapsed_ms,
        } => {
            assert_eq!(actual_session_id, session_id);
            assert!(elapsed_ms >= 300);
        }
        other => panic!("unexpected decision: {other:?}"),
    }

    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Listening;
    }
    assert_eq!(
        device_key_ble_recording_control_decision(&coordinator.inner),
        DeviceKeyBleRecordingControlDecision::Stop {
            session_id,
            phase: SessionPhase::Listening
        }
    );

    coordinator.inner.state.lock().phase = SessionPhase::Idle;
    assert_eq!(
        device_key_ble_recording_control_decision(&coordinator.inner),
        DeviceKeyBleRecordingControlDecision::Start
    );
}

#[test]
fn device_key_ble_pending_start_clears_exact_action_only() {
    let coordinator = Coordinator::new();
    queue_pending_device_key_ble_start(
        &coordinator.inner,
        DeviceCustomKeyId::Key1,
        DeviceCustomKeyGesture::SingleClick,
        "test",
    );

    assert!(!clear_pending_device_key_ble_start(
        &coordinator.inner,
        DeviceCustomKeyId::Key2,
        DeviceCustomKeyGesture::SingleClick,
        "wrong_key",
    ));
    assert!(coordinator
        .inner
        .device_key_pending_ble_action
        .lock()
        .is_some());

    assert!(clear_pending_device_key_ble_start(
        &coordinator.inner,
        DeviceCustomKeyId::Key1,
        DeviceCustomKeyGesture::SingleClick,
        "right_key",
    ));
    assert!(coordinator
        .inner
        .device_key_pending_ble_action
        .lock()
        .is_none());
}

#[test]
fn device_key_ble_pending_start_expires() {
    let coordinator = Coordinator::new();
    *coordinator.inner.device_key_pending_ble_action.lock() = Some(PendingDeviceKeyBleAction {
        kind: PendingDeviceKeyBleActionKind::Start,
        key: DeviceCustomKeyId::Key1,
        gesture: DeviceCustomKeyGesture::SingleClick,
        queued_at: Instant::now() - DEVICE_KEY_BLE_PENDING_ACTION_TTL - Duration::from_millis(1),
    });

    assert!(take_pending_device_key_ble_start(&coordinator.inner, "test").is_none());
    assert!(coordinator
        .inner
        .device_key_pending_ble_action
        .lock()
        .is_none());
}

#[test]
fn device_key_ble_terminal_ttl_clears_only_its_own_pending_action() {
    let coordinator = Coordinator::new();
    let expired = PendingDeviceKeyBleAction {
        kind: PendingDeviceKeyBleActionKind::Start,
        key: DeviceCustomKeyId::Key1,
        gesture: DeviceCustomKeyGesture::SingleClick,
        queued_at: Instant::now() - DEVICE_KEY_BLE_PENDING_ACTION_TTL - Duration::from_millis(1),
    };
    *coordinator.inner.device_key_pending_ble_action.lock() = Some(expired);

    assert_eq!(
        take_expired_pending_device_key_ble_action(
            &coordinator.inner,
            expired,
            "test_terminal_ttl",
        ),
        Some(expired)
    );
    assert!(coordinator
        .inner
        .device_key_pending_ble_action
        .lock()
        .is_none());

    let replacement = PendingDeviceKeyBleAction {
        queued_at: Instant::now(),
        ..expired
    };
    *coordinator.inner.device_key_pending_ble_action.lock() = Some(replacement);
    assert!(take_expired_pending_device_key_ble_action(
        &coordinator.inner,
        expired,
        "test_replacement",
    )
    .is_none());
    assert_eq!(
        *coordinator.inner.device_key_pending_ble_action.lock(),
        Some(replacement)
    );
}

#[test]
fn device_key_ble_retry_pending_only_for_start_recoverable_errors() {
    assert!(should_keep_device_key_ble_start_pending_after_error(
        DeviceKeyBleRecordingControlDecision::Start,
        "Listener BLE low-power idle disconnect"
    ));
    assert!(!should_keep_device_key_ble_start_pending_after_error(
        DeviceKeyBleRecordingControlDecision::Start,
        "no characteristics found for audio control"
    ));
    assert!(!should_keep_device_key_ble_start_pending_after_error(
        DeviceKeyBleRecordingControlDecision::Stop {
            session_id: new_session_id(),
            phase: SessionPhase::Listening,
        },
        "Listener BLE low-power idle disconnect"
    ));
    assert_eq!(
        should_keep_device_key_ble_action_pending_after_error(
            DeviceKeyBleRecordingControlDecision::Stop {
                session_id: new_session_id(),
                phase: SessionPhase::Listening,
            },
            "Listener BLE low-power idle disconnect"
        ),
        Some(PendingDeviceKeyBleActionKind::Stop)
    );
}

#[test]
fn device_key_ble_pending_start_drops_if_state_changed_before_flush() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    queue_pending_device_key_ble_start(
        &coordinator.inner,
        DeviceCustomKeyId::Key1,
        DeviceCustomKeyGesture::SingleClick,
        "test",
    );

    flush_pending_device_key_ble_start(&coordinator.inner, "test");

    assert!(coordinator
        .inner
        .device_key_pending_ble_action
        .lock()
        .is_none());
}

#[test]
fn device_key_ble_pending_stop_restores_until_notify_ready() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    queue_pending_device_key_ble_stop(
        &coordinator.inner,
        DeviceCustomKeyId::Key1,
        DeviceCustomKeyGesture::SingleClick,
        "test",
    );

    flush_pending_device_key_ble_action(&coordinator.inner, "test");

    let pending = coordinator.inner.device_key_pending_ble_action.lock();
    assert!(pending.as_ref().is_some_and(|action| {
        action.kind == PendingDeviceKeyBleActionKind::Stop
            && action.key == DeviceCustomKeyId::Key1
            && action.gesture == DeviceCustomKeyGesture::SingleClick
    }));
}

#[test]
fn device_key_idle_wake_preflight_bypass_is_generation_scoped() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .embedded_ble_device_key_wake_generation
        .store(7, Ordering::SeqCst);

    assert!(!take_embedded_ble_device_key_wake_preflight_bypass(
        &coordinator.inner,
        6
    ));
    assert!(take_embedded_ble_device_key_wake_preflight_bypass(
        &coordinator.inner,
        7
    ));
    assert!(!take_embedded_ble_device_key_wake_preflight_bypass(
        &coordinator.inner,
        7
    ));
}

#[test]
fn end_firmware_ota_transfer_restores_background_listener_for_type_ready() {
    // Source contract: ending any OTA reservation must re-arm notify/TYPE:READY so the
    // device leaves STATUS_LED_BLE_CONNECTED ("找 Type" breathing) after preflight or fail.
    // Successful transfer may defer restore to post-confirm settle via
    // end_firmware_ota_transfer_with_listener_restore(false).
    let source = include_str!("coordinator.rs");
    let start = source
        .find("pub fn end_firmware_ota_transfer")
        .expect("end_firmware_ota_transfer should exist");
    let end = source[start..]
        .find("pub async fn wait_for_embedded_ble_listener_ready_after_firmware_ota")
        .map(|offset| start + offset)
        .expect("end_firmware_ota_transfer boundary");
    let body = &source[start..end];
    assert!(
        body.contains("end_firmware_ota_transfer_with_listener_restore")
            && body.contains("embedded_ble_ota_active")
            && body.contains("store(false")
            && body.contains("refresh_embedded_ble_listener")
            && body.contains("embedded_ble_background_listener_expected"),
        "end_firmware_ota_transfer must clear OTA active then restore background listener/TYPE:READY by default"
    );
}

#[test]
fn firmware_ota_recovery_preflight_bypass_is_generation_scoped() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .embedded_ble_ota_recovery_generation
        .store(7, Ordering::SeqCst);

    assert!(!take_embedded_ble_ota_recovery_preflight_bypass(
        &coordinator.inner,
        6
    ));
    assert!(take_embedded_ble_ota_recovery_preflight_bypass(
        &coordinator.inner,
        7
    ));
    assert!(!take_embedded_ble_ota_recovery_preflight_bypass(
        &coordinator.inner,
        7
    ));
}

#[test]
fn failed_firmware_ota_recovery_is_non_destructive_and_has_no_success_capsule() {
    let runtime = include_str!("coordinator.rs");
    let loop_start = runtime
        .find("async fn embedded_ble_background_listener_loop")
        .expect("background listener loop should exist");
    let cleanup_start = runtime[loop_start..]
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .map(|offset| loop_start + offset)
        .expect("stale cleanup boundary should exist");
    let listener_loop = &runtime[loop_start..cleanup_start];
    assert!(
        listener_loop.contains("let mut firmware_ota_recovery = false;")
            && listener_loop.contains("firmware_ota_recovery = true;")
            && listener_loop.contains("if !firmware_ota_recovery")
            && listener_loop.contains("if firmware_ota_recovery")
            && listener_loop.contains("EmbeddedBleStalePairingCleanupOutcome::RetrySoon"),
        "OTA recovery generation must bypass lost-pair holds and stale-pair cleanup"
    );

    let coordinator = include_str!("coordinator.rs");
    let failure_start = coordinator
        .find("pub fn end_failed_firmware_ota_transfer")
        .expect("failed OTA recovery helper should exist");
    let failure_end = coordinator[failure_start..]
        .find("pub async fn wait_for_embedded_ble_listener_ready_after_firmware_ota")
        .map(|offset| failure_start + offset)
        .expect("failed OTA recovery helper boundary should exist");
    let failure = &coordinator[failure_start..failure_end];
    assert!(
        failure.contains("end_firmware_ota_transfer_with_listener_restore(false)")
            && failure.contains("refresh_embedded_ble_listener_after_failed_firmware_ota"),
        "failed OTA must clear the gate and start one bonded recovery generation"
    );
    assert!(
        runtime.contains("refresh_embedded_ble_listener_with_options(inner, false, true, false)"),
        "failed OTA recovery must not arm the success-only audio-restored capsule"
    );
}

#[test]
fn ec11_hardware_recovery_supersedes_older_ota_recovery_semantics() {
    let runtime = include_str!("coordinator.rs");
    let loop_start = runtime
        .find("async fn embedded_ble_background_listener_loop")
        .expect("background listener loop should exist");
    let cleanup_start = runtime[loop_start..]
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .map(|offset| loop_start + offset)
        .expect("stale cleanup boundary should exist");
    let listener_loop = &runtime[loop_start..cleanup_start];

    let ec11_notice = listener_loop
        .find("let hardware_ec11_recovery_notice =")
        .expect("listener loop must classify explicit EC11 recovery evidence");
    let clear_ota = listener_loop[ec11_notice..]
        .find("firmware_ota_recovery = false;")
        .map(|offset| ec11_notice + offset)
        .expect("explicit EC11 recovery must end older OTA semantics");
    let manual_pairing_hold = listener_loop
        .find("maybe_hold_embedded_ble_after_lost_native_pairing")
        .expect("manual pairing hold must remain present");
    let ota_retry = listener_loop
        .find("if firmware_ota_recovery {")
        .expect("genuine OTA transient recovery must remain non-destructive");

    assert!(
        listener_loop.contains("if firmware_ota_recovery && hardware_ec11_recovery_notice")
            && listener_loop.contains("&& !hardware_ec11_recovery_notice")
            && listener_loop.contains("acknowledged EC11 hardware recovery superseded older OTA recovery semantics"),
        "acknowledged EC11 recovery must override stale OTA preservation and bypass the manual-unpair hold"
    );
    assert!(
        ec11_notice < clear_ota
            && clear_ota < manual_pairing_hold
            && manual_pairing_hold < ota_retry,
        "EC11 arbitration must happen before both lost-pairing hold and OTA bonded-GATT retry"
    );
}

#[test]
fn device_key_idle_wake_joins_an_active_notify_recovery() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    let generation_before = coordinator
        .inner
        .embedded_ble_listener_generation
        .load(Ordering::SeqCst);

    refresh_embedded_ble_listener_for_device_key_wake(&coordinator.inner);

    assert_eq!(
        coordinator
            .inner
            .embedded_ble_listener_generation
            .load(Ordering::SeqCst),
        generation_before
    );
    assert!(!active.load(Ordering::SeqCst));
    assert!(coordinator
        .inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, &active)));
}

#[test]
fn embedded_ble_listener_cancel_replacement_is_pointer_safe() {
    let coordinator = Coordinator::new();
    let first = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    assert!(!first.load(Ordering::SeqCst));
    assert!(embedded_ble_listener_capture_active(&coordinator.inner));
    assert!(!embedded_ble_listener_capture_ready(&coordinator.inner));
    mark_embedded_ble_listener_ready(&coordinator.inner, &first);
    assert!(embedded_ble_listener_capture_ready(&coordinator.inner));

    let second = install_embedded_ble_listener_cancel(&coordinator.inner, 2);
    assert!(first.load(Ordering::SeqCst));
    assert!(embedded_ble_listener_capture_active(&coordinator.inner));
    assert!(!embedded_ble_listener_capture_ready(&coordinator.inner));

    clear_embedded_ble_listener_cancel(&coordinator.inner, &first);
    assert!(coordinator
        .inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|active| Arc::ptr_eq(active, &second)));

    cancel_embedded_ble_listener_capture(&coordinator.inner, "test", false);
    assert!(second.load(Ordering::SeqCst));
    assert!(coordinator
        .inner
        .embedded_ble_listener_cancel
        .lock()
        .is_none());
    assert!(!embedded_ble_listener_capture_active(&coordinator.inner));
    assert!(!embedded_ble_listener_capture_ready(&coordinator.inner));
}

#[tokio::test]
async fn firmware_ota_post_ready_wait_requires_notify_subscription_for_embedded_input() {
    let coordinator = Coordinator::new();
    force_embedded_ble_input_for_test(&coordinator);

    let err = coordinator
        .wait_for_embedded_ble_listener_ready_after_firmware_ota(Duration::from_millis(1))
        .await
        .expect_err("an OTA may not report Type ready before its notify subscription is ready");
    assert!(err.contains("notify subscription did not recover"));

    let cancel = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &cancel);
    assert_eq!(
        coordinator
            .wait_for_embedded_ble_listener_ready_after_firmware_ota(Duration::from_millis(1))
            .await
            .expect("ready notify subscription should complete post-OTA recovery"),
        true
    );
}

#[tokio::test]
async fn firmware_ota_post_ready_wait_wakes_on_the_notify_ready_edge() {
    let coordinator = Coordinator::new();
    force_embedded_ble_input_for_test(&coordinator);
    let inner = Arc::clone(&coordinator.inner);
    let wait = tokio::spawn(async move {
        wait_for_embedded_ble_listener_ready(&inner, Duration::from_secs(1)).await
    });

    tokio::task::yield_now().await;
    let cancel = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &cancel);

    let result = tokio::time::timeout(Duration::from_millis(50), wait)
        .await
        .expect("OTA wait must wake from notify-ready without the 100ms poll delay")
        .expect("OTA wait task must complete");
    assert!(
        result.is_ok(),
        "OTA wait must accept the notify-ready edge: {result:?}"
    );
}

#[tokio::test]
async fn firmware_ota_pretransfer_wait_requires_notify_subscription_for_embedded_input() {
    let coordinator = Coordinator::new();
    force_embedded_ble_input_for_test(&coordinator);

    let err = coordinator
        .wait_for_embedded_ble_listener_ready_before_firmware_ota(Duration::from_millis(1))
        .await
        .expect_err("an OTA may not take over a cold EmbeddedBle listener before notify is ready");
    assert!(err.contains("notify subscription did not recover"));

    let cancel = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &cancel);
    assert_eq!(
        coordinator
            .wait_for_embedded_ble_listener_ready_before_firmware_ota(Duration::from_millis(1))
            .await
            .expect("ready notify subscription should permit the OTA handoff"),
        true
    );
}

#[test]
fn ble_name_apply_handoff_marks_only_the_active_capture_for_disconnect_handoff() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    let handoff = embedded_ble_listener_cccd_handoff_flag(&coordinator.inner, &active);

    pause_embedded_ble_listener_capture_for_ble_name_apply_handoff(&coordinator.inner);

    assert!(active.load(Ordering::SeqCst));
    assert!(handoff.load(Ordering::SeqCst));
    assert!(coordinator
        .inner
        .embedded_ble_listener_cancel
        .lock()
        .is_none());
    assert!(
        coordinator
            .inner
            .embedded_ble_listener_leave_cccd_enabled_on_cancel
            .lock()
            .as_ref()
            .is_some_and(|(active_cancel, _)| Arc::ptr_eq(active_cancel, &active)),
        "the active capture keeps its handoff marker until its cleanup finishes"
    );
}

#[test]
fn firmware_ota_handoff_preserves_the_active_capture_cccd_and_type_lease() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    let handoff = embedded_ble_listener_cccd_handoff_flag(&coordinator.inner, &active);

    pause_embedded_ble_listener_capture_for_ota(&coordinator.inner);

    assert!(active.load(Ordering::SeqCst));
    assert!(
        handoff.load(Ordering::SeqCst),
        "OTA must leave the bonded CCCD enabled so notify teardown skips TYPE:BYE"
    );
}

#[test]
fn recovery_cleanup_waits_for_the_actual_notify_capture_to_release() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn wait_for_embedded_ble_listener_inactive")
        .expect("BLE listener inactive wait should exist");
    let end = source[start..]
        .find("fn mark_embedded_ble_listener_ready")
        .map(|offset| start + offset)
        .expect("BLE listener inactive wait boundary should exist");
    let body = &source[start..end];
    assert!(body.contains("embedded_ble_listener_capture_active(inner)"));
    assert!(
        body.contains("crate::embedded_ble::notify_capture_session_active()"),
        "a cancelled flag alone is not proof that the serialized Windows GATT session released"
    );
}

#[test]
fn shutdown_waits_for_the_actual_winrt_notify_owner_to_release() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn wait_for_embedded_ble_listener_shutdown_release")
        .expect("shutdown release wait should exist");
    let end = source[start..]
        .find("fn mark_embedded_ble_listener_ready")
        .map(|offset| start + offset)
        .expect("shutdown release wait boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("crate::embedded_ble::notify_capture_session_active()"));
    assert!(body.contains("Duration::from_millis(25)"));
    assert!(
        !body.contains("embedded_ble_listener_capture_active"),
        "the cancel slot is cleared before WinRT teardown and cannot prove release"
    );
}

#[test]
fn embedded_ble_pairing_hold_blocks_background_refresh() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &active);
    let generation_before_hold = coordinator.embedded_ble_listener_generation();

    hold_embedded_ble_listener_for_pairing_confirmation(&coordinator.inner, "test hold");

    assert!(active.load(Ordering::SeqCst));
    assert!(!embedded_ble_listener_capture_active(&coordinator.inner));
    assert!(!embedded_ble_listener_capture_ready(&coordinator.inner));
    assert!(coordinator.embedded_ble_listener_generation() > generation_before_hold);
    let generation_after_hold = coordinator.embedded_ble_listener_generation();

    refresh_embedded_ble_listener(&coordinator.inner);

    assert_eq!(
        coordinator.embedded_ble_listener_generation(),
        generation_after_hold
    );
    assert!(!embedded_ble_listener_capture_active(&coordinator.inner));

    clear_embedded_ble_pairing_confirmation_hold(&coordinator.inner, "test clear");
    assert!(
        embedded_ble_pairing_confirmation_hold_remaining(&coordinator.inner, Instant::now())
            .is_none()
    );
}

#[test]
fn embedded_ble_pairing_ready_requires_confirmed_windows_pairing() {
    let mut pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 1,
        failed_devices: 0,
        open_bluetooth_settings: false,
        details: Vec::new(),
    };

    assert!(embedded_ble_pairing_prompt_ready(&pairing));

    pairing.status = crate::embedded_ble::BleDevicePairingPromptStatus::Paired;
    assert!(embedded_ble_pairing_prompt_ready(&pairing));

    pairing.status = crate::embedded_ble::BleDevicePairingPromptStatus::NotFound;
    assert!(!embedded_ble_pairing_prompt_ready(&pairing));

    pairing.status = crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired;
    pairing.open_bluetooth_settings = true;
    assert!(!embedded_ble_pairing_prompt_ready(&pairing));

    pairing.open_bluetooth_settings = false;
    pairing.failed_devices = 1;
    assert!(!embedded_ble_pairing_prompt_ready(&pairing));
}

#[test]
fn embedded_ble_foreground_probe_reuses_ready_background_without_refresh() {
    let coordinator = Coordinator::new();
    open_startup_ble_name_sync_gate_for_test(&coordinator);
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &active);

    assert_eq!(
        embedded_ble_foreground_probe_mode(&coordinator.inner),
        EmbeddedBleForegroundProbeMode::ReuseReadyBackground
    );

    // Full-library tests must not enqueue real Windows GATT work. Ready
    // health probes must keep the live notify session intact.
    cancel_embedded_ble_listener_capture(&coordinator.inner, "test cleanup", false);
    assert!(active.load(Ordering::SeqCst));
}

#[tokio::test]
async fn embedded_ble_foreground_probe_ready_path_does_not_refresh_generation() {
    let coordinator = Coordinator::new();
    open_startup_ble_name_sync_gate_for_test(&coordinator);
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &active);
    let generation_before = coordinator.embedded_ble_listener_generation();

    coordinator
        .probe_embedded_audio_ble_subscription(Some(1_000))
        .await
        .expect("ready background probe must succeed without GATT work");

    assert_eq!(
        coordinator.embedded_ble_listener_generation(),
        generation_before,
        "ready TYPE:READY probe must not restart the background listener"
    );
    assert!(
        coordinator
            .inner
            .embedded_ble_listener_cancel
            .lock()
            .as_ref()
            .is_some_and(|cancel| Arc::ptr_eq(cancel, &active)),
        "ready probe must keep the existing capture cancel handle"
    );
    assert!(embedded_ble_listener_capture_ready(&coordinator.inner));

    cancel_embedded_ble_listener_capture(&coordinator.inner, "test cleanup", false);
}

#[tokio::test]
async fn embedded_ble_start_dictation_reuses_ready_background_without_reopen() {
    let coordinator = Coordinator::new();
    force_embedded_ble_input_for_test(&coordinator);
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &active);
    let generation_before = coordinator.embedded_ble_listener_generation();

    coordinator.start_dictation().await.unwrap();

    assert_eq!(
        coordinator.embedded_ble_listener_generation(),
        generation_before
    );
    assert!(!active.load(Ordering::SeqCst));
    assert!(embedded_ble_listener_capture_ready(&coordinator.inner));
    assert!(coordinator
        .inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|cancel| Arc::ptr_eq(cancel, &active)));
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Starting);
    }
    assert!(embedded_ble_session_actor_history(&coordinator.inner)
        .iter()
        .any(
            |record| record.command == EmbeddedBleSessionActorCommand::StartCommand
                && record.detail.contains("host_start_ready_listener")
        ));
}

#[test]
fn embedded_ble_foreground_probe_refreshes_active_background_even_when_unready() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);

    assert_eq!(
        embedded_ble_foreground_probe_mode(&coordinator.inner),
        EmbeddedBleForegroundProbeMode::RefreshBackgroundListener
    );

    cancel_embedded_ble_listener_capture(&coordinator.inner, "test cleanup", false);
    assert!(active.load(Ordering::SeqCst));
}

#[tokio::test]
async fn embedded_ble_foreground_probe_routes_selected_source_to_background_listener() {
    let _guard = ENV_LOCK.lock().await;
    std::env::remove_var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE");

    let coordinator = Coordinator::new();
    force_microphone_input_for_test(&coordinator);
    assert_eq!(
        embedded_ble_foreground_probe_mode(&coordinator.inner),
        EmbeddedBleForegroundProbeMode::ForegroundProbe
    );

    let mut prefs = coordinator.inner.prefs.get();
    prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    coordinator.inner.prefs.replace_for_tests(prefs);
    assert_eq!(
        embedded_ble_foreground_probe_mode(&coordinator.inner),
        EmbeddedBleForegroundProbeMode::StartBackgroundListener
    );

    std::env::set_var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE", "1");
    assert_eq!(
        embedded_ble_foreground_probe_mode(&coordinator.inner),
        EmbeddedBleForegroundProbeMode::ForegroundProbe
    );

    std::env::remove_var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE");
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    assert_eq!(
        embedded_ble_foreground_probe_mode(&coordinator.inner),
        EmbeddedBleForegroundProbeMode::RefreshBackgroundListener
    );
    cancel_embedded_ble_listener_capture(&coordinator.inner, "test cleanup", false);
    assert!(active.load(Ordering::SeqCst));
}

#[tokio::test]
async fn embedded_ble_repair_refreshes_ready_background_status() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &active);

    let result = coordinator
        .repair_embedded_ble_connection(Some(1_000))
        .await;

    assert!(active.load(Ordering::SeqCst));
    assert!(!coordinator
        .inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|cancel| Arc::ptr_eq(cancel, &active)));
    if let Ok(snapshot) = result {
        assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Ready);
        assert_eq!(
            snapshot.notify_subscription_state,
            EmbeddedBleNotifySubscriptionState::Subscribed
        );
    }
}

#[test]
fn embedded_ble_background_retry_backs_off_for_transient_reopen_errors() {
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "listener: BLE characteristic discovery returned status=GattCommunicationStatus(1)",
            EMBEDDED_BLE_RETRY_BASE_DELAY,
        ),
        EMBEDDED_BLE_RETRY_LONG_DELAY
    );
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "listener: BLE characteristic discovery returned status=GattCommunicationStatus(3)",
            EMBEDDED_BLE_RETRY_BASE_DELAY,
        ),
        EMBEDDED_BLE_RETRY_LONG_DELAY
    );
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "BLE CCCD write async error: Some(HRESULT(0x800706BA))",
            Duration::from_secs(10),
        ),
        EMBEDDED_BLE_RETRY_MAX_DELAY
    );
}

#[test]
fn embedded_ble_background_retry_keeps_noisy_cccd_failures_responsive() {
    for message in [
        "BLE CCCD write async error: Some(HRESULT(0x800704C7))",
        "BLE CCCD write timed out after 8000 ms",
        "BLE CCCD notify write returned status=GattCommunicationStatus(1)",
        "BLE CCCD notify write returned status=ProtocolError protocol_error=3",
    ] {
        assert!(is_embedded_ble_noisy_cccd_failure(message));
        assert!(!is_embedded_ble_background_offline_backoff_error(message));
        assert_eq!(
            next_embedded_ble_background_retry_delay(message, EMBEDDED_BLE_RETRY_BASE_DELAY),
            EMBEDDED_BLE_RETRY_NOISY_CCCD_DELAY
        );
    }
}

#[test]
fn embedded_ble_background_retry_backs_off_for_offline_gatt_failures() {
    let message = "Embedded audio BLE service not found from service selector; device-address fallback failed: BLE device notify path D3B1B3DAC206 failed: BluetoothCacheMode(0): BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected";

    assert!(is_embedded_ble_background_offline_backoff_error(message));
    assert_eq!(
        next_embedded_ble_background_retry_delay(message, EMBEDDED_BLE_RETRY_BASE_DELAY),
        EMBEDDED_BLE_RETRY_OFFLINE_DELAY
    );
}

#[test]
fn embedded_ble_background_retry_caps_generic_errors() {
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "BLE embedded audio notification wait failed: channel closed unexpectedly",
            EMBEDDED_BLE_RETRY_BASE_DELAY,
        ),
        // Base is 200ms; generic backoff doubles once → 400ms (capped later at MAX).
        Duration::from_millis(400)
    );
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "BLE embedded audio notification wait failed: channel closed unexpectedly",
            Duration::from_secs(10),
        ),
        EMBEDDED_BLE_RETRY_MAX_DELAY
    );
}

#[test]
fn embedded_ble_background_retry_is_fast_for_link_loss_events() {
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "BLE device connection status changed to Disconnected; transport_not_ready",
            Duration::from_secs(4),
        ),
        EMBEDDED_BLE_RETRY_FAST_DELAY
    );
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "BLE embedded audio notification wait failed: disconnected",
            Duration::from_secs(4),
        ),
        EMBEDDED_BLE_RETRY_FAST_DELAY
    );
    assert!(is_embedded_ble_automatic_recovery_error(
        "BLE GATT session status changed to Some(GattSessionStatus(0)); transport_not_ready"
    ));
}

#[test]
fn embedded_ble_background_retry_keeps_idle_timeout_responsive() {
    assert_eq!(
        next_embedded_ble_background_retry_delay(
            "BLE embedded audio capture timed out after 60000 ms",
            Duration::from_secs(10),
        ),
        EMBEDDED_BLE_RETRY_BASE_DELAY
    );
}

#[test]
fn embedded_ble_listener_error_snapshot_records_and_clears_setup_failures() {
    let coordinator = Coordinator::new();
    assert_eq!(coordinator.embedded_ble_listener_last_error(), None);

    record_embedded_ble_listener_last_error(
        &coordinator.inner,
        "BLE CCCD write async error: Some(HRESULT(0x800706BA))",
    );
    assert_eq!(
        coordinator.embedded_ble_listener_last_error(),
        Some("BLE CCCD write async error: Some(HRESULT(0x800706BA))".to_string())
    );

    clear_embedded_ble_listener_last_error(&coordinator.inner);
    assert_eq!(coordinator.embedded_ble_listener_last_error(), None);
}

#[test]
fn embedded_ble_notify_ready_clears_stale_listener_error_snapshot() {
    let coordinator = Coordinator::new();
    let cancel = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    record_embedded_ble_listener_last_error(
        &coordinator.inner,
        "BLE embedded audio notification wait failed: background listener stale GATT cache",
    );

    mark_embedded_ble_listener_ready(&coordinator.inner, &cancel);

    assert_eq!(coordinator.embedded_ble_listener_last_error(), None);
    let ready = coordinator.embedded_ble_wake_recovery_snapshot();
    assert_eq!(ready.status, EmbeddedBleWakeRecoveryStatus::Ready);
    assert_eq!(
        ready.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Subscribed
    );
}

#[test]
fn embedded_ble_stale_pairing_cleanup_aborts_after_notify_ready_or_new_error() {
    let coordinator = Coordinator::new();
    let err = "BLE embedded audio notification wait failed: stale GATT cache";
    record_embedded_ble_listener_last_error(&coordinator.inner, err);
    assert!(embedded_ble_recovery_error_still_current(
        &coordinator.inner,
        err,
        "test_current"
    ));

    let cancel = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &cancel);
    assert!(!embedded_ble_recovery_error_still_current(
        &coordinator.inner,
        err,
        "test_ready"
    ));

    let _cancel = install_embedded_ble_listener_cancel(&coordinator.inner, 2);
    record_embedded_ble_listener_last_error(&coordinator.inner, "newer BLE failure");
    assert!(!embedded_ble_recovery_error_still_current(
        &coordinator.inner,
        err,
        "test_replaced"
    ));
}

#[test]
fn embedded_ble_pairing_recovery_guard_blocks_overlap_until_link_check_finishes() {
    let coordinator = Coordinator::new();
    let first = try_begin_embedded_ble_pairing_recovery(
        &coordinator.inner,
        EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON,
    )
    .expect("first recovery should acquire the guard");
    assert!(
        try_begin_embedded_ble_pairing_recovery(
            &coordinator.inner,
            EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON,
        )
        .is_none(),
        "overlapping stale cleanup must not delete Windows pairing while the first recovery is still proving GATT reachability"
    );

    drop(first);

    assert!(
        try_begin_embedded_ble_pairing_recovery(
            &coordinator.inner,
            EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON,
        )
        .is_some(),
        "guard must release after the recovery flow returns"
    );
}

#[test]
fn type_pairasync_recovery_defers_startup_manual_delete_preflight() {
    let coordinator = Coordinator::new();
    assert!(!embedded_ble_type_pairasync_startup_guard_active(
        &coordinator.inner
    ));

    arm_embedded_ble_type_pairasync_startup_guard(&coordinator.inner);
    assert!(embedded_ble_type_pairasync_startup_guard_active(
        &coordinator.inner
    ));

    *coordinator
        .inner
        .embedded_ble_type_pairasync_startup_guard_until
        .lock() = Some(Instant::now());
    assert!(!embedded_ble_type_pairasync_startup_guard_active(
        &coordinator.inner
    ));
    assert!(coordinator
        .inner
        .embedded_ble_type_pairasync_startup_guard_until
        .lock()
        .is_none());

    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_hold_embedded_ble_startup_without_current_native_pairing")
        .expect("startup current-native-pairing preflight helper should exist");
    let end = source[start..]
        .find("fn embedded_ble_background_pairasync_is_authorized")
        .map(|offset| start + offset)
        .expect("startup manual-delete preflight boundary should exist");
    let body = &source[start..end];
    assert!(
        body.find("embedded_ble_type_pairasync_startup_guard_active")
            < body.find("native_windows_hid_present_pairing_addresses_for_startup"),
        "a successful Type PairAsync must suppress startup manual-delete classification until Windows finishes rebuilding services"
    );
}

#[test]
fn startup_active_native_connection_bypasses_incomplete_pairing_enumeration() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_hold_embedded_ble_startup_without_current_native_pairing")
        .expect("startup current-native-pairing preflight helper should exist");
    let end = source[start..]
        .find("fn embedded_ble_background_pairasync_is_authorized")
        .map(|offset| start + offset)
        .expect("startup manual-delete preflight boundary should exist");
    let body = &source[start..end];
    let native_hid_index = body
        .find("native_windows_hid_present_pairing_addresses_for_startup")
        .expect("startup must collect native Windows HID evidence");
    let active_connection_index = body
        .find("native_windows_hid_pairing_active_connection")
        .expect("startup must distinguish an active local HID connection from stale pairing rows");
    let pairing_query_index = body.find("query_listener_pairing").expect(
        "startup must still use the paired-device query when no local connection is active",
    );
    assert!(
        native_hid_index < active_connection_index
            && active_connection_index < pairing_query_index
            && body.contains("startup native Windows HID active connection allows persisted GATT reopen")
            && body.contains("ignoring incomplete paired-device enumeration"),
        "an active local Windows BLE connection must bypass only the false manual-unpair classification before the slower paired-device query"
    );
}

#[test]
fn embedded_ble_recording_control_guidance_calls_out_stale_firmware_or_gatt_cache() {
    let message = embedded_ble_recording_control_guidance(
        "audio control characteristic not found in Listener BLE service",
    );

    assert!(message.contains("BLE 录音控制特征"));
    assert!(message.contains("新固件"));
    assert!(message.contains("重新配对"));
}

#[test]
fn embedded_ble_wake_recovery_snapshot_guides_idle_recovery() {
    let coordinator = Coordinator::new();

    record_embedded_ble_reconnect_attempt(&coordinator.inner, "test");
    let reconnecting = coordinator.embedded_ble_wake_recovery_snapshot();
    assert_eq!(
        reconnecting.status,
        EmbeddedBleWakeRecoveryStatus::Reconnecting
    );
    assert_eq!(reconnecting.reconnect_attempts, 1);
    assert_eq!(
        reconnecting.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Opening
    );

    record_embedded_ble_recovery_failure(
        &coordinator.inner,
        "Listener BLE notify subscription did not recover within 1000 ms after foreground probe; last error: service not found",
    );
    let failed = coordinator.embedded_ble_wake_recovery_snapshot();
    assert_eq!(failed.status, EmbeddedBleWakeRecoveryStatus::NeedsWakeKey);
    assert!(failed.user_guidance.contains("KEY4"));
    assert_eq!(failed.firmware_wake_policy.policy, "key4_only");
    assert!(!failed.firmware_wake_policy.voice_key_deep_sleep_wake);

    record_embedded_ble_notify_ready(&coordinator.inner);
    let ready = coordinator.embedded_ble_wake_recovery_snapshot();
    assert_eq!(ready.status, EmbeddedBleWakeRecoveryStatus::Ready);
    assert_eq!(
        ready.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Subscribed
    );
    assert!(ready.last_ready_at.is_some());
}

#[test]
fn embedded_ble_wake_recovery_tracks_idle_disconnect_as_reconnecting() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .embedded_ble_wake_recovery
        .lock()
        .usb_powered = Some(false);

    record_embedded_ble_recovery_failure(
        &coordinator.inner,
        "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
    );
    let snapshot = coordinator.embedded_ble_wake_recovery_snapshot();

    assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Reconnecting);
    assert_eq!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Lost
    );
    assert!(snapshot.user_guidance.contains("离线状态断开"));
    assert!(snapshot
        .recent_disconnect_reason
        .as_deref()
        .unwrap_or_default()
        .contains("reason=546"));
}

#[test]
fn embedded_ble_notify_ready_suppresses_battery_idle_recovery_capsule() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .embedded_ble_wake_recovery
        .lock()
        .usb_powered = Some(false);

    record_embedded_ble_recovery_failure(
        &coordinator.inner,
        "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
    );
    assert!(!record_embedded_ble_notify_ready(&coordinator.inner));
    let snapshot = coordinator.embedded_ble_wake_recovery_snapshot();

    assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Ready);
    assert_eq!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Subscribed
    );
    assert!(snapshot.recent_disconnect_reason.is_none());
}

#[test]
fn embedded_ble_notify_ready_suppresses_unknown_power_idle_recovery_capsule() {
    let coordinator = Coordinator::new();

    record_embedded_ble_recovery_failure(
        &coordinator.inner,
        "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
    );
    assert!(!record_embedded_ble_notify_ready(&coordinator.inner));
}

#[test]
fn embedded_ble_startup_power_snapshot_keeps_plugged_recovery_context() {
    let coordinator = Coordinator::new();
    let firmware = firmware_snapshot_for_auto_input_test(true);

    record_embedded_ble_firmware_power_snapshot(
        &coordinator.inner,
        &firmware,
        "startup_embedded_ble_power_probe",
    );
    record_embedded_ble_recovery_failure(
        &coordinator.inner,
        "BLE device connection status changed to Disconnected; transport_not_ready",
    );
    let snapshot = coordinator.embedded_ble_wake_recovery_snapshot();

    assert_eq!(snapshot.usb_powered, Some(true));
    assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Reconnecting);
    assert!(snapshot.user_guidance.contains("临时中断"));
}

#[test]
fn embedded_ble_notify_ready_reports_recovered_for_powered_disconnect() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .embedded_ble_wake_recovery
        .lock()
        .usb_powered = Some(true);

    record_embedded_ble_recovery_failure(
        &coordinator.inner,
        "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
    );
    assert!(record_embedded_ble_notify_ready(&coordinator.inner));
}

#[test]
fn recovered_capsule_guard_suppresses_non_link_loss_and_repeated_reconnect() {
    // 2026-07-25 日志里 reconnect_attempts=303 的重连循环,每次 notify-ready
    // 都因为"当前已有听写会话在运行"这种所有权冲突(非链路掉线原因)弹了
    // 录音胶囊,造成胶囊刷屏。守卫现在要求真正的链路恢复才弹,且反复重连不弹。
    let ownership_conflict = "当前已有听写会话在运行，暂不能提交嵌入式音频";
    assert!(
        !should_emit_embedded_ble_recovered_capsule_for_reason(ownership_conflict, None, 0),
        "ownership conflict is not a link-loss recovery and must not emit a capsule"
    );
    assert!(
        !should_emit_embedded_ble_recovered_capsule_for_reason(ownership_conflict, Some(true), 1),
        "ownership conflict must not emit even when usb-powered on attempt 1"
    );

    // refresh / shutdown 等内部原因永远不弹(与既有集成测试一致)
    assert!(!should_emit_embedded_ble_recovered_capsule_for_reason(
        "refresh",
        Some(true),
        0
    ));
    assert!(!should_emit_embedded_ble_recovered_capsule_for_reason(
        "shutdown",
        Some(true),
        0
    ));

    // 真正的链路掉线(reason=546,通电)首次/单次重连应该弹
    let link_loss = "Windows BLE disconnected; reason=546; audio path returned transport_not_ready";
    assert!(
        should_emit_embedded_ble_recovered_capsule_for_reason(link_loss, Some(true), 0),
        "genuine link-loss recovery on a fresh reconnect should emit the capsule"
    );
    assert!(
        should_emit_embedded_ble_recovered_capsule_for_reason(link_loss, Some(true), 1),
        "first reconnect attempt of a link-loss recovery should still emit"
    );

    // 但反复自动重连(>1)不再弹,避免重连循环刷屏——正是 303 次重连不再刷屏的关键
    assert!(
        !should_emit_embedded_ble_recovered_capsule_for_reason(link_loss, Some(true), 2),
        "repeated automatic reconnect (>1) must not re-emit the recovered capsule"
    );
    assert!(
        !should_emit_embedded_ble_recovered_capsule_for_reason(link_loss, Some(true), 303),
        "a runaway reconnect loop (303 attempts) must not spam the recovered capsule"
    );
}

#[test]
fn type_recovery_audio_capsule_is_armed_when_intermediate_is_suppressed_and_consumed_once() {
    let coordinator = Coordinator::new();
    assert!(!take_embedded_ble_type_recovery_audio_capsule(
        &coordinator.inner
    ));
    arm_embedded_ble_type_recovery_audio_capsule(&coordinator.inner);
    assert!(take_embedded_ble_type_recovery_audio_capsule(
        &coordinator.inner
    ));
    assert!(
        !take_embedded_ble_type_recovery_audio_capsule(&coordinator.inner),
        "terminal Type-recovery audio capsule must be one-shot"
    );
}

#[test]
fn ota_recovery_capsule_is_consumed_once_for_the_matching_generation() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .embedded_ble_listener_generation
        .store(42, Ordering::SeqCst);
    coordinator
        .inner
        .embedded_ble_ota_recovery_capsule_generation
        .store(42, Ordering::SeqCst);

    assert!(take_embedded_ble_ota_recovery_capsule(
        &coordinator.inner,
        42
    ));
    assert!(
        !take_embedded_ble_ota_recovery_capsule(&coordinator.inner, 42),
        "the same OTA recovery must not emit the grey capsule twice"
    );

    coordinator
        .inner
        .embedded_ble_ota_recovery_capsule_generation
        .store(42, Ordering::SeqCst);
    assert!(
        !take_embedded_ble_ota_recovery_capsule(&coordinator.inner, 43),
        "an unrelated later listener generation must not consume stale OTA recovery"
    );
}

#[test]
fn embedded_ble_notify_ready_suppresses_internal_refresh_capsule() {
    let coordinator = Coordinator::new();

    record_embedded_ble_listener_cancelled(&coordinator.inner, "refresh");

    assert!(!record_embedded_ble_notify_ready(&coordinator.inner));
}

#[test]
fn embedded_ble_background_recovery_capsule_respects_power_state() {
    let coordinator = Coordinator::new();

    assert!(should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "BLE device connection status changed to Disconnected; transport_not_ready",
    ));

    coordinator
        .inner
        .embedded_ble_wake_recovery
        .lock()
        .usb_powered = Some(false);
    assert!(should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "BLE device connection status changed to Disconnected; transport_not_ready",
    ));

    assert!(!should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
    ));

    coordinator
        .inner
        .embedded_ble_wake_recovery
        .lock()
        .usb_powered = Some(true);
    assert!(should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "BLE device connection status changed to Disconnected; transport_not_ready",
    ));

    assert!(!should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "BLE CCCD write timed out after 8000 ms",
    ));

    record_embedded_ble_reconnect_attempt(&coordinator.inner, "background_retry_test");
    record_embedded_ble_reconnect_attempt(&coordinator.inner, "background_retry_test");
    assert!(!should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "BLE device connection status changed to Disconnected; transport_not_ready",
    ));

    assert!(!should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected",
    ));

    assert!(!should_emit_embedded_ble_background_recovery_capsule(
        &coordinator.inner,
        "BLE CCCD write async error: Some(HRESULT(0x800704C7))",
    ));
}

#[test]
fn embedded_ble_background_pairing_decision_precedes_generic_reconnect_capsule() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn embedded_ble_background_listener_loop")
        .expect("background listener loop should exist");
    let end = source[start..]
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .map(|offset| start + offset)
        .expect("background pairing decision should follow listener loop");
    let body = &source[start..end];
    let pairing_decision = body
        .find("maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background loop must evaluate pairing/recovery state");
    let generic_capsule = body
        .find("EmbeddedBleRecoveryCapsuleMessage::RestoringAudio")
        .expect("background loop should still have a generic reconnect capsule");

    assert!(
        pairing_decision < generic_capsule,
        "user-requested re-pair and stale-cache cleanup must decide first so the generic reconnect capsule does not race Windows pairing UX"
    );
    assert!(
        body.contains(
            "stale_cleanup_outcome == EmbeddedBleStalePairingCleanupOutcome::Skipped"
        ),
        "generic reconnect capsule should only appear when pairing/recovery handling did not take over"
    );
}

#[test]
fn embedded_ble_recovery_capsule_messages_fit_without_ellipsis() {
    let messages = [
        EmbeddedBleRecoveryCapsuleMessage::RestoringAudio,
        EmbeddedBleRecoveryCapsuleMessage::WaitingWindowsPairing,
        EmbeddedBleRecoveryCapsuleMessage::WaitingManualPairing,
        EmbeddedBleRecoveryCapsuleMessage::RebuildingPairing,
        EmbeddedBleRecoveryCapsuleMessage::CleaningPairing,
        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
        EmbeddedBleRecoveryCapsuleMessage::WaitingTypePairing,
        EmbeddedBleRecoveryCapsuleMessage::AudioRecovered,
    ];

    for message in messages {
        let display_units: usize = message
            .text()
            .chars()
            .map(|ch| if ch.is_ascii() { 1 } else { 2 })
            .sum();
        assert!(
            display_units <= 26,
            "{} is too wide for the recovery capsule ({display_units} > 26)",
            message.text()
        );
    }
}

#[test]
fn embedded_ble_background_stale_cleanup_handles_gatt_and_cccd_pairing_cache_failures() {
    let test_start = Instant::now();
    let now =
        test_start + EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN + Duration::from_secs(120);
    let recent_cleanup_at = now - Duration::from_secs(60);
    let cooled_cleanup_at =
        now - EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN - Duration::from_secs(1);
    let snapshot = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD,
        consecutive_reconnect_failures: EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD,
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Failed,
        usb_powered: Some(true),
        recent_disconnect_reason: Some(
            "BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected".to_string(),
        ),
        ..Default::default()
    };
    let gatt_error = "BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected";
    let cccd_error = "BLE CCCD write async error: Some(HRESULT(0x800704C7))";

    assert!(
        should_attempt_embedded_ble_background_stale_pairing_cleanup(
            gatt_error, &snapshot, None, now,
        )
    );
    assert!(
        should_attempt_embedded_ble_background_stale_pairing_cleanup(
            cccd_error, &snapshot, None, now,
        )
    );

    let too_early = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD - 1,
        consecutive_reconnect_failures: EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD - 1,
        ..snapshot.clone()
    };
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            gatt_error, &too_early, None, now,
        )
    );
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            cccd_error, &too_early, None, now,
        )
    );

    let cccd_before_first_attempt = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: 0,
        consecutive_reconnect_failures: 0,
        ..snapshot.clone()
    };
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            cccd_error,
            &cccd_before_first_attempt,
            None,
            now,
        )
    );

    let battery = EmbeddedBleWakeRecoverySnapshot {
        usb_powered: Some(false),
        ..snapshot.clone()
    };
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            gatt_error, &battery, None, now,
        )
    );

    let unknown_power = EmbeddedBleWakeRecoverySnapshot {
        usb_powered: None,
        ..snapshot.clone()
    };
    assert!(
        should_attempt_embedded_ble_background_stale_pairing_cleanup(
            gatt_error,
            &unknown_power,
            None,
            now,
        )
    );

    let link_loss = "BLE device connection status changed to Disconnected; transport_not_ready";
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            link_loss, &snapshot, None, now,
        )
    );

    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            gatt_error,
            &snapshot,
            Some(recent_cleanup_at),
            now,
        )
    );
    assert!(
        should_throttle_embedded_ble_background_stale_pairing_cleanup(
            gatt_error,
            &snapshot,
            Some(recent_cleanup_at),
            now,
        )
    );
    assert!(
        should_throttle_embedded_ble_background_stale_pairing_cleanup(
            cccd_error,
            &snapshot,
            Some(recent_cleanup_at),
            now,
        )
    );
    assert!(
        should_attempt_embedded_ble_background_stale_pairing_cleanup(
            gatt_error,
            &snapshot,
            Some(cooled_cleanup_at),
            now,
        )
    );
    assert!(
        !should_throttle_embedded_ble_background_stale_pairing_cleanup(
            gatt_error,
            &snapshot,
            Some(cooled_cleanup_at),
            now,
        )
    );
    assert!(
        !should_throttle_embedded_ble_background_stale_pairing_cleanup(
            link_loss,
            &snapshot,
            Some(recent_cleanup_at),
            now,
        )
    );
}

#[test]
fn embedded_ble_background_stale_cleanup_respects_manual_windows_unpair() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background stale cleanup helper should exist");
    let end = source[start..]
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| start + offset)
        .expect("background stale cleanup helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("query_listener_pairing"),
        "background stale cleanup must check Windows pairing state before deciding whether Type owns local stale-cache cleanup"
    );
    let manual_helper_start = source
        .find("async fn hold_embedded_ble_for_manual_windows_unpair")
        .expect("manual Windows removal should have one shared hold helper");
    let manual_helper_end = source[manual_helper_start..]
        .find("async fn maybe_hold_embedded_ble_startup_without_current_native_pairing")
        .map(|offset| manual_helper_start + offset)
        .expect("manual Windows hold helper boundary should exist");
    let manual_helper = &source[manual_helper_start..manual_helper_end];
    assert!(
        manual_helper.contains(
            "suppressed automatic PairAsync because Windows no longer reports a paired Listener"
        ),
        "manual Windows device removal must stop Type from immediately pairing the device back"
    );
    assert!(
        manual_helper.contains("EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON"),
        "manual Windows removal should hold for explicit user pairing instead of looping automatic recovery"
    );
    assert!(
        body.contains("prompt_listener_pairing_after_type_recovery"),
        "with Type present, background stale-cache recovery must use the same bounded automatic PairAsync path as BLE rename"
    );
    assert!(
        body.contains("local_stale_cache_recovery_allows_cleanup"),
        "background recovery must keep an automatic local cleanup path when Windows exposes stale local Listener cache evidence"
    );
    let automatic_cleanup_start = body
        .find("let automatic_cleanup_allowed =")
        .expect("automatic cleanup decision should exist");
    let automatic_cleanup_end = body[automatic_cleanup_start..]
        .find("if noisy_cccd_stale_cache_type_owned_cleanup")
        .map(|offset| automatic_cleanup_start + offset)
        .expect("automatic cleanup decision should end before the noisy CCCD log");
    assert!(
        !body[automatic_cleanup_start..automatic_cleanup_end]
            .contains("recovery_advertisement_allows_cleanup"),
        "recovery advertisement visibility alone must not authorize background PairAsync after a manual Windows delete"
    );
    assert!(
        body[automatic_cleanup_start..automatic_cleanup_end]
            .contains("embedded_ble_background_pairasync_is_authorized"),
        "manual Windows removal must be an explicit input to the automatic PairAsync authorization decision"
    );
    assert!(
        body.contains("pairing.already_paired_devices > 0")
            && body.contains("pairing.matched_devices > 0")
            && body.contains("pairing.failed_devices > 0"),
        "automatic cleanup must cover paired cache, stale PnP/cache matches, and failed stale nodes before Type PairAsync"
    );
    let query_index = body
        .find("query_listener_pairing")
        .expect("background stale cleanup must query Windows pairing state");
    let hardware_hold_index = body
        .find("if recovery_pairing_window_visible\n        && !direct_gatt_instability_recovery")
        .expect("hardware recovery hold branch should exist");
    assert!(
        query_index < hardware_hold_index,
        "recovery advertisements must query Windows pairing cache before deciding whether to hold; otherwise Type cannot distinguish stale paired cache from a user switching computers"
    );
    let generic_hold_end = body[hardware_hold_index..]
        .find("if !automatic_cleanup_allowed")
        .map(|offset| hardware_hold_index + offset)
        .expect("hardware recovery hold should end before generic cleanup handling");
    assert!(
        body[hardware_hold_index..generic_hold_end].contains("&& !manual_unpair_hold"),
        "the generic visible-advertisement hold must yield to the dedicated manual-delete branch"
    );
    assert!(
        manual_helper.contains("start_embedded_ble_passive_local_reattach_watch"),
        "manual Windows removal must wait passively for explicit local recovery"
    );
    assert!(
        manual_helper.contains("start_embedded_ble_manual_unpair_recovery_watch")
            && manual_helper.contains("baseline_recovery_addresses"),
        "manual Windows removal must retain the already-visible recovery-address baseline and watch only for a fresh EC11 identity"
    );
    for forbidden in [
        "send_recording_control_manual_pairing",
        "start_embedded_ble_pairing_confirmation_watch",
        "prompt_listener_pairing_after_type_recovery",
    ] {
        assert!(
            !manual_helper.contains(forbidden),
            "manual Windows removal must stay quiet and must not invoke {forbidden}"
        );
    }
    let manual_suppression_index = manual_helper
        .find("suppressed automatic PairAsync because Windows no longer reports a paired Listener")
        .expect("manual removal suppression log should exist");
    let manual_quiet_index = manual_helper[manual_suppression_index..]
        .find("manual Windows unpair quiet hold")
        .map(|offset| manual_suppression_index + offset)
        .expect("manual removal branch should record its quiet ownership release");
    let manual_emit_index = manual_helper[manual_suppression_index..]
        .find("EmbeddedBleRecoveryCapsuleMessage::WaitingManualPairing")
        .map(|offset| manual_suppression_index + offset)
        .expect("manual removal branch should expose an explicit manual-pairing state");
    let manual_call_index = body
        .find("hold_embedded_ble_for_manual_windows_unpair")
        .expect("stale cleanup must call the shared manual-delete hold helper");
    let generic_cleanup_index = body
        .find("if !automatic_cleanup_allowed")
        .expect("generic stale-cache recovery branch should exist");
    let direct_gatt_retry_index = body
        .find("retrying direct audio GATT before clearing Windows pairing cache")
        .expect(
            "generic stale-cache recovery may retain its direct GATT retry for non-manual failures",
        );
    let cleanup_index = body
        .find("prompt_listener_pairing_after_type_recovery")
        .expect("automatic Type recovery branch should still exist");
    assert!(
        manual_suppression_index < manual_quiet_index
            && manual_quiet_index < manual_emit_index
            && manual_call_index < generic_cleanup_index
            && generic_cleanup_index < direct_gatt_retry_index
            && manual_call_index < cleanup_index,
        "manual removal must hold before both direct GATT retry and Type automatic PairAsync recovery"
    );
    let startup_helper_start = source
        .find("async fn maybe_hold_embedded_ble_startup_without_current_native_pairing")
        .expect("startup must preflight a missing current Windows Listener pairing");
    let startup_helper_end = source[startup_helper_start..]
        .find("fn embedded_ble_background_pairasync_is_authorized")
        .map(|offset| startup_helper_start + offset)
        .expect("startup manual-delete preflight boundary should exist");
    let startup_helper = &source[startup_helper_start..startup_helper_end];
    assert!(
        startup_helper.contains("query_listener_pairing")
            && startup_helper.contains("embedded_ble_current_native_pairing_is_missing")
            && startup_helper.contains("hold_embedded_ble_for_missing_native_pairing"),
        "startup must hold a missing current Windows device before persisted GATT can reopen"
    );
    let native_hid_index = startup_helper
        .find("native_windows_hid_present_pairing_addresses_for_startup")
        .expect("startup must recognize a complete native Windows Listener HID pairing");
    let manual_query_index = startup_helper.find("query_listener_pairing").expect(
        "startup manual-delete preflight must still query the weaker Windows BLE pairing view",
    );
    let current_pairing_index = startup_helper
        .find("if pairing.already_paired_devices > 0")
        .expect("a current Windows pairing must remain the fast persisted-GATT path");
    let recovery_scan_index = startup_helper
        .find("listener_recovery_pairing_advertisement_probe")
        .expect("a stale native HID must scan for current recovery advertising");
    assert!(
        native_hid_index < manual_query_index
            && manual_query_index < current_pairing_index
            && current_pairing_index < recovery_scan_index
            && startup_helper.contains("startup native Windows HID/current pairing evidence allows persisted GATT reopen")
            && startup_helper.contains("startup_stale_native_hid_recovery_is_authorized")
            && startup_helper.contains("hold_embedded_ble_for_stale_native_hid_recovery"),
        "a current Listener pairing must retain the fast persisted-GATT path, while a stale HID must require active recovery advertising before bounded local PairAsync"
    );
    assert!(
        body.contains("native_windows_hid_pairing_addresses")
            && body.contains("native Windows HID pairing remains installed; retrying direct GATT without pairing cleanup")
            && body.contains("let manual_unpair_hold = !native_windows_hid_pairing_blocks_pairasync")
            && body.contains("let automatic_cleanup_allowed = !native_windows_hid_pairing_blocks_pairasync"),
        "a native Windows HID pairing must also stay out of the manual-delete hold after a transient GATT failure"
    );
    let listener_loop_start = source
        .find("async fn embedded_ble_background_listener_loop")
        .expect("background listener loop should exist");
    let listener_loop_end = source[listener_loop_start..]
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .map(|offset| listener_loop_start + offset)
        .expect("background listener loop boundary should exist");
    let listener_loop = &source[listener_loop_start..listener_loop_end];
    assert!(
        listener_loop.find("maybe_hold_embedded_ble_startup_without_current_native_pairing")
            < listener_loop.find("install_embedded_ble_listener_cancel"),
        "startup manual-delete preflight must run before persisted GATT/notify setup"
    );
    assert!(
        body.contains("background Type recovery PairAsync result"),
        "Type-owned stale-cache cleanup must explicitly run the bounded Type PairAsync recovery path"
    );
    assert!(
        body.contains("如果你已在另一台电脑用 Windows 弹窗连上，这是预期")
            && body.contains("EmbeddedBleRecoveryCapsuleMessage::WaitingTypePairing"),
        "runtime guidance should keep computer switching safe while the visible capsule stays short"
    );
    let active_gate_index = body
        .find("listener_pairing_maintenance_active")
        .expect("background cleanup must check for an active pairing/cache owner");
    let recovery_guard_index = body
        .find("try_begin_embedded_ble_pairing_recovery")
        .expect("background cleanup must keep a recovery guard through link reachability checks");
    let type_pairasync_index = body
        .find("prompt_listener_pairing_after_type_recovery")
        .expect("background cleanup should use Type's bounded recovery PairAsync path");
    assert!(
        active_gate_index < recovery_guard_index && recovery_guard_index < type_pairasync_index,
        "background cleanup must hold a recovery guard before Type automatic PairAsync recovery"
    );
    assert!(
        !body.contains("background Type recovery pre-pair link check")
            && !body.contains("background Type recovery skipped PairAsync because Listener GATT became reachable first"),
        "EC11 Type double-click recovery must not skip local stale-pair cleanup just because an old GATT path briefly looks reachable"
    );
    assert!(
        body.contains("background Type recovery PairAsync result"),
        "with Type present, double-click recovery must try the bounded Type PairAsync path after local stale-cache cleanup"
    );
    assert!(
        body.contains("if another host paired first, this Type instance must stop instead of stealing it back"),
        "if a different computer completes the Windows popup first, the old Type instance must stop instead of looping or stealing the bond back"
    );
    let active_gate_body = &body[active_gate_index..type_pairasync_index];
    assert!(
        active_gate_body.contains("EmbeddedBleStalePairingCleanupOutcome::RetrySoon"),
        "cross-process pairing maintenance should make the tray retry soon after CLI pairing completes, not sleep for the offline confirmation window"
    );
    let paired_reachable_index = body
        .find("background Type recovery PairAsync paired and link reachable")
        .expect("successful Type PairAsync recovery should prove GATT reachability");
    let immediate_retry_index = body[paired_reachable_index..]
        .find("EmbeddedBleStalePairingCleanupOutcome::RetryImmediate")
        .map(|offset| paired_reachable_index + offset)
        .expect("successful Type PairAsync recovery should reopen notify immediately");
    let confirmation_hold_index = body[paired_reachable_index..]
        .find("EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation")
        .map(|offset| paired_reachable_index + offset)
        .expect("unconfirmed Type PairAsync recovery should still hold for confirmation");
    assert!(
        immediate_retry_index < confirmation_hold_index,
        "once PairAsync and GATT reachability are confirmed, Type should reopen notify immediately instead of sleeping through the old retry delay"
    );
}

#[test]
fn embedded_ble_successful_pairasync_retry_reopens_notify_without_sleep() {
    let source = include_str!("coordinator.rs");
    let loop_start = source
        .find("submit_embedded_audio_ble_stream_background")
        .expect("background listener loop should exist");
    let loop_end = source[loop_start..]
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .map(|offset| loop_start + offset)
        .expect("background listener loop boundary should exist");
    let body = &source[loop_start..loop_end];
    let immediate_branch = body
        .find("stale_cleanup_retry_immediate")
        .expect("background listener should recognize immediate recovery outcome");
    let zero_delay = body[immediate_branch..]
        .find("Duration::ZERO")
        .map(|offset| immediate_branch + offset)
        .expect("immediate recovery outcome should not sleep before reopening notify");
    let long_retry = body[immediate_branch..]
        .find("EMBEDDED_BLE_RETRY_LONG_DELAY")
        .map(|offset| immediate_branch + offset)
        .expect("non-immediate retry outcomes should still use the bounded long delay");
    assert!(
        zero_delay < long_retry,
        "successful PairAsync+GATT recovery must reopen notify before the ordinary retry delay path"
    );
}

#[test]
fn embedded_ble_manual_windows_unpair_suppresses_background_pairasync() {
    let removed = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NotFound,
        attempted: true,
        matched_devices: 0,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 0,
        open_bluetooth_settings: false,
        details: vec![],
    };
    assert!(
        should_hold_embedded_ble_background_recovery_after_manual_unpair(&removed, false, false,)
    );
    assert!(
        !should_hold_embedded_ble_background_recovery_after_manual_unpair(&removed, true, false,),
        "Type-confirmed direct GATT instability recovery may still rebuild pairing"
    );

    let paired = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 1,
        failed_devices: 0,
        open_bluetooth_settings: false,
        details: vec![],
    };
    assert!(
        !should_hold_embedded_ble_background_recovery_after_manual_unpair(&paired, false, false,)
    );

    let manual_delete_failed_node = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 1,
        open_bluetooth_settings: true,
        details: vec![],
    };
    assert!(
        should_hold_embedded_ble_background_recovery_after_manual_unpair(
            &manual_delete_failed_node,
            false,
            false,
        ),
        "manual Windows delete leaves a matched but unpaired Listener devnode; Type must not background PairAsync it back"
    );
    assert!(
        !should_hold_embedded_ble_background_recovery_after_manual_unpair(
            &manual_delete_failed_node,
            false,
            true,
        ),
        "EC11/Type-owned recovery advertisement must not be swallowed by the manual-delete no-steal branch"
    );
    assert!(
        !embedded_ble_background_pairasync_is_authorized(true, true, true, true, true, true, true,),
        "manual Windows delete must override every background PairAsync heuristic"
    );
    assert!(
        embedded_ble_background_pairasync_is_authorized(
            false, true, false, false, false, false, false,
        ),
        "an explicitly Type-observed recovery advertisement may still use the bounded automatic recovery path"
    );
}

#[test]
fn embedded_ble_noisy_cccd_stale_cache_evidence_does_not_hit_manual_delete_hold() {
    let stale_failed_node = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 2,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 2,
        open_bluetooth_settings: true,
        details: vec![],
    };
    let noisy_cccd_error = "BLE CCCD write async error: Some(HRESULT(0x800704C7))";
    assert!(
        noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
            noisy_cccd_error,
            true,
            &stale_failed_node,
        ),
        "a repeated Type-owned CCCD/cache failure must not be swallowed by the manual-delete hold when the advertisement scan misses"
    );
    assert!(
        !should_hold_embedded_ble_background_recovery_after_manual_unpair(
            &stale_failed_node,
            false,
            noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
                noisy_cccd_error,
                true,
                &stale_failed_node,
            ),
        ),
        "EC11/Type recovery should still enter bounded PairAsync when CCCD stale-cache evidence is present"
    );
    assert!(
        !noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
            noisy_cccd_error,
            false,
            &stale_failed_node,
        ),
        "the noisy CCCD fallback remains threshold-gated"
    );

    let missing_pairing_error =
        "No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es)";
    assert!(
        !noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
            missing_pairing_error,
            true,
            &stale_failed_node,
        ),
        "the accepted manual Windows delete path remains user-controlled"
    );
    assert!(
        should_hold_embedded_ble_background_recovery_after_manual_unpair(
            &stale_failed_node,
            false,
            false,
        ),
        "manual Windows delete still suppresses automatic PairAsync/no-steal recovery"
    );
}

#[test]
fn embedded_ble_stale_unpaired_windows_node_stays_user_controlled() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("let local_stale_cache_recovery_allows_cleanup")
        .expect("local stale-cache recovery gate should exist");
    let end = source[start..]
        .find("if recovery_pairing_window_visible")
        .map(|offset| start + offset)
        .expect("local stale-cache recovery gate should precede the hardware hold branch");
    let body = &source[start..end];
    assert!(
        body.contains("pairing.already_paired_devices > 0")
            && body.contains("pairing.matched_devices > 0")
            && body.contains("pairing.failed_devices > 0"),
        "Type cleanup must run for any local stale-cache evidence so users do not click a Windows notification against an uncleared stale node"
    );
    assert!(
        body.contains("!manual_unpair_hold"),
        "manual NotFound removal still stays user-controlled; the cleanup path must not turn into PairAsync"
    );
}

#[test]
fn embedded_ble_manual_windows_unpair_watch_expiry_does_not_refresh_background_listener() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn start_embedded_ble_pairing_confirmation_watch")
        .expect("pairing confirmation watch helper should exist");
    let end = source[start..]
        .find("fn startup_ble_name_sync_reason")
        .map(|offset| start + offset)
        .expect("pairing confirmation watch helper boundary should exist");
    let body = &source[start..end];
    let user_controlled_expiry = body
        .find("!embedded_ble_pairing_confirmation_expiry_should_refresh_background(reason)")
        .expect("user-controlled pairing expiry should have a dedicated no-refresh branch");
    let refresh = body
        .find("refresh_embedded_ble_listener(&inner);")
        .expect("non-manual pairing recovery expiry should still refresh");
    assert!(
        user_controlled_expiry < refresh,
        "manual Windows removal, Type stale cleanup, or hardware recovery must not immediately restart the BLE audio loop through stale GATT after the hold expires"
    );
    assert!(
        body.contains("Type 不会自动抢回连接"),
        "manual/hardware recovery guidance should make the no auto-pair contract explicit"
    );
    let no_refresh_branch = &body[user_controlled_expiry..refresh];
    assert!(
        no_refresh_branch.contains("start_embedded_ble_passive_local_reattach_watch"),
        "after a user-controlled pairing hold expires, Type must keep observing explicit local Windows pairing evidence so a later manual re-pair restores the persistent listener"
    );
    assert!(
        !no_refresh_branch.contains("refresh_embedded_ble_listener(&inner);"),
        "the passive local reattach observer must not turn expiry into an automatic GATT/background retry"
    );
    assert!(
        source.contains("reason != EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON")
            && source.contains("reason != EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON")
            && source.contains("reason != EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON")
            && source.contains("reason != EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON")
            && source.contains("reason != EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON"),
        "manual Windows removal, Type cleanup, direct GATT repair, and physical recovery pairing should stay user-controlled on expiry"
    );
}

#[test]
fn embedded_ble_ec11_recovery_enters_type_controlled_pairing_path() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background BLE recovery helper should exist");
    let probe = source[start..]
        .find("let recovery_pairing_probe")
        .map(|offset| start + offset)
        .expect("BLE recovery advertisement probe should precede Type ownership classification");
    let type_controlled = source[start..]
        .find("let type_observed_recovery_advertisement")
        .map(|offset| start + offset)
        .expect("EC11 recovery should classify the observed advertisement as Type-controlled");
    let cleanup_end = source[start..]
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| start + offset)
        .expect("background BLE recovery helper boundary should exist");
    let preflight = &source[start..probe];
    let cleanup = &source[start..cleanup_end];
    assert!(
        !preflight.contains("HoldForConfirmation"),
        "EC11 recovery evidence must reach advertisement/pairing preflight instead of being blanket-classified as passive"
    );
    assert!(
        probe < type_controlled
            && !cleanup.contains("ec11_external_native_pairing_handoff_requires_arbitration"),
        "a random recovery address is not proof of an external owner; Type must retain its bounded local cleanup path"
    );
}

#[test]
fn ec11_type_controlled_recovery_capsule_waits_for_firmware_terminal_control_write() {
    let source = include_str!("coordinator.rs");
    let cleanup_start = source
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background BLE recovery helper should exist");
    let cleanup_end = source[cleanup_start..]
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| cleanup_start + offset)
        .expect("background BLE recovery helper boundary should exist");
    let cleanup = &source[cleanup_start..cleanup_end];
    assert!(
        cleanup.contains(
            "let ec11_type_controlled_recovery = type_controlled_recovery && hardware_ec11_recovery_notice;"
        ) && cleanup.contains(
            "EC11 Type-controlled recovery suppresses pairing-progress capsule until firmware Type-ready terminal confirmation"
        ),
        "EC11 Type-controlled recovery must suppress its pre-terminal pairing-progress capsule"
    );
    assert!(
        cleanup.contains(
            "EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,\n                        !ec11_type_controlled_recovery,"
        ),
        "the EC11 Type-controlled PairAsync path must not publish a local-pairing capsule before terminal confirmation"
    );

    let resume_start = source
        .find("fn resume_embedded_ble_listener_after_pairing_recovery")
        .expect("pairing recovery resume helper should exist");
    let resume_end = source[resume_start..]
        .find("fn arm_embedded_ble_type_pairasync_startup_guard")
        .map(|offset| resume_start + offset)
        .expect("pairing recovery resume helper boundary should exist");
    let resume = &source[resume_start..resume_end];
    assert!(
        resume.contains("if emit_reconnecting_capsule")
            && resume.contains(
                "EC11 Type-controlled recovery suppresses intermediate capsule until firmware Type-ready terminal confirmation"
            )
            && resume.contains("arm_embedded_ble_type_recovery_audio_capsule"),
        "the notify-reopen path must suppress intermediate UI and arm the terminal grey audio-recovered capsule"
    );
    assert!(
        source.contains("take_embedded_ble_type_recovery_audio_capsule")
            && source.contains("type_recovery_recovered")
            && source.contains("AudioRecovered"),
        "notify ready must consume the armed Type-recovery capsule as grey Listener 音频已恢复"
    );

    let embedded_ble_source = include_str!("embedded_ble.rs");
    let cccd_enabled = embedded_ble_source
        .find("capture #{capture_id}: notify CCCD enabled")
        .expect("background capture must log notify CCCD enablement");
    let terminal_write = embedded_ble_source[cccd_enabled..]
        .find("cleanup.write_type_heartbeat(&type_ready_command, \"Type heartbeat ready\")")
        .map(|offset| cccd_enabled + offset)
        .expect("background capture must write TYPE:READY after notify CCCD");
    let ready_callback = embedded_ble_source[terminal_write..]
        .find("on_ready()?;")
        .map(|offset| terminal_write + offset)
        .expect("ready callback must remain after the terminal control write");
    assert!(
        cccd_enabled < terminal_write && terminal_write < ready_callback,
        "CCCD enablement alone must never publish the recovery terminal UI"
    );
    assert!(
        embedded_ble_source
            .contains("if !type_ready_confirmed {\n                        on_ready()?;")
            && embedded_ble_source
                .contains("Type ready terminal confirmation recovered through heartbeat"),
        "a retried TYPE:READY heartbeat must publish the terminal state exactly once"
    );
}

#[test]
fn embedded_ble_passive_local_reattach_stops_the_failed_background_retry_loop() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn embedded_ble_background_listener_loop")
        .expect("background listener loop should exist");
    let end = source[start..]
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .map(|offset| start + offset)
        .expect("background listener loop boundary should exist");
    let body = &source[start..end];
    let recovery = body
        .find("maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background listener must classify recovery failures");
    let passive = body[recovery..]
        .find("embedded_ble_passive_local_reattach_active")
        .map(|offset| recovery + offset)
        .expect("background loop must observe passive local reattach ownership");
    let retry = body[passive..]
        .find("background listen retrying in")
        .map(|offset| passive + offset)
        .expect("background retry log should remain after the passive branch");
    let passive_branch = &body[passive..retry];
    assert!(
        passive_branch.contains("break;"),
        "the failed background loop must stop instead of retrying while passive local reattach owns recovery"
    );
}

#[test]
fn embedded_ble_matched_pairing_without_already_paired_is_not_manual_unpair() {
    // Owner 2026-07-28: startup log status=NeedsUserAction matched=2 already_paired=0
    // failed=2 with empty HID present list must NOT block persisted GATT reopen.
    let incomplete_aep = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 2,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 2,
        open_bluetooth_settings: true,
        details: Vec::new(),
    };
    assert!(
        !embedded_ble_current_native_pairing_is_missing(&[], &incomplete_aep),
        "matched Windows pairing entries mean the bond cache is not fully gone"
    );
    assert!(
        !embedded_ble_lost_current_native_pairing_should_pause(
            "BLE device connection status changed to Disconnected; transport_not_ready",
            &[],
            &incomplete_aep,
        ),
        "matched-but-not-already-paired must not enter the 180s manual-unpair hold"
    );
    let truly_missing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NotFound,
        attempted: true,
        matched_devices: 0,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 0,
        open_bluetooth_settings: true,
        details: Vec::new(),
    };
    assert!(embedded_ble_current_native_pairing_is_missing(
        &[],
        &truly_missing
    ));
}

#[test]
fn embedded_ble_lost_native_pairing_pauses_before_stale_gatt_retry() {
    let missing_pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NotFound,
        attempted: true,
        matched_devices: 0,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 0,
        open_bluetooth_settings: true,
        details: Vec::new(),
    };
    let link_loss = "BLE device connection status changed to Disconnected; transport_not_ready";
    assert!(embedded_ble_lost_current_native_pairing_should_pause(
        link_loss,
        &[],
        &missing_pairing,
    ));
    assert!(
        embedded_ble_lost_current_native_pairing_should_pause(
            link_loss,
            &[0xD374D0103F0B],
            &missing_pairing,
        ),
        "stale HID nodes may remain Present after Settings removes the real pairing"
    );
    assert!(!embedded_ble_lost_current_native_pairing_should_pause(
        "BLE embedded audio capture timed out",
        &[],
        &missing_pairing,
    ));

    let source = include_str!("coordinator.rs");
    let loop_start = source
        .find("async fn embedded_ble_background_listener_loop")
        .expect("background listener loop should exist");
    let loop_end = source[loop_start..]
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .map(|offset| loop_start + offset)
        .expect("background listener loop boundary should exist");
    let listener_loop = &source[loop_start..loop_end];
    let lost_pairing_hold = listener_loop
        .find("maybe_hold_embedded_ble_after_lost_native_pairing")
        .expect("link loss must recheck current native Windows pairing before stale GATT retry");
    let stale_cleanup = listener_loop
        .find("maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background stale cleanup should remain after the missing-pairing guard");
    assert!(
        lost_pairing_hold < stale_cleanup,
        "no-current-HID pause must run before generic pairing cleanup can reopen old GATT"
    );
    assert!(
        listener_loop[lost_pairing_hold..stale_cleanup].contains("break;"),
        "after current native pairing disappears, the active background loop must stop instead of retrying"
    );

    let runtime_source = source.as_str();
    let lost_helper_start = runtime_source
        .find("async fn maybe_hold_embedded_ble_after_lost_native_pairing")
        .expect("lost-native-pairing helper should exist");
    let lost_helper_end = runtime_source[lost_helper_start..]
        .find("async fn maybe_hold_embedded_ble_startup_without_current_native_pairing")
        .map(|offset| lost_helper_start + offset)
        .expect("lost-native-pairing helper boundary should exist");
    let lost_helper = &runtime_source[lost_helper_start..lost_helper_end];
    assert!(
        lost_helper.contains("native_windows_hid_present_pairing_addresses()"),
        "link-loss ownership must use current Present HID evidence, not historical PnP nodes left by Windows device removal"
    );
    assert!(
        !lost_helper.contains("native_windows_hid_pairing_addresses()"),
        "historical/non-present HID nodes must not authorize stale GATT reopen after manual Windows removal"
    );
    assert!(
        lost_helper.matches("query_listener_pairing").count() >= 2
            && lost_helper.contains("EMBEDDED_BLE_MANUAL_UNPAIR_CONFIRM_DELAY")
            && lost_helper.contains("hold_embedded_ble_for_stale_native_hid_recovery"),
        "link loss must confirm pairing removal twice and route a stale Present HID into quiet EC11-advertisement recovery instead of reopening GATT"
    );
    assert!(
        lost_helper.contains("listener_recovery_addresses_from_error(err)"),
        "the no-HID link-loss hold must preserve any recovery address already visible before it watches for a fresh EC11 identity"
    );

    let notify_ready_start = runtime_source
        .find("fn record_embedded_ble_notify_ready")
        .expect("notify-ready recorder should exist");
    let notify_ready_end = runtime_source[notify_ready_start..]
        .find("fn firmware_mode_for_device_knob_rotation_action")
        .map(|offset| notify_ready_start + offset)
        .expect("notify-ready recorder boundary should exist");
    let notify_ready = &runtime_source[notify_ready_start..notify_ready_end];
    assert!(
        notify_ready.contains("clear_embedded_ble_type_pairasync_startup_guard"),
        "a proven notify-ready session must end the Windows service-rebuild guard so a later manual delete is visible immediately"
    );

    let helper_start = source
        .find("fn hold_embedded_ble_for_missing_native_pairing")
        .expect("missing-native-pairing hold helper should exist");
    let helper_end = source[helper_start..]
        .find("async fn maybe_hold_embedded_ble_after_lost_native_pairing")
        .map(|offset| helper_start + offset)
        .expect("lost-native-pairing hold helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    assert!(
        helper.contains("start_embedded_ble_passive_local_reattach_watch"),
        "the missing-native-pairing path must wait for explicit local HID evidence"
    );
    for forbidden in [
        ".PairAsync(",
        "UnpairAsync",
        "send_recording_control_recovery",
    ] {
        assert!(
            !helper.contains(forbidden),
            "the missing-native-pairing hold must release old GATT rather than reclaim through {forbidden}"
        );
    }
}

#[test]
fn embedded_ble_passive_local_reattach_requires_current_pairing_and_hid() {
    let stale_hid = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 2,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 2,
        open_bluetooth_settings: true,
        details: Vec::new(),
    };
    assert!(
        !embedded_ble_passive_local_reattach_evidence_ready(
            Some(&stale_hid),
            &[0xC1460007A2B6],
            Some(&[0xC1460007A2B6])
        ),
        "a stale HID node without a current Windows pairing must not trigger a GATT reconnect"
    );

    let current_pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired,
        already_paired_devices: 1,
        ..stale_hid
    };
    assert!(
        !embedded_ble_passive_local_reattach_evidence_ready(
            Some(&current_pairing),
            &[],
            Some(&[0xC1460007A2B6])
        ),
        "a paired AEP entry must wait for the current Listener HID node before Type reopens GATT"
    );
    assert!(
        embedded_ble_passive_local_reattach_evidence_ready(
            Some(&current_pairing),
            &[0xC1460007A2B6],
            Some(&[0xC1460007A2B6])
        ),
        "a current Windows pairing plus Listener HID can authorize the bounded fresh GATT check"
    );
    assert!(
        !embedded_ble_passive_local_reattach_evidence_ready(
            None,
            &[0xC1460007A2B6],
            Some(&[0xC1460007A2B6])
        ),
        "the pre-recovery HID address alone must remain insufficient when the pairing query is unavailable"
    );
    assert!(
        !embedded_ble_passive_local_reattach_evidence_ready(None, &[0xF81DF2D18D74], None),
        "a new-looking HID address without a pre-recovery baseline must not reopen GATT"
    );
    assert!(
        embedded_ble_passive_local_reattach_evidence_ready(
            None,
            &[0xF81DF2D18D74],
            Some(&[0xC1460007A2B6])
        ),
        "a newly observed Listener HID address after the passive baseline proves explicit local Windows re-pairing even when the paired-device query times out"
    );
}

#[test]
fn embedded_ble_passive_local_reattach_requires_windows_evidence_before_gatt_resume() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn start_embedded_ble_passive_local_reattach_watch")
        .expect("passive local reattach helper should exist");
    let end = source[start..]
        .find("fn start_embedded_ble_pairing_confirmation_watch")
        .map(|offset| start + offset)
        .expect("passive local reattach helper boundary should exist");
    let body = &source[start..end];
    let pairing_query = body
        .find("query_listener_pairing")
        .expect("passive monitor must read local Windows pairing state");
    let native_hid = body
        .find("native_windows_hid_present_pairing_addresses")
        .expect("passive monitor must read local Windows HID pairing evidence");
    let pairing_ready = body
        .find("embedded_ble_passive_local_reattach_evidence_ready")
        .expect("passive monitor must require current local pairing plus HID evidence");
    let gatt = body
        .find("embedded_ble_pairing_recovery_link_reachable")
        .expect("an unchanged current pairing must retain a bounded GATT check");
    let resume = body
        .find("resume_embedded_ble_listener_after_pairing_recovery")
        .expect("new local HID evidence must restore the persistent notify listener");
    assert!(
        native_hid < pairing_query && pairing_query < pairing_ready && pairing_ready < resume && resume < gatt,
        "passive reattach must use new local HID evidence to restart notify before the slower unchanged-pairing GATT fallback"
    );
    assert!(
        body.contains("baseline_native_hid_addresses")
            && body.contains("new native HID address after passive monitor baseline"),
        "a paired-device enumeration timeout may only be bypassed by a new Listener HID identity relative to the recorded passive baseline"
    );
    let fresh_hid_address = body
        .find("fresh_native_hid_address")
        .expect("passive monitor must retain the exact new HID address");
    let direct_notify = body
        .find("reopening background notify directly without the status-characteristic probe")
        .expect("a fresh HID identity must bypass the long status-characteristic probe");
    assert!(
        fresh_hid_address < direct_notify && direct_notify < resume,
        "a newly observed local HID address must reopen notify immediately"
    );
    assert!(
        body.contains("embedded_ble_power_cycle_hid_observed_at"),
        "fresh local HID evidence must arm the power-cycle audio recovery metric"
    );
    assert!(
        body.contains("arm_embedded_ble_type_pairasync_startup_guard")
            && body.find("arm_embedded_ble_type_pairasync_startup_guard")
                < body.find("resume_embedded_ble_listener_after_pairing_recovery"),
        "a proven local reattach must protect the notify restart from the startup stale-HID preflight while Windows rebuilds services"
    );
    for forbidden in [
        "UnpairAsync",
        "listener_recovery_pairing_advertisement_probe",
        "prompt_listener_pairing",
        "restart_windows_bluetooth_adapter_after_pairing_failure",
    ] {
        assert!(
            !body.contains(forbidden),
            "passive local reattach must not auto-reclaim another host through {forbidden}"
        );
    }
}

#[test]
fn embedded_ble_manual_unpair_passive_timeout_keeps_ec11_watcher_alive() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn start_embedded_ble_passive_local_reattach_watch")
        .expect("passive local reattach helper should exist");
    let end = source[start..]
        .find("fn embedded_ble_passive_local_reattach_evidence_ready")
        .map(|offset| start + offset)
        .expect("passive local reattach helper boundary should exist");
    let body = &source[start..end];
    let timeout = body
        .find("monitor_started.elapsed() >= Duration::from_secs(45)")
        .expect("passive reattach should retain its bounded timeout");
    let manual_hold = body[timeout..]
        .find("reason == EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON")
        .map(|offset| timeout + offset)
        .expect("manual unpair timeout should have a dedicated branch");
    let clear_hold = body[timeout..]
        .find("clear_embedded_ble_pairing_confirmation_hold")
        .map(|offset| timeout + offset)
        .expect("other passive reattach reasons should still clear their hold");

    assert!(manual_hold < clear_hold);
    assert!(
        body[manual_hold..clear_hold].contains("fresh-identity EC11 watcher"),
        "the 45-second passive HID timeout must not cancel the longer explicit EC11 recovery watcher"
    );
}

#[test]
fn embedded_ble_generic_listener_refresh_clears_stale_passive_reattach() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn refresh_embedded_ble_listener_with_options")
        .expect("background listener refresh helper should exist");
    let end = source[start..]
        .find("fn embedded_ble_wake_recovery_snapshot")
        .map(|offset| start + offset)
        .expect("background listener refresh helper boundary should exist");
    let body = &source[start..end];
    assert!(
        body.contains("embedded_ble_passive_local_reattach_active")
            && body.contains("background listener refresh clearing passive local reattach")
            && body.contains("background listener refresh supersedes passive reattach"),
        "an explicit listener refresh must clear a stale passive-reattach hold before rebuilding notify"
    );
}

#[test]
fn embedded_ble_missing_pairing_can_probe_recovery_pairing_advertisement_before_cleanup_threshold()
{
    let now = Instant::now();
    let snapshot = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: 1,
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Failed,
        usb_powered: Some(true),
        ..Default::default()
    };
    let missing_pairing_error = "No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es) A4CB8FF2B512; skipping audio notify advertisement GATT fallback until Windows pairing completes";

    assert!(
        should_probe_embedded_ble_recovery_pairing_advertisement(
            missing_pairing_error,
            &snapshot,
            None,
            now,
        ),
        "physical double-click recovery should let Type scan for the recovery advertisement immediately instead of waiting for stale-cleanup retry threshold"
    );
    assert!(
        recovery_pairing_advertisement_allows_immediate_stale_cleanup(missing_pairing_error),
        "once Windows says the advertised Listener address is not paired, a visible recovery advertisement should enter stale-cache preflight immediately"
    );
    assert!(
        !recovery_pairing_probe_allows_immediate_stale_cleanup(
            missing_pairing_error,
            &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
                visible: true,
                has_random_identity: true,
                addresses: Vec::new(),
            },
            false,
        ),
        "the direct-GATT fast path remains separate; MissingPairing now reaches the normal Windows pairing preflight instead of sleeping"
    );
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            missing_pairing_error,
            &snapshot,
            None,
            now,
        ),
        "the fast path should be gated by actually seeing recovery advertising"
    );
}

#[test]
fn embedded_ble_recovery_advertisement_after_stale_threshold_reaches_pairing_preflight() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background stale cleanup helper should exist");
    let end = source[start..]
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| start + offset)
        .expect("background stale cleanup helper boundary should exist");
    let body = &source[start..end];
    assert!(
        source.contains("EMBEDDED_BLE_TYPE_RECOVERY_PAIRING_SETTLE"),
        "Type-controlled recovery must keep a named settle window before local stale-cache cleanup"
    );
    assert!(
        body.contains("EMBEDDED_BLE_TYPE_RECOVERY_PAIRING_SETTLE"),
        "direct GATT recovery must wait for firmware async bond deletion before Type automatic PairAsync recovery"
    );
    let preflight = body
        .find("background stale pairing cleanup preflight Windows pairing")
        .expect("Windows pairing preflight should exist before cleanup");
    let automatic_allowed = body
        .find("let automatic_cleanup_allowed = !native_windows_hid_pairing_blocks_pairasync")
        .expect("automatic cleanup decision should exist after Windows pairing preflight");
    let visible_hold = body
        .find("recovery pairing advertisement visible from hardware/user action")
        .expect("visible recovery hold branch should exist");
    assert!(
        preflight < automatic_allowed && automatic_allowed < visible_hold,
        "visible recovery advertising alone may hold for manual pairing, but proven stale Windows cache must be decided after Windows pairing preflight instead of sleeping through recovery"
    );
    assert!(
        body.contains("recovery_advertisement_allows_cleanup")
            && body.contains("local_stale_cache_recovery_allows_cleanup")
            && body.contains("&& embedded_ble_background_pairasync_is_authorized"),
        "MissingPairing/StaleGatt recovery advertisements and stale Windows cache evidence must still reach the ownership-aware cleanup decision"
    );
}

#[test]
fn ec11_random_identity_keeps_type_recovery_bounded_and_non_reclaiming() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background stale cleanup helper should exist");
    let end = source[start..]
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| start + offset)
        .expect("background stale cleanup helper boundary should exist");
    let body = &source[start..end];
    assert!(
        body.contains("hardware_ec11_recovery_notice && recovery_pairing_probe.visible")
            && body.contains("background Type recovery PairAsync result"),
        "an EC11 recovery advertisement must take the bounded local Type PairAsync path instead of using address type as an ownership verdict"
    );
    assert!(
        source.contains("if another host paired first, this Type instance must stop instead of stealing it back")
            && source.contains("EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation"),
        "an unsuccessful local PairAsync must enter bounded confirmation wait, not reclaim or retry another host's bond"
    );
    assert!(
        source.contains("if embedded_ble_hardware_ec11_recovery_notice_observed(err) {\n        return true;"),
        "the explicit pre-reset EC11 notice must trigger the recovery-advertisement probe before a downstream GATT timeout"
    );
}

#[test]
fn embedded_ble_type_observed_recovery_uses_after_cache_type_pairing_path() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background stale cleanup helper should exist");
    let end = source[start..]
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| start + offset)
        .expect("background stale cleanup helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("recovery_pairing_advertisement_already_observed_during_notify_open")
            && body.contains("let should_query_pairing_preflight = !type_observed_recovery_advertisement"),
        "Type-observed recovery advertising already proves Type owns the current recovery, so it should not repeat the slow manual-pairing preflight"
    );
    assert!(
        body.contains("!pairing_confirmation_hold_active"),
        "Type-observed fast recovery must not steal back connections while a manual/hardware pairing hold is active"
    );
    assert!(
        body.contains("recover_listener_pairing_after_type_recovery_for_addresses")
            && body.contains("&observed_recovery_addresses")
            && body.contains("&pairing_recovery_addresses"),
        "Type-controlled recovery must pass exact cleanup and fresh pairing addresses into one atomic pairing transaction"
    );
    assert!(
        body.contains("pairing_maintenance_active && !type_controlled_recovery")
            && body.contains("Type-controlled recovery will wait for active pairing/cache maintenance inside the atomic pairing transaction"),
        "an in-flight maintenance owner must be awaited by Type-controlled recovery instead of returning RetrySoon to the stale GATT ladder"
    );

    let pairing_source = include_str!("embedded_ble.rs");
    let atomic_start = pairing_source
        .find("fn recover_listener_pairing_after_type_recovery_for_addresses_inner")
        .expect("atomic Type recovery helper should exist");
    let atomic_end = pairing_source[atomic_start..]
        .find("fn pairing_prompt_result_ready_for_atomic_recovery")
        .map(|offset| atomic_start + offset)
        .expect("atomic Type recovery helper boundary should exist");
    let atomic = &pairing_source[atomic_start..atomic_end];
    let ownership_index = atomic
        .find("begin_listener_pairing_maintenance_after_wait")
        .expect("atomic recovery must acquire pairing maintenance ownership");
    let pairing_only_cleanup_index = atomic
        .find("unpair_listener_devices_for_known_addresses_inner")
        .expect("fresh recovery addresses must take the pairing-only cleanup path");
    let direct_pairasync_index = atomic
        .find("let pairing = prompt_listener_pairing_inner")
        .expect("Type-controlled recovery must PairAsync using the fresh observed address");
    let fallback_marker_index = atomic
        .find("fresh-address PairAsync did not complete")
        .expect("exact cache cleanup must remain an explicit PairAsync-failure fallback");
    let fallback_cleanup_index = atomic
        .find("clear_listener_bthport_cache_for_known_addresses_inner")
        .expect("PairAsync failure must retain the exact-cache fallback");
    assert!(
        ownership_index < pairing_only_cleanup_index
            && pairing_only_cleanup_index < direct_pairasync_index
            && direct_pairasync_index < fallback_marker_index
            && fallback_marker_index < fallback_cleanup_index,
        "one maintenance owner must span pairing-only cleanup, direct PairAsync, and the exact-cache fallback"
    );
    assert!(
        atomic
            .matches("begin_listener_pairing_maintenance_after_wait")
            .count()
            == 1
            && atomic.matches("prompt_listener_pairing_inner(").count() == 2
            && !atomic.contains("unpair_listener_pairing_for_known_addresses(")
            && !atomic.contains("clear_listener_bthport_cache_for_known_addresses("),
        "atomic recovery must not release ownership through separately guarded public cleanup or pairing entrypoints"
    );
    assert!(
        body.contains("background Type controlled-recovery PairAsync paired; reopening notify immediately for GATT/notify validation"),
        "after PairAsync succeeds, Type-controlled recovery should let the real notify-open path provide GATT evidence instead of doing a duplicate status probe"
    );
    assert!(
        body.contains("let stale_native_hid_recovery =")
            && body.contains("type_observed_recovery_advertisement || stale_native_hid_recovery"),
        "a random recovery advertisement with an unusable native HID record must reuse the no-popup Type-controlled path instead of falling back to generic Windows pairing"
    );
}

#[test]
fn embedded_ble_silent_ec11_recovery_advertisement_is_type_owned() {
    let error = "Listener recovery advertisement visible for persisted address D09E84330820 after explicit EC11 fresh identity; missing pairing must use Type automatic PairAsync recovery before declaring notify ready";

    assert!(recovery_pairing_advertisement_already_observed_during_notify_open(error));
    assert_eq!(
        listener_recovery_addresses_from_error(error),
        vec![0xD09E_8433_0820]
    );
}

#[test]
fn embedded_ble_observed_recovery_extracts_known_address_for_fast_cleanup() {
    let addresses = listener_recovery_addresses_from_error(
        "Listener recovery Swift Pair advertisement visible for notify CCCD address F1CFEC3F0E5E after BLE CCCD write async error: Some(HRESULT(0x800704C7))",
    );
    assert_eq!(addresses, vec![0xF1CF_EC3F_0E5E]);

    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("background stale cleanup helper should exist");
    let end = source[start..]
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| start + offset)
        .expect("background stale cleanup helper boundary should exist");
    let body = &source[start..end];
    assert!(
        body.contains("recovery_pairing_addresses_for_cleanup")
            && body.contains("recover_listener_pairing_after_type_recovery_for_addresses")
            && body.contains("&observed_recovery_addresses")
            && !body.contains("unpair_listener_devices_for_names(&cleanup_names)"),
        "Type-observed recovery must carry either its scanned or error-embedded address directly into the atomic known-address recovery transaction instead of doing the slow full-name cleanup"
    );
}

#[test]
fn embedded_ble_device_missing_can_probe_recovery_pairing_advertisement_before_cleanup_threshold() {
    let now = Instant::now();
    let snapshot = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: 1,
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Failed,
        usb_powered: Some(true),
        ..Default::default()
    };
    let device_missing_error =
        "Embedded audio BLE service 710AF845-0000-1000-8000-00805F9B34FB not found by Windows status selector";

    assert!(
        should_probe_embedded_ble_recovery_pairing_advertisement(
            device_missing_error,
            &snapshot,
            None,
            now,
        ),
        "if Listener is visibly advertising for recovery, Type should not treat Windows service-selector missing as a long offline wait"
    );
    assert!(
        recovery_pairing_advertisement_allows_immediate_stale_cleanup(device_missing_error),
        "a visible recovery advertisement turns service-selector missing into local stale-cache cleanup and bounded Type automatic PairAsync recovery"
    );
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            device_missing_error,
            &snapshot,
            None,
            now,
        ),
        "without a visible recovery advertisement, device-missing still stays conservative"
    );
}

#[test]
fn embedded_ble_noisy_cccd_visible_recovery_advertisement_rebuilds_pairing_before_threshold() {
    let now = Instant::now();
    let snapshot = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: 1,
        consecutive_reconnect_failures: 1,
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Failed,
        usb_powered: Some(true),
        ..Default::default()
    };
    let cccd_error = "BLE CCCD write async error: Some(HRESULT(0x800704C7))";

    assert!(
        should_probe_embedded_ble_recovery_pairing_advertisement(
            cccd_error,
            &snapshot,
            None,
            now,
        ),
        "a noisy CCCD failure should scan for the physical recovery-pairing advertisement immediately"
    );
    assert!(
        recovery_pairing_advertisement_allows_immediate_stale_cleanup(cccd_error),
        "once recovery advertising is visible, noisy CCCD errors are stale Windows link state, not a silent retry case"
    );
    assert!(
        !should_attempt_embedded_ble_background_stale_pairing_cleanup(
            cccd_error, &snapshot, None, now,
        ),
        "the early path is still gated by actually seeing recovery advertising"
    );
}

#[test]
fn embedded_ble_notify_advertisement_evidence_skips_only_the_duplicate_scan() {
    let known_recovery_error = "Listener recovery Swift Pair advertisement visible for notify CCCD address F1CFEC3F0E5E after BLE CCCD write async error: Some(HRESULT(0x800704C7)); missing pairing must use Type automatic PairAsync recovery before declaring notify ready";
    let active_capture_recovery_error = "Listener recovery Swift Pair advertisement visible for active capture address F1CFEC3F0E5E after BLE device connection status changed to Disconnected; transport_not_ready; missing pairing must use Type automatic PairAsync recovery before declaring notify ready";
    let manual_windows_unpair_error = "No paired BLE device found in Windows Bluetooth pairing store for advertised Listener address(es) F1CFEC3F0E5E";

    assert!(
        recovery_pairing_advertisement_already_observed_during_notify_open(known_recovery_error),
        "a notify-open recovery-advertisement observation should not be scanned again"
    );
    assert!(
        recovery_pairing_advertisement_already_observed_during_notify_open(
            active_capture_recovery_error
        ),
        "active-capture recovery advertising should not repeat the coordinator scan"
    );
    assert!(
        !recovery_pairing_advertisement_already_observed_during_notify_open(
            manual_windows_unpair_error
        ),
        "manual Windows unpair must keep its separate no-auto-pair decision path"
    );

    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn maybe_probe_embedded_ble_recovery_pairing_advertisement")
        .expect("recovery advertisement probe helper should exist");
    let end = source[start..]
        .find("fn should_probe_embedded_ble_recovery_pairing_advertisement")
        .map(|offset| start + offset)
        .expect("recovery advertisement probe helper should end before its predicate");
    let body = &source[start..end];
    let known_evidence = body
        .find("recovery_pairing_advertisement_already_observed_during_notify_open")
        .expect("known notify evidence should be handled before scanning");
    let duplicate_scan = body
        .find("listener_recovery_pairing_advertisement_probe")
        .expect("unknown recovery state should retain the bounded advertisement scan");
    assert!(
        known_evidence < duplicate_scan,
        "known notify evidence may remove only the redundant scan; unknown recovery state must still scan"
    );
}

#[test]
fn embedded_ble_recovery_pairing_probe_ignores_stale_usb_power_snapshot() {
    let now = Instant::now();
    let snapshot = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD,
        consecutive_reconnect_failures:
            EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD,
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Failed,
        usb_powered: Some(false),
        ..Default::default()
    };
    let reconnect_error =
        "BLE device connection status changed to Disconnected; transport_not_ready";

    assert!(
        should_probe_embedded_ble_recovery_pairing_advertisement(
            reconnect_error,
            &snapshot,
            None,
            now,
        ),
        "a stale battery/USB snapshot must not block probing for a visible double-click recovery pairing advertisement"
    );
    assert!(
        !recovery_pairing_advertisement_allows_immediate_stale_cleanup(reconnect_error),
        "a transient disconnect must retry direct audio GATT instead of clearing Windows pairing cache just because recovery advertising is visible"
    );
    assert!(
        !recovery_pairing_probe_allows_immediate_stale_cleanup(
            reconnect_error,
            &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
                visible: true,
                has_random_identity: false,
                addresses: Vec::new(),
            },
            false,
        ),
        "a public-address recovery advertisement can still be a transient reconnect, so it should not immediately tear down Windows pairing"
    );
    assert!(
        !should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            reconnect_error,
            &EmbeddedBleWakeRecoverySnapshot {
                reconnect_attempts:
                    EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD - 1,
                consecutive_reconnect_failures:
                    EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD - 1,
                ..snapshot.clone()
            },
            None,
            now,
        ),
        "direct GATT pairing recovery waits for repeated lease churn, not the first transient recovery scan"
    );
}

#[test]
fn embedded_ble_transient_link_loss_skips_recovery_pairing_scan_before_direct_gatt_threshold() {
    let now = Instant::now();
    let reconnect_error =
        "BLE device connection status changed to Disconnected; transport_not_ready";
    let transient_snapshot = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD - 1,
        consecutive_reconnect_failures:
            EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD - 1,
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Lost,
        usb_powered: Some(true),
        ..Default::default()
    };

    assert!(
        !should_probe_embedded_ble_recovery_pairing_advertisement(
            reconnect_error,
            &transient_snapshot,
            None,
            now,
        ),
        "ordinary reboot/link-loss reconnect must not spend the fast path on a recovery-advertisement scan"
    );
    assert_eq!(
        next_embedded_ble_background_retry_delay(reconnect_error, Duration::from_secs(4)),
        EMBEDDED_BLE_RETRY_FAST_DELAY,
        "the next background retry after ordinary link loss stays on the fast reconnect cadence"
    );
}

#[test]
fn embedded_ble_random_identity_recovery_advertisement_waits_for_user_pairing() {
    let reconnect_error =
        "BLE device connection status changed to Disconnected; transport_not_ready";

    assert!(
        !recovery_pairing_probe_allows_immediate_stale_cleanup(
            reconnect_error,
            &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
                visible: true,
                has_random_identity: true,
                addresses: Vec::new(),
            },
            false,
        ),
        "a random-address recovery advertisement can be a user switching computers, so background Type must wait for explicit Windows pairing instead of auto-pairing the old host"
    );
    assert!(
        recovery_pairing_probe_allows_immediate_stale_cleanup(
            reconnect_error,
            &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
                visible: true,
                has_random_identity: true,
                addresses: Vec::new(),
            },
            true,
        ),
        "the Type-confirmed direct GATT recovery path may still rebuild Windows pairing"
    );
}

#[test]
fn stale_native_hid_evidence_does_not_block_physical_double_recovery() {
    let stale_pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: false,
        matched_devices: 2,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 2,
        open_bluetooth_settings: true,
        details: Vec::new(),
    };
    let random_recovery = crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
        visible: true,
        has_random_identity: true,
        addresses: Vec::new(),
    };

    assert!(
        !native_windows_hid_pairing_blocks_type_pairasync(
            true,
            &random_recovery,
            Some(&stale_pairing),
        ),
        "a physical double-click recovery must rebuild an unusable local HID pairing key instead of retrying that stale address forever"
    );
    assert!(
        native_windows_hid_pairing_blocks_type_pairasync(true, &random_recovery, None),
        "without Windows evidence that the HID record is unusable, a random recovery advertisement stays conservative for a possible computer switch"
    );
    assert!(
        native_windows_hid_pairing_blocks_type_pairasync(
            true,
            &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default(),
            Some(&stale_pairing),
        ),
        "ordinary startup and transient link handling must keep the accepted native-HID takeover path"
    );
}

#[test]
fn startup_stale_native_hid_requires_visible_recovery_before_type_pairasync() {
    let stale_pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 1,
        open_bluetooth_settings: true,
        details: Vec::new(),
    };
    let recovery_advertisement = crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
        visible: true,
        has_random_identity: true,
        addresses: vec![0xDCC2_3A61_9576],
    };
    assert!(startup_stale_native_hid_recovery_is_authorized(
        &[0xDCC2_3A61_9576],
        &stale_pairing,
        &recovery_advertisement,
    ));
    assert!(!startup_stale_native_hid_recovery_is_authorized(
        &[],
        &stale_pairing,
        &recovery_advertisement,
    ));
    assert!(!startup_stale_native_hid_recovery_is_authorized(
        &[0xDCC2_3A61_9576],
        &stale_pairing,
        &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default(),
    ));

    let current_pairing = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired,
        attempted: true,
        matched_devices: 1,
        prompted_devices: 0,
        already_paired_devices: 1,
        failed_devices: 0,
        open_bluetooth_settings: false,
        details: Vec::new(),
    };
    assert!(!startup_stale_native_hid_recovery_is_authorized(
        &[0xDCC2_3A61_9576],
        &current_pairing,
        &recovery_advertisement,
    ));

    let manual_delete = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NotFound,
        attempted: true,
        matched_devices: 0,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 0,
        open_bluetooth_settings: false,
        details: Vec::new(),
    };
    assert!(
        !startup_stale_native_hid_recovery_is_authorized(
            &[0xDCC2_3A61_9576],
            &manual_delete,
            &recovery_advertisement,
        ),
        "the old same-address advertisement after manual deletion is not new physical authorization"
    );
    assert!(
        startup_stale_native_hid_recovery_is_authorized(
            &[0xDCC2_3A61_9577],
            &manual_delete,
            &recovery_advertisement,
        ),
        "a fresh rotated recovery identity after confirmed manual deletion is the EC11 authorization boundary"
    );

    assert!(
        recovery_pairing_probe_fresh_addresses(&[], &recovery_advertisement,)
            .contains(&0xDCC2_3A61_9576)
    );
    assert!(
        recovery_pairing_probe_fresh_addresses(&[0xDCC2_3A61_9576], &recovery_advertisement,)
            .is_empty(),
        "an advertisement already visible at the quiet-hold boundary cannot authorize PairAsync"
    );
}

#[test]
fn stale_native_hid_watch_starts_pairasync_only_after_recovery_advertising() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn start_embedded_ble_stale_native_hid_recovery_watch")
        .expect("stale native HID recovery watch should exist");
    let end = source[start..]
        .find("async fn maybe_hold_embedded_ble_after_lost_native_pairing")
        .map(|offset| start + offset)
        .expect("stale native HID recovery watch boundary should exist");
    let watch = &source[start..end];
    let authorization = watch
        .find("startup_stale_native_hid_recovery_is_authorized")
        .expect("watch must require stale local ownership evidence");
    let clear_hold = watch
        .find("clear_embedded_ble_pairing_confirmation_hold")
        .expect("watch must release the passive hold only after recovery evidence");
    let pairasync = watch
        .find("maybe_attempt_embedded_ble_background_stale_pairing_cleanup")
        .expect("watch must use the shared bounded Type PairAsync transaction");
    assert!(
        authorization < clear_hold && clear_hold < pairasync,
        "stale native HID recovery must see local stale-pairing plus active recovery advertising before it clears the passive hold and invokes PairAsync"
    );
    assert!(
        watch.contains("EMBEDDED_BLE_PAIRING_CONFIRMATION_HOLD")
            && !watch.contains("EMBEDDED_BLE_RECOVERY_PAIRING_ADV_SCAN_TIMEOUT")
            && !watch.contains("native_windows_hid_pairing_addresses")
            && !watch.contains("query_listener_pairing"),
        "the stale-HID watcher must keep one bounded advertisement listener and reuse its startup proof instead of inserting a second slow HID/pairing query after a physical recovery signal"
    );

    let manual_start = source
        .find("fn hold_embedded_ble_for_missing_native_pairing")
        .expect("manual Windows removal hold should exist");
    let manual_end = source[manual_start..]
        .find("fn hold_embedded_ble_for_stale_native_hid_recovery")
        .map(|offset| manual_start + offset)
        .expect("manual Windows removal hold boundary should exist");
    assert!(
        !source[manual_start..manual_end]
            .contains("start_embedded_ble_stale_native_hid_recovery_watch"),
        "manual Windows removal must keep its passive local reattach path and never auto-pair from this watcher"
    );
}

#[test]
fn embedded_ble_repeated_direct_gatt_link_loss_escalates_to_pairing_recovery() {
    let now = Instant::now();
    let reconnect_error =
        "BLE device connection status changed to Disconnected; transport_not_ready";
    let snapshot = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD,
        consecutive_reconnect_failures:
            EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD,
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Lost,
        usb_powered: Some(true),
        ..Default::default()
    };

    assert!(
        should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            reconnect_error,
            &snapshot,
            None,
            now,
        ),
        "repeated Windows direct-GATT lease drops should stop silent retry and rebuild pairing"
    );

    let too_early = EmbeddedBleWakeRecoverySnapshot {
        reconnect_attempts: EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD - 1,
        consecutive_reconnect_failures:
            EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD - 1,
        ..snapshot.clone()
    };
    assert!(
        !should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            reconnect_error,
            &too_early,
            None,
            now,
        )
    );

    let subscribed = EmbeddedBleWakeRecoverySnapshot {
        notify_subscription_state: EmbeddedBleNotifySubscriptionState::Subscribed,
        ..snapshot.clone()
    };
    assert!(
        !should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            reconnect_error,
            &subscribed,
            None,
            now,
        )
    );

    assert!(
        !should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            "Windows BLE disconnected; reason=546; low-power idle",
            &snapshot,
            None,
            now,
        )
    );

    assert!(
        !should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            reconnect_error,
            &snapshot,
            Some(now - Duration::from_secs(60)),
            now,
        ),
        "the pairing rebuild path is cooldown-protected so Windows prompts do not repeat"
    );
}

#[test]
fn embedded_ble_manual_unpair_requires_windows_pairing_before_gatt() {
    assert!(!embedded_ble_pairing_recovery_accepts_link_reachable(
        EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON,
        false,
    ));
    assert!(embedded_ble_pairing_recovery_accepts_link_reachable(
        EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON,
        true,
    ));
    assert!(!embedded_ble_pairing_recovery_accepts_link_reachable(
        EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON,
        false,
    ));
    assert!(
        !embedded_ble_pairing_recovery_accepts_link_reachable(
            EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
            false,
        ),
        "a manual Windows delete must not let stale readable GATT clear the user-controlled hold"
    );
    assert!(!embedded_ble_pairing_recovery_accepts_link_reachable(
        EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON,
        false,
    ));
    assert!(!embedded_ble_pairing_recovery_accepts_link_reachable(
        EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON,
        false,
    ));
    assert!(embedded_ble_pairing_recovery_accepts_link_reachable(
        EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON,
        true,
    ));
}

#[test]
fn embedded_ble_pairing_confirmation_accepts_complete_native_hid_when_aep_lags() {
    let lagging_aep = crate::embedded_ble::BleDevicePairingPromptResult {
        status: crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction,
        attempted: true,
        matched_devices: 2,
        prompted_devices: 0,
        already_paired_devices: 0,
        failed_devices: 2,
        open_bluetooth_settings: true,
        details: Vec::new(),
    };

    assert!(
        !embedded_ble_pairing_confirmation_ready(&lagging_aep, &[]),
        "a manual Windows delete must remain held when neither AEP nor complete native HID evidence is present"
    );
    assert!(
        embedded_ble_pairing_confirmation_ready(&lagging_aep, &[0xCAC5_121B_9576]),
        "a matching Listener BTHLE root plus HID keyboard confirms a completed user pairing while the weaker AEP pairing view refreshes"
    );
}

#[test]
fn embedded_ble_pairing_recovery_status_probe_uses_windows_gatt_rebuild_budget() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("async fn embedded_ble_pairing_recovery_link_reachable")
        .expect("pairing recovery reachability helper should exist");
    let end = source[start..]
        .find("fn start_embedded_ble_passive_local_reattach_watch")
        .map(|offset| start + offset)
        .expect("pairing recovery helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("EMBEDDED_BLE_PAIRING_GATT_REBUILD_TIMEOUT")
            && source.contains("const EMBEDDED_BLE_PAIRING_GATT_REBUILD_TIMEOUT: Duration = Duration::from_secs(20);"),
        "Windows BLE service rebuild after UnpairAsync/PairAsync can exceed a short 5s probe; recovery resume must keep a 20s GATT budget"
    );
    assert!(
        !body.contains("EMBEDDED_BLE_PRE_PAIR_LINK_CHECK_TIMEOUT"),
        "EC11 Type double-click recovery must clear the local stale pair first instead of using a pre-pair GATT shortcut"
    );
}

#[test]
fn embedded_ble_notify_ready_reports_power_cycle_audio_recovery_target() {
    let source = include_str!("coordinator.rs");
    let start = source
        .find("fn mark_embedded_ble_listener_ready")
        .expect("notify-ready helper should exist");
    let end = source[start..]
        .find("fn install_embedded_ble_listener_cancel")
        .map(|offset| start + offset)
        .expect("notify-ready helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("embedded_ble_power_cycle_hid_observed_at"));
    assert!(body.contains("EMBEDDED_BLE_POWER_CYCLE_AUDIO_RECOVERY_TARGET"));
    assert!(source.contains(
        "const EMBEDDED_BLE_POWER_CYCLE_AUDIO_RECOVERY_TARGET: Duration = Duration::from_secs(3);"
    ));
}

#[test]
fn embedded_ble_repeated_notification_disconnects_stay_fast_and_diagnostic() {
    let coordinator = Coordinator::new();

    for attempt in 1..=5 {
        record_embedded_ble_reconnect_attempt(&coordinator.inner, "rapid_reconnect_test");
        record_embedded_ble_recovery_failure(
            &coordinator.inner,
            "BLE device connection status changed to Disconnected; transport_not_ready",
        );
        let reconnecting = coordinator.embedded_ble_wake_recovery_snapshot();

        assert_eq!(reconnecting.reconnect_attempts, attempt);
        assert_eq!(
            reconnecting.status,
            EmbeddedBleWakeRecoveryStatus::Reconnecting
        );
        assert_eq!(
            reconnecting.notify_subscription_state,
            EmbeddedBleNotifySubscriptionState::Lost
        );
        assert!(reconnecting
            .recent_disconnect_reason
            .as_deref()
            .unwrap_or_default()
            .contains("connection status changed"));
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                reconnecting.recent_disconnect_reason.as_deref().unwrap(),
                Duration::from_secs(4),
            ),
            EMBEDDED_BLE_RETRY_FAST_DELAY
        );

        let recovered_capsule_visible = record_embedded_ble_notify_ready(&coordinator.inner);
        assert_eq!(
            recovered_capsule_visible,
            attempt == 1,
            "only the first repeated automatic recovery should show a reconnected capsule"
        );
        let ready = coordinator.embedded_ble_wake_recovery_snapshot();
        assert_eq!(ready.status, EmbeddedBleWakeRecoveryStatus::Ready);
        assert_eq!(
            ready.consecutive_reconnect_failures, 0,
            "a successful notify subscription must reset the direct-GATT cleanup counter"
        );
        assert_eq!(
            ready.notify_subscription_state,
            EmbeddedBleNotifySubscriptionState::Subscribed
        );
        assert!(ready.recent_disconnect_reason.is_none());
    }
}

#[tokio::test]
async fn hotkey_injection_gate_logs_pressed_and_cancels() {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(false)
        .try_init();
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN", "1");

    let coordinator = Coordinator::new();
    force_microphone_input_for_test(&coordinator);
    coordinator.inject_hotkey_click_for_dev().await.unwrap();

    assert_eq!(coordinator.inner.state.lock().phase, SessionPhase::Idle);
    std::env::remove_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN");
}

#[tokio::test]
async fn begin_session_dry_run_enters_listening_and_clears_stale_edges() {
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN", "1");

    let coordinator = Coordinator::new();
    force_microphone_input_for_test(&coordinator);
    let old_session_id = coordinator.inner.state.lock().session_id;
    {
        let mut state = coordinator.inner.state.lock();
        state.pending_stop = true;
        state.cancelled = true;
    }

    coordinator.start_dictation().await.unwrap();

    let state = coordinator.inner.state.lock();
    assert_eq!(state.phase, SessionPhase::Listening);
    assert!(!state.pending_stop);
    assert!(!state.cancelled);
    assert_ne!(state.session_id, old_session_id);

    std::env::remove_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN");
}

#[tokio::test]
async fn begin_session_ignores_non_idle_phase() {
    let _guard = ENV_LOCK.lock().await;
    std::env::set_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN", "1");

    let coordinator = Coordinator::new();
    force_microphone_input_for_test(&coordinator);
    let old_session_id = {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Processing;
        state.session_id = session_id(99);
        state.session_id
    };

    coordinator.start_dictation().await.unwrap();

    let state = coordinator.inner.state.lock();
    assert_eq!(state.phase, SessionPhase::Processing);
    assert_eq!(state.session_id, old_session_id);

    std::env::remove_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN");
}

#[test]
fn window_key_matcher_mirrors_windows_trigger_aliases() {
    let cases = [
        (HotkeyTrigger::RightControl, "Control", "ControlRight"),
        (HotkeyTrigger::LeftControl, "Control", "ControlLeft"),
        (HotkeyTrigger::RightOption, "Alt", "AltRight"),
        (HotkeyTrigger::RightAlt, "AltGraph", "AltRight"),
        (HotkeyTrigger::RightCommand, "Meta", "MetaRight"),
        (HotkeyTrigger::LeftOption, "Alt", "AltLeft"),
        // Mirrors Windows trigger_to_vk_code aliases.
        (HotkeyTrigger::Fn, "Control", "ControlRight"),
    ];
    for (trigger, key, code) in cases {
        assert!(
            window_key_matches_trigger(trigger, key, code),
            "{trigger:?} should match {key}/{code}"
        );
    }

    assert!(!window_key_matches_trigger(
        HotkeyTrigger::RightControl,
        "Control",
        "ControlLeft"
    ));
    assert!(!window_key_matches_trigger(
        HotkeyTrigger::LeftOption,
        "Alt",
        "AltRight"
    ));
    assert!(!window_key_matches_trigger(HotkeyTrigger::Fn, "Fn", "Fn"));
}

#[test]
fn foundry_local_provider_is_keyless_and_not_whisper_compatible() {
    #[cfg(target_os = "windows")]
    assert!(is_keyless_local_asr_provider(
        crate::asr::local::foundry::PROVIDER_ID
    ));
    #[cfg(not(target_os = "windows"))]
    assert!(!is_keyless_local_asr_provider(
        crate::asr::local::foundry::PROVIDER_ID
    ));
    assert!(!is_whisper_compatible_provider(
        crate::asr::local::foundry::PROVIDER_ID
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn coordinator_shares_app_foundry_runtime() {
    let runtime = Arc::new(crate::asr::local::FoundryLocalRuntime::new());
    let coordinator = Coordinator::new_with_foundry_runtime(Arc::clone(&runtime));

    assert!(Arc::ptr_eq(
        &runtime,
        &coordinator.inner.foundry_local_runtime
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn foundry_transcribe_skips_global_timeout_for_first_run_provisioning() {
    let provider = Arc::new(crate::asr::local::FoundryLocalWhisperAsr::new(
        Arc::new(crate::asr::local::FoundryLocalRuntime::new()),
        crate::asr::local::foundry::DEFAULT_MODEL_ALIAS.to_string(),
        "auto".to_string(),
        None,
    ));
    let active_asr = ActiveAsr::FoundryLocalWhisper(provider);

    assert!(!asr_transcribe_uses_global_timeout(&active_asr));
}

#[cfg(target_os = "windows")]
#[test]
fn foundry_audio_transcribe_timeout_is_separate_from_prepare() {
    let timeout = foundry_audio_transcribe_timeout_duration();

    assert_eq!(
        timeout,
        std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
    );
}

#[test]
fn local_qwen_timeout_floors_at_global_timeout_for_short_audio() {
    // 5s 录音：5 × 0.6 = 3, +10 = 13, max(15) = 15。短录音保留 15s 兜底。
    assert_eq!(
        local_qwen_transcribe_timeout(5.0),
        std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
    );
}

#[test]
fn local_qwen_timeout_scales_with_audio_duration() {
    // 60s 录音：60 × 0.6 = 36, +10 = 46s。覆盖 RTF ≈ 0.5 的边界。
    assert_eq!(
        local_qwen_transcribe_timeout(60.0),
        std::time::Duration::from_secs(46)
    );
}

#[test]
fn local_qwen_timeout_ceils_partial_seconds() {
    // 10.1s 录音：10.1 × 0.6 = 6.06, ceil = 7, +10 = 17, max(15) = 17。
    assert_eq!(
        local_qwen_transcribe_timeout(10.1),
        std::time::Duration::from_secs(17)
    );
}

#[test]
fn local_qwen_timeout_handles_zero_duration() {
    // 0 时长（空 buffer 边界）：0 × 0.6 = 0, +10 = 10, max(15) = 15。
    assert_eq!(
        local_qwen_transcribe_timeout(0.0),
        std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
    );
}

#[cfg(target_os = "windows")]
#[test]
fn foundry_release_uses_foundry_keep_loaded_preference() {
    let runtime = Arc::new(crate::asr::local::FoundryLocalRuntime::new());
    let coordinator = Coordinator::new_with_foundry_runtime(runtime);
    let mut prefs = coordinator.inner.prefs.get();
    prefs.local_asr_keep_loaded_secs = 3;
    prefs.foundry_local_asr_keep_loaded_secs = 7;
    coordinator.inner.prefs.set(prefs).unwrap();

    assert_eq!(foundry_local_asr_release_keep_secs(&coordinator.inner), 7);
}

#[cfg(target_os = "windows")]
#[test]
fn foundry_release_guard_rejects_stale_session() {
    let runtime = Arc::new(crate::asr::local::FoundryLocalRuntime::new());
    let coordinator = Coordinator::new_with_foundry_runtime(runtime);
    let old_session_id = coordinator.inner.state.lock().session_id;

    assert!(foundry_release_session_is_current(
        &coordinator.inner,
        old_session_id
    ));

    coordinator.inner.state.lock().session_id = new_session_id();

    assert!(!foundry_release_session_is_current(
        &coordinator.inner,
        old_session_id
    ));
}

#[test]
fn resolve_ark_endpoint_rejects_blank_key_without_custom_endpoint() {
    assert_eq!(
        resolve_ark_endpoint_with_policy("ark", "", None)
            .unwrap_err()
            .to_string(),
        "API Key 为空"
    );
}

#[test]
fn resolve_ark_endpoint_rejects_blank_key_with_default_endpoint() {
    assert_eq!(
        resolve_ark_endpoint_with_policy(
            "ark",
            "",
            Some("https://ark.cn-beijing.volces.com/api/v3/chat/completions".to_string()),
        )
        .unwrap_err()
        .to_string(),
        "API Key 为空"
    );
}

#[test]
fn resolve_ark_endpoint_allows_blank_key_with_custom_endpoint() {
    let endpoint = resolve_ark_endpoint_with_policy(
        "custom",
        "",
        Some("https://example.com/v1/chat/completions".to_string()),
    )
    .unwrap();
    assert_eq!(endpoint, "https://example.com/v1/chat/completions");
}

#[test]
fn deferred_asr_bridge_flushes_startup_audio_before_live_chunks() {
    #[derive(Default)]
    struct RecordingConsumer {
        bytes: Mutex<Vec<u8>>,
    }

    impl crate::asr::AudioConsumer for RecordingConsumer {
        fn consume_pcm_chunk(&self, pcm: &[u8]) {
            self.bytes.lock().extend_from_slice(pcm);
        }
    }

    let bridge = DeferredAsrBridge::new();
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[1, 2]);
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[3, 4]);

    let target = Arc::new(RecordingConsumer::default());
    let target_for_attach: Arc<dyn crate::asr::AudioConsumer> = target.clone();
    assert_eq!(bridge.attach(target_for_attach), 4);

    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[5, 6]);
    assert_eq!(&*target.bytes.lock(), &[1, 2, 3, 4, 5, 6]);
}

#[tokio::test]
async fn manual_stop_during_starting_is_queued() {
    let coordinator = Coordinator::new();
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Starting;
        state.pending_stop = false;
    }

    coordinator.stop_dictation().await.unwrap();

    let state = coordinator.inner.state.lock();
    assert_eq!(state.phase, SessionPhase::Starting);
    assert!(state.pending_stop);
}

#[tokio::test]
async fn stop_dictation_from_listening_without_asr_returns_idle() {
    let coordinator = Coordinator::new();
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Listening;
        state.session_id = session_id(123);
    }

    coordinator.stop_dictation().await.unwrap();

    assert_eq!(coordinator.inner.state.lock().phase, SessionPhase::Idle);
}

#[test]
fn cancel_session_state_machine_is_table_driven() {
    let cases = [
        (SessionPhase::Idle, SessionPhase::Idle, false),
        (SessionPhase::Starting, SessionPhase::Idle, true),
        (SessionPhase::Listening, SessionPhase::Idle, true),
        (SessionPhase::Processing, SessionPhase::Idle, true),
        (SessionPhase::Inserting, SessionPhase::Inserting, false),
    ];

    for (initial, expected_phase, expected_cancelled) in cases {
        let coordinator = Coordinator::new();
        {
            let mut state = coordinator.inner.state.lock();
            state.phase = initial;
            state.cancelled = false;
            state.focus_target = Some(1);
        }

        coordinator.cancel_dictation();

        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, expected_phase, "initial={initial:?}");
        assert_eq!(state.cancelled, expected_cancelled, "initial={initial:?}");
        if matches!(initial, SessionPhase::Starting | SessionPhase::Listening) {
            assert!(state.focus_target.is_none(), "initial={initial:?}");
        }
    }
}

#[test]
fn recorder_runtime_error_aborts_active_session() {
    let coordinator = Coordinator::new();
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    abort_recording_with_error(&coordinator.inner, "录音中断: stream failed".to_string());

    let state = coordinator.inner.state.lock();
    assert_eq!(state.phase, SessionPhase::Idle);
    assert!(state.cancelled);
    assert!(coordinator.inner.recorder.lock().is_none());
    assert!(coordinator.inner.asr.lock().is_none());
}

#[test]
fn abort_recording_keeps_session_non_idle_until_restore_can_run() {
    let mut state = SessionState::default();
    state.phase = SessionPhase::Listening;
    state.cancelled = false;
    state.session_id = session_id(7);

    let abort = begin_recording_abort_before_restore(&mut state).unwrap();

    assert_eq!(abort.session_id, session_id(7));
    assert!(state.cancelled);
    assert_eq!(state.phase, SessionPhase::Listening);

    publish_abort_idle_after_restore(&mut state, abort.session_id);

    assert_eq!(state.phase, SessionPhase::Idle);
}

#[tokio::test]
async fn pressed_edge_during_inserting_does_not_start_new_session() {
    let coordinator = Coordinator::new();
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Inserting;
        state.session_id = session_id(41);
    }

    handle_pressed_edge(&coordinator.inner).await;

    let state = coordinator.inner.state.lock();
    assert_eq!(state.phase, SessionPhase::Inserting);
    assert_eq!(state.session_id, session_id(41));
}

#[tokio::test]
async fn repeated_pressed_edge_during_hold_session_does_not_restart() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .prefs
        .set(crate::types::UserPreferences {
            hotkey: crate::types::HotkeyBinding {
                trigger: HotkeyTrigger::RightControl,
                mode: HotkeyMode::Hold,
                keys: None,
            },
            ..Default::default()
        })
        .unwrap();
    coordinator.inner.state.lock().phase = SessionPhase::Listening;
    coordinator
        .inner
        .hotkey_trigger_held
        .store(true, Ordering::SeqCst);

    handle_pressed_edge(&coordinator.inner).await;

    assert_eq!(
        coordinator.inner.state.lock().phase,
        SessionPhase::Listening
    );
    assert!(coordinator.inner.hotkey_trigger_held.load(Ordering::SeqCst));
}

#[test]
fn enabling_shortcut_recording_clears_dictation_hold_latch() {
    let coordinator = Coordinator::new();
    coordinator
        .inner
        .hotkey_trigger_held
        .store(true, Ordering::SeqCst);

    coordinator.set_shortcut_recording_active(true);

    assert!(!coordinator.inner.hotkey_trigger_held.load(Ordering::SeqCst));
}

#[test]
fn window_hotkey_fallback_is_disabled_when_no_explicit_fallback_is_advertised() {
    assert_eq!(
        window_hotkey_fallback_enabled(),
        crate::types::HotkeyCapability::current().explicit_fallback_available
    );
}

#[test]
#[cfg(target_os = "windows")]
fn prepared_windows_ime_slot_is_taken_only_for_matching_session() {
    let mut slots = vec![PreparedWindowsImeSessionSlot {
        session_id: session_id(2),
        prepared: PreparedWindowsImeSession::unavailable(),
    }];

    assert!(take_matching_prepared_windows_ime_session(&mut slots, session_id(1)).is_none());
    assert_eq!(
        slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
        vec![session_id(2)]
    );

    assert!(take_matching_prepared_windows_ime_session(&mut slots, session_id(2)).is_some());
    assert!(slots.is_empty());
}

#[test]
#[cfg(target_os = "windows")]
fn prepared_windows_ime_sessions_keep_overlapping_snapshots() {
    let mut slots = Vec::new();
    store_prepared_windows_ime_session(
        &mut slots,
        session_id(1),
        PreparedWindowsImeSession::unavailable(),
    );
    store_prepared_windows_ime_session(
        &mut slots,
        session_id(2),
        PreparedWindowsImeSession::unavailable(),
    );

    assert_eq!(
        slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
        vec![session_id(1), session_id(2)]
    );

    assert!(take_matching_prepared_windows_ime_session(&mut slots, session_id(1)).is_some());
    assert_eq!(
        slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
        vec![session_id(2)]
    );
}

#[test]
#[cfg(target_os = "windows")]
fn stale_prepared_windows_ime_restore_discards_old_snapshot_without_restoring() {
    let mut slots = Vec::new();
    store_prepared_windows_ime_session(
        &mut slots,
        session_id(1),
        PreparedWindowsImeSession::unavailable(),
    );
    store_prepared_windows_ime_session(
        &mut slots,
        session_id(2),
        PreparedWindowsImeSession::unavailable(),
    );

    assert!(take_current_prepared_windows_ime_session_for_restore(
        &mut slots,
        session_id(1),
        session_id(2)
    )
    .is_none());
    assert_eq!(
        slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
        vec![session_id(2)]
    );
}

#[test]
#[cfg(target_os = "windows")]
fn non_tsf_insertion_fallback_gate_blocks_only_when_disabled() {
    assert!(should_try_non_tsf_insertion_fallback(
        true,
        InsertStatus::CopiedFallback
    ));
    assert!(should_try_non_tsf_insertion_fallback(
        true,
        InsertStatus::Failed
    ));
    assert!(!should_try_non_tsf_insertion_fallback(
        true,
        InsertStatus::Inserted
    ));
    assert!(!should_try_non_tsf_insertion_fallback(
        false,
        InsertStatus::CopiedFallback
    ));
    assert!(!should_try_non_tsf_insertion_fallback(
        false,
        InsertStatus::Failed
    ));
}

#[test]
fn focus_restore_failure_uses_specific_error_code_when_insert_fails() {
    assert_eq!(
        dictation_error_code(InsertStatus::Failed, false, false, false, false),
        Some("focusRestoreFailed")
    );
}

#[test]
#[cfg(target_os = "windows")]
fn missing_windows_hwnd_is_not_present() {
    use windows::Win32::Foundation::HWND;

    assert!(!windows_hwnd_is_present(HWND::default()));
}

#[test]
#[cfg(target_os = "windows")]
fn tsf_required_failure_keeps_tsf_error_when_focus_was_ready() {
    assert_eq!(
        dictation_error_code(InsertStatus::Failed, false, true, false, false),
        Some("windowsImeTsfRequired")
    );
}

#[test]
fn startup_race_check_treats_newer_session_as_stale() {
    let mut state = SessionState::default();
    state.phase = SessionPhase::Starting;
    state.cancelled = false;
    state.session_id = session_id(2);

    assert_eq!(
        startup_race_status(&state, session_id(1)),
        StartupRaceStatus::StaleContinuation
    );
}

#[test]
fn startup_race_check_is_table_driven_for_begin_session_edges() {
    let cases = [
        (
            SessionPhase::Starting,
            false,
            session_id(7),
            StartupRaceStatus::ActiveStarting,
        ),
        (
            SessionPhase::Starting,
            true,
            session_id(7),
            StartupRaceStatus::CancelRaced,
        ),
        (
            SessionPhase::Idle,
            false,
            session_id(7),
            StartupRaceStatus::CancelRaced,
        ),
        (
            SessionPhase::Listening,
            false,
            session_id(7),
            StartupRaceStatus::CancelRaced,
        ),
        (
            SessionPhase::Starting,
            false,
            session_id(8),
            StartupRaceStatus::StaleContinuation,
        ),
    ];

    for (phase, cancelled, actual_session_id, expected) in cases {
        let mut state = SessionState::default();
        state.phase = phase;
        state.cancelled = cancelled;
        state.session_id = actual_session_id;

        assert_eq!(
            startup_race_status(&state, session_id(7)),
            expected,
            "phase={phase:?} cancelled={cancelled} actual_session={actual_session_id}"
        );
    }
}

#[test]
fn begin_recording_abort_is_noop_after_prior_cancel_or_idle() {
    let cases = [
        (SessionPhase::Idle, false),
        (SessionPhase::Processing, false),
        (SessionPhase::Listening, true),
    ];

    for (phase, cancelled) in cases {
        let mut state = SessionState::default();
        state.phase = phase;
        state.cancelled = cancelled;

        assert!(begin_recording_abort_before_restore(&mut state).is_none());
        assert_eq!(state.phase, phase);
        assert_eq!(state.cancelled, cancelled);
    }
}

#[test]
fn stale_startup_cleanup_keeps_newer_asr_resource() {
    let coordinator = Coordinator::new();
    let newer_asr = Arc::new(WhisperBatchASR::new(
        "key".to_string(),
        "http://localhost".to_string(),
        "model".to_string(),
        None,
    ));
    *coordinator.inner.asr.lock() = Some(SessionResource::new(
        session_id(2),
        ActiveAsr::Whisper(Arc::clone(&newer_asr)),
    ));

    discard_startup_resources_for_session(&coordinator.inner, session_id(1));

    assert_eq!(
        coordinator
            .inner
            .asr
            .lock()
            .as_ref()
            .map(|resource| resource.session_id),
        Some(session_id(2))
    );

    discard_startup_resources_for_session(&coordinator.inner, session_id(2));

    assert!(coordinator.inner.asr.lock().is_none());
}

#[test]
fn capsule_recording_window_ops_are_low_frequency() {
    let mut throttle = CapsuleUiThrottleState::default();
    let start = Instant::now();
    let mut runs = 0;

    for tick in 0..300 {
        let now = start + Duration::from_millis(tick * 33);
        if throttle.should_run_window_ops(
            CapsuleWindowRequest {
                session_id: Some("session-1".to_string()),
                state: CapsuleState::Recording,
                visible: true,
                translation: false,
                show_capsule: true,
            },
            now,
        ) {
            runs += 1;
        }
    }

    assert!(
        runs <= 10,
        "30 Hz Recording ticks over ~10s should produce <= 10 window ops, got {runs}"
    );
}

#[test]
fn capsule_state_transition_bypasses_recording_window_throttle() {
    let mut throttle = CapsuleUiThrottleState::default();
    let start = Instant::now();

    assert!(throttle.should_run_window_ops(
        CapsuleWindowRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Recording,
            visible: true,
            translation: false,
            show_capsule: true,
        },
        start,
    ));
    assert!(!throttle.should_run_window_ops(
        CapsuleWindowRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Recording,
            visible: true,
            translation: false,
            show_capsule: true,
        },
        start + Duration::from_millis(100),
    ));
    assert!(throttle.should_run_window_ops(
        CapsuleWindowRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Transcribing,
            visible: true,
            translation: false,
            show_capsule: true,
        },
        start + Duration::from_millis(100),
    ));
}

#[test]
fn capsule_recording_frontend_events_are_throttled() {
    let mut throttle = CapsuleUiThrottleState::default();
    let start = Instant::now();
    let request = CapsuleFrontendRequest {
        session_id: Some("session-1".to_string()),
        state: CapsuleState::Recording,
        visible: true,
        translation: false,
        show_capsule: true,
        message: None,
        inserted_chars: None,
    };
    let mut emitted = 0;

    for tick in 0..300 {
        let now = start + Duration::from_millis(tick * 2);
        if throttle.should_emit_frontend(request.clone(), now) {
            emitted += 1;
        }
    }

    assert!(
        (10..=13).contains(&emitted),
        "2 ms Recording ticks over ~600 ms should emit at about 20 Hz, got {emitted}"
    );
}

#[test]
fn capsule_recording_diagnostics_are_sampled_but_state_and_text_are_retained() {
    let mut throttle = CapsuleUiThrottleState::default();
    let start = Instant::now();
    let recording = CapsulePayload {
        seq: 1,
        session_id: Some("session-1".to_string()),
        state: CapsuleState::Recording,
        level: 0.2,
        elapsed_ms: 0,
        message: None,
        inserted_chars: None,
        translation: false,
    };

    assert!(throttle.should_record_backend_emit(&recording, start));
    assert!(!throttle.should_record_backend_emit(&recording, start + Duration::from_millis(950),));
    assert!(throttle.should_record_backend_emit(&recording, start + Duration::from_secs(1),));

    let preview = CapsulePayload {
        message: Some("preview".to_string()),
        ..recording
    };
    assert!(throttle.should_record_backend_emit(&preview, start + Duration::from_millis(1010),));
}

#[test]
fn capsule_recording_level_ticks_continue_after_preview_payload() {
    let mut throttle = CapsuleUiThrottleState::default();
    let start = Instant::now();
    let preview = CapsuleFrontendRequest {
        session_id: Some("session-1".to_string()),
        state: CapsuleState::Recording,
        visible: true,
        translation: false,
        show_capsule: true,
        message: Some("preview".to_string()),
        inserted_chars: None,
    };
    let level_tick = CapsuleFrontendRequest {
        message: None,
        ..preview.clone()
    };

    assert!(throttle.should_emit_frontend(preview, start));
    assert!(throttle.should_emit_frontend(level_tick.clone(), start + Duration::from_millis(10),));
    assert!(!throttle.should_emit_frontend(level_tick.clone(), start + Duration::from_millis(40),));
    assert!(throttle.should_emit_frontend(level_tick, start + Duration::from_millis(60),));
}

#[test]
fn capsule_frontend_state_transition_bypasses_throttle() {
    let mut throttle = CapsuleUiThrottleState::default();
    let start = Instant::now();

    assert!(throttle.should_emit_frontend(
        CapsuleFrontendRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Recording,
            visible: true,
            translation: false,
            show_capsule: true,
            message: None,
            inserted_chars: None,
        },
        start,
    ));
    assert!(!throttle.should_emit_frontend(
        CapsuleFrontendRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Recording,
            visible: true,
            translation: false,
            show_capsule: true,
            message: None,
            inserted_chars: None,
        },
        start + Duration::from_millis(10),
    ));
    assert!(throttle.should_emit_frontend(
        CapsuleFrontendRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Transcribing,
            visible: true,
            translation: false,
            show_capsule: true,
            message: None,
            inserted_chars: None,
        },
        start + Duration::from_millis(10),
    ));
}

#[test]
fn capsule_frontend_text_change_bypasses_level_throttle() {
    let mut throttle = CapsuleUiThrottleState::default();
    let start = Instant::now();

    assert!(throttle.should_emit_frontend(
        CapsuleFrontendRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Recording,
            visible: true,
            translation: false,
            show_capsule: true,
            message: Some("第一段".to_string()),
            inserted_chars: None,
        },
        start,
    ));
    assert!(!throttle.should_emit_frontend(
        CapsuleFrontendRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Recording,
            visible: true,
            translation: false,
            show_capsule: true,
            message: Some("第一段".to_string()),
            inserted_chars: None,
        },
        start + Duration::from_millis(10),
    ));
    assert!(throttle.should_emit_frontend(
        CapsuleFrontendRequest {
            session_id: Some("session-1".to_string()),
            state: CapsuleState::Recording,
            visible: true,
            translation: false,
            show_capsule: true,
            message: Some("第二段".to_string()),
            inserted_chars: None,
        },
        start + Duration::from_millis(10),
    ));
}

#[test]
fn embedded_ble_pcm_capsule_trace_is_sampled() {
    let mut trace = EmbeddedBlePcmCapsuleTraceState::default();
    let start = Instant::now();
    let session_a = session_id(1);
    let session_b = session_id(2);

    assert!(trace.should_trace(session_a, false, start));
    assert!(!trace.should_trace(session_a, false, start + Duration::from_millis(100)));
    assert!(trace.should_trace(session_a, false, start + Duration::from_millis(500)));
    assert!(trace.should_trace(session_b, false, start + Duration::from_millis(510)));
    assert!(trace.should_trace(session_b, true, start + Duration::from_millis(520)));
}
