use super::device::*;
#[cfg(target_os = "windows")]
use super::release_foundry_runtime_if_inactive;
use super::*;
use super::{
    active_asr_is_keyless_for_validation, active_foundry_model_from_prefs,
    asr_configured_for_provider, asr_transcriptions_url, diagnostic_recent_errors,
    fetch_provider_models, fetch_provider_models_cached, is_diagnostic_error_line,
    is_gemini_base_url, is_valid_local_pack_id, is_valid_session_id, llm_configured_for_provider,
    local_asr_release_plan_for_provider, models_url, normalize_foundry_language_hint,
    parse_gemini_model_ids, parse_model_ids, persist_settings, provider_models_cache,
    sanitize_diagnostic_log_line, validate_foundry_model_alias, ProviderConfig, SettingsWriter,
};
use crate::coordinator::Coordinator;
use crate::embedded_audio::{SessionEndReason, SessionErrorCode, SessionStats};
use crate::embedded_ble::FirmwareOtaDeviceSnapshot;
use crate::persistence::CredentialsSnapshot;
use crate::polish::ProviderProxyConfig;
use crate::types::{
    ComboBinding, DeviceCustomKeyAction, DeviceCustomKeyMapping, DictationSession, HotkeyBinding,
    HotkeyMode, HotkeyTrigger, InsertStatus, PolishMode, ShortcutBinding, UserPreferences,
    MAX_DEVICE_LOW_POWER_IDLE_MINUTES,
};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::thread;

static PROVIDER_MODELS_CACHE_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn provider_models_cache_test_lock() -> &'static tokio::sync::Mutex<()> {
    PROVIDER_MODELS_CACHE_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn normalized_commands_source() -> String {
    let mut s = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/commands/mod.rs"))
        .replace("\r\n", "\n");
    s.push('\n');
    for part in [
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/style_pack_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/local_asr_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/diagnostics_export.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/marketplace.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/device/mod.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/device/settings.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/device/ble.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/commands/device/firmware.rs"
        )),
    ] {
        s.push_str(&part.replace("\r\n", "\n"));
        s.push('\n');
    }
    s
}

#[test]
fn wake_phrase_change_is_prepared_and_invalidated_before_persistence() {
    let source = normalized_commands_source();
    let start = source
        .find("pub fn set_settings")
        .expect("set_settings command must exist");
    let end = source[start..]
        .find("fn refresh_tray_menu_async")
        .map(|offset| start + offset)
        .expect("set_settings command boundary must exist");
    let body = &source[start..end];
    let prepare = body
        .find("crate::wake_phrase::prepare(&next_wake_phrase)")
        .expect("changed wake phrase must be prewarmed");
    let invalidate = body
        .find("crate::speaker_verification::invalidate_for_phrase_change")
        .expect("changed wake phrase must invalidate the old voiceprint");
    let persist = body
        .find("persist_settings(&*coord, prefs.clone())")
        .expect("normalized wake phrase must be persisted");

    assert!(
        prepare < invalidate && invalidate < persist,
        "wake phrase changes must prewarm first, invalidate the old voiceprint second, and persist last"
    );
}

#[test]
fn ota_observability_handoff_precedes_background_listener_pause() {
    let source = normalized_commands_source();
    // mod.rs may re-export the symbol; use the real implementation body in firmware.rs.
    let transfer_start = source
        .rfind("pub async fn transfer_firmware_ota_ble")
        .expect("firmware OTA command should exist");
    let transfer_end = source[transfer_start..]
        .find("Ok(FirmwareOtaBleTransferResult")
        .map(|offset| transfer_start + offset)
        .expect("firmware OTA command should return its result");
    let transfer = &source[transfer_start..transfer_end];
    let handoff = transfer
        .find("request_listener_ota_v1_active_link(Some(")
        .expect("OTA should hand off its observability context on the active link");
    let begin = transfer
        .find("coord.try_begin_firmware_ota_transfer()")
        .expect("OTA transfer must reserve exclusive OTA after TYPE:OTA handoff");
    let pause = transfer
        .find(".pause_embedded_ble_listener_for_ota(FIRMWARE_OTA_LISTENER_RELEASE_TIMEOUT)")
        .expect("OTA should pause the listener before opening its GATT transfer");
    let target_prepare = transfer
        .find("let target_prepare_started = Instant::now();")
        .expect("OTA should prepare its GATT target after the listener is fully released");

    assert!(
        handoff < begin && begin < pause && pause < target_prepare,
        "TYPE:OTA must reach firmware while notify is live, then OTA must release notify before target preparation"
    );
}

#[test]
fn ota_preflight_reuses_active_link_handoff_without_interrupting_notify() {
    let source = normalized_commands_source();
    let start = source
        .find("pub async fn get_firmware_ota_preflight_snapshot")
        .expect("firmware OTA preflight command should exist");
    let end = source[start..]
        .find("pub struct FirmwareOtaBleTransferResult")
        .map(|offset| start + offset)
        .expect("firmware OTA preflight command boundary should exist");
    let preflight = &source[start..end];
    let recording_guard = preflight
        .find("if phase != SessionPhase::Idle")
        .expect("preflight must reject active dictation before changing the BLE link");
    let handoff = preflight
        .find("request_listener_ota_v1_active_link(None)")
        .expect("preflight must request the firmware OTA active-link handoff");
    let begin = preflight
        .find("coord.try_begin_firmware_ota_transfer()")
        .expect("preflight must reserve the OTA operation after live handoff");
    let probe = preflight
        .find("listener_ota_v1_gatt_probe_after_active_link_hint")
        .expect("preflight must probe through the verified active-link handoff path");
    let soft_fail_probe = preflight
        .find("listener_ota_v1_gatt_probe_snapshot")
        .expect("preflight must soft-fail TYPE:OTA and still probe OTA GATT for firmware info");
    let end_guard = preflight
        .find("coord.end_failed_firmware_ota_transfer();")
        .expect("preflight must release its OTA reservation through bonded recovery");
    assert!(recording_guard < handoff);
    // Live notify TYPE:OTA first; try_begin suppresses capture and must not run earlier.
    assert!(
        handoff < begin && begin < probe && probe < end_guard && soft_fail_probe < end_guard,
        "preflight must TYPE:OTA on live notify before try_begin, then always GATT-probe"
    );
    // Listener restore is owned by the non-destructive OTA recovery path.
    assert!(
        preflight.contains("end_failed_firmware_ota_transfer"),
        "preflight must end OTA without allowing transient CCCD errors to delete pairing"
    );
}

#[test]
fn ota_preflight_reports_handoff_and_target_probe_timing_without_transfer() {
    let source = normalized_commands_source();
    let start = source
        .find("pub async fn get_firmware_ota_preflight_snapshot")
        .expect("firmware OTA preflight command should exist");
    let end = source[start..]
        .find("pub struct FirmwareOtaBleTransferResult")
        .map(|offset| start + offset)
        .expect("firmware OTA preflight command boundary should exist");
    let preflight = &source[start..end];
    let handoff_started = preflight
        .find("let handoff_started = Instant::now();")
        .expect("preflight must start its handoff timer before the active-link command");
    let handoff = preflight
        .find("request_listener_ota_v1_active_link(None)")
        .expect("preflight must send the active-link command");
    let probe_started = preflight
        .find("let target_probe_started = Instant::now();")
        .expect("preflight must start its target probe timer after the handoff");
    let probe = preflight
        .find("listener_ota_v1_gatt_probe_after_active_link_hint")
        .expect("preflight must probe the target without starting a transfer");

    assert!(handoff_started < handoff && handoff < probe_started && probe_started < probe);
    assert!(preflight.contains("handoff_elapsed_ms: Some(handoff_elapsed_ms)"));
    assert!(preflight.contains("target_probe_elapsed_ms,"));
    assert!(preflight.contains("total_elapsed_ms,"));
    assert!(preflight.contains("[ota] active-link preflight timing"));
    assert!(
        !preflight.contains("prepared.transfer("),
        "readiness preflight must not write OTA control or payload data"
    );
}

#[test]
fn ota_transfer_failure_restores_the_background_listener() {
    let source = normalized_commands_source();
    // Prefer the device/firmware implementation, not the thin commands/mod.rs re-export.
    let transfer_start = source
        .rfind("pub async fn transfer_firmware_ota_ble")
        .expect("firmware OTA command should exist");
    let transfer_end = source[transfer_start..]
        .find("Ok(FirmwareOtaBleTransferResult")
        .map(|offset| transfer_start + offset)
        .expect("firmware OTA command should return its result");
    let transfer = &source[transfer_start..transfer_end];
    let success = transfer
        .find("end_firmware_ota_transfer_with_listener_restore(false)")
        .expect("successful OTA must clear gate without a racing first listener restore");
    let result = transfer
        .find("let stats = transfer?;")
        .expect("OTA transfer should propagate its result after cleanup");
    let end_fail = transfer[..result]
        .rfind("coord.end_failed_firmware_ota_transfer();")
        .expect("failed OTA transfer must restore through bonded recovery");
    let after_ota = transfer
        .find("refresh_embedded_ble_listener_after_firmware_ota")
        .expect("successful OTA must use post-confirm listener restore");
    let target_ready = transfer
        .find("Listener OTA v1 target prepared after listener release")
        .expect("OTA target must be prepared after listener release");
    let pause = transfer
        .find(".pause_embedded_ble_listener_for_ota(FIRMWARE_OTA_LISTENER_RELEASE_TIMEOUT)")
        .expect("OTA must pause the listener before target preparation");
    assert!(
        transfer.contains("log::error!") && transfer.contains("[firmware-ota] transfer failed"),
        "OTA transfer failures must log the full host error string for operator diagnostics"
    );
    let transfer_timer = transfer
        .find("let transfer_started = Instant::now();")
        .expect("OTA transfer timing must start after the listener is paused");

    // Success: the retained stable ATT proof permits a direct single after-OTA restore.
    assert!(success < result && result < after_ota);
    assert!(!transfer.contains("FIRMWARE_OTA_POST_CONFIRM_SETTLE"));
    // Failure: the coordinator owns one non-destructive restore before propagation.
    assert!(end_fail < result);
    assert!(
        transfer[..result]
            .matches("coord.end_failed_firmware_ota_transfer();")
            .count()
            >= 4,
        "every post-handoff failure exit must preserve the existing pairing"
    );
    assert!(pause < target_ready && target_ready < transfer_timer);
}

#[test]
fn ota_result_separates_protocol_transfer_from_generation_recovery_time() {
    let source = normalized_commands_source();
    let transfer_start = source
        .rfind("pub async fn transfer_firmware_ota_ble")
        .expect("firmware OTA command should exist");
    let transfer_end = source[transfer_start..]
        .find("Ok(FirmwareOtaBleTransferResult")
        .map(|offset| transfer_start + offset)
        .expect("firmware OTA command should return its result");
    let transfer = &source[transfer_start..transfer_end];

    assert!(
        transfer.contains("let transfer_elapsed_ms = stats.protocol_transfer_elapsed_ms;")
            && transfer.contains("transfer_wall_elapsed_ms.saturating_sub(transfer_elapsed_ms)"),
        "reported transfer time must come from the OTA protocol engine"
    );
    assert!(
        transfer.contains(".saturating_add(transfer_fixed_elapsed_ms)")
            && transfer.contains("transfer_fixed_ms={}"),
        "secure reopen and reboot/ATT generation waiting must remain in the fixed-time gate"
    );
}

fn ota_snapshot_with_version(version: Option<&str>) -> FirmwareOtaDeviceSnapshot {
    FirmwareOtaDeviceSnapshot {
        connected: true,
        hardware_revision: Some("keyboard-v1".to_string()),
        firmware_version: version.map(str::to_string),
        capabilities: vec![crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY.to_string()],
        battery_percent: Some(80),
        usb_powered: Some(true),
        detail: None,
    }
}

#[test]
fn firmware_ota_confirmation_uses_fast_service_probe_before_full_gatt_fallback() {
    let source = normalized_commands_source();
    let start = source
        .find("async fn confirm_listener_ota_v1_reachable")
        .expect("OTA confirmation helper should exist");
    let end = source[start..]
        .find("pub fn load_firmware_ota_package")
        .map(|offset| start + offset)
        .expect("OTA confirmation helper should end before the next device-firmware helper");
    let body = &source[start..end];
    let fast_probe = body
        .find("listener_ota_v1_service_reachable_snapshot")
        .expect("OTA confirmation must first use the service-only reachability probe");
    let full_probe = body
        .find("listener_ota_v1_gatt_probe_snapshot")
        .expect("OTA confirmation must retain the complete GATT fallback");
    assert!(
        fast_probe < full_probe,
        "the fast service-only probe must remain ahead of the complete GATT fallback"
    );
}

#[test]
fn firmware_ota_version_match_accepts_dis_v_prefix() {
    assert!(firmware_ota_versions_match(" v1.2.0 ", "1.2.0"));
    assert!(firmware_ota_versions_match("1.2.0", "v1.2.0"));
    assert!(!firmware_ota_versions_match("1.2.0-dev", "1.2.0"));
    assert!(!firmware_ota_versions_match("", "1.2.0"));
}

#[test]
fn firmware_ota_snapshot_version_reads_dis_firmware_revision() {
    assert_eq!(
        firmware_ota_snapshot_version(&ota_snapshot_with_version(Some(" v1.2.0 "))),
        Some("v1.2.0".to_string())
    );
    assert_eq!(
        firmware_ota_snapshot_version(&ota_snapshot_with_version(Some("   "))),
        None
    );
    assert_eq!(
        firmware_ota_snapshot_version(&ota_snapshot_with_version(None)),
        None
    );
}

#[test]
fn device_settings_request_accepts_safe_values() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener-dev".to_string(),
    };

    assert!(validate_device_settings_request(&request).is_ok());
}

#[test]
fn device_settings_request_rejects_unsafe_ble_name() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener=bad".to_string(),
    };

    assert!(validate_device_settings_request(&request).is_err());
}

#[test]
fn device_settings_request_rejects_ble_name_spaces() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener dev".to_string(),
    };

    assert!(validate_device_settings_request(&request).is_err());
}

#[test]
fn device_settings_request_rejects_ble_name_too_long_for_advertising() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener-1234567890123456789012".to_string(),
    };

    assert!(validate_device_settings_request(&request).is_err());
}

#[test]
fn device_settings_request_rejects_invalid_low_power_idle() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: MAX_DEVICE_LOW_POWER_IDLE_MINUTES + 1,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener-dev".to_string(),
    };

    assert!(validate_device_settings_request(&request).is_err());
}

#[test]
fn device_settings_request_rejects_plugged_auto_shutdown() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 30,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener-dev".to_string(),
    };

    assert!(validate_device_settings_request(&request).is_err());
}

#[test]
fn device_settings_firmware_readback_syncs_local_preferences_exactly() {
    let mut prefs = crate::types::UserPreferences::default();
    prefs.device_status_led_brightness_percent = 80;
    prefs.device_key_led_brightness_percent = 80;
    prefs.device_knob_led_brightness_percent = 100;
    prefs.device_edge_led_brightness_percent = 100;
    prefs.device_plugged_low_power_idle_minutes = 1;
    prefs.device_battery_low_power_idle_minutes = 1;
    prefs.device_low_power_idle_minutes = 1;
    prefs.device_battery_auto_shutdown_minutes = 10;
    prefs.device_ble_name = "listener".to_string();

    let snapshot = DeviceSettingsSnapshot {
        schema: "listener.device_settings.v1",
        connected: true,
        write_supported: true,
        source: "firmware",
        status_led_brightness_percent: 66,
        key_led_brightness_percent: 47,
        knob_led_brightness_percent: 91,
        edge_led_brightness_percent: 83,
        led_zone_brightness_supported: true,
        compact_set_supported: false,
        low_power_idle_minutes: 3,
        plugged_low_power_idle_minutes: 3,
        battery_low_power_idle_minutes: 1,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_ms: 0,
        battery_auto_shutdown_ms: 18 * 60_000,
        knob_rotation_action: "screenBrightness".to_string(),
        ble_name: "3xczcC6WuYC3".to_string(),
        ble_name_pending_restart: false,
        active_power_source: "plugged",
        battery_percent: Some(100),
        detail: None,
        last_updated_at: None,
    };

    assert!(apply_device_settings_snapshot_to_preferences(
        &mut prefs, &snapshot
    ));
    assert_eq!(prefs.device_status_led_brightness_percent, 66);
    assert_eq!(prefs.device_key_led_brightness_percent, 47);
    assert_eq!(prefs.device_knob_led_brightness_percent, 91);
    assert_eq!(prefs.device_edge_led_brightness_percent, 83);
    assert_eq!(prefs.device_plugged_low_power_idle_minutes, 3);
    assert_eq!(prefs.device_battery_low_power_idle_minutes, 1);
    assert_eq!(prefs.device_low_power_idle_minutes, 1);
    assert!(prefs.device_plugged_low_power_enabled);
    assert_eq!(prefs.device_battery_auto_shutdown_minutes, 18);
    assert_eq!(
        prefs.device_knob_rotation_action,
        crate::types::DeviceKnobRotationAction::ScreenBrightness
    );
    assert_eq!(prefs.device_ble_name, "3xczcC6WuYC3");

    let mut pending_snapshot = snapshot;
    pending_snapshot.ble_name = "pending-name".to_string();
    pending_snapshot.ble_name_pending_restart = true;
    assert!(!apply_device_settings_snapshot_to_preferences(
        &mut prefs,
        &pending_snapshot
    ));
    assert_eq!(
        prefs.device_ble_name, "3xczcC6WuYC3",
        "Type must not switch its BLE target to a name firmware has not started advertising"
    );
}

#[test]
fn device_settings_snapshot_uses_firmware_readback_values() {
    let snapshot =
        device_settings_snapshot_from_status(crate::embedded_ble::DeviceSettingsStatus {
            brightness_percent: 80,
            plugged_brightness_percent: 80,
            battery_brightness_percent: 80,
            status_led_brightness_percent: 70,
            key_led_brightness_percent: 65,
            knob_led_brightness_percent: 60,
            edge_led_brightness_percent: 55,
            led_zone_brightness_supported: true,
            compact_set_supported: false,
            low_power_idle_minutes: 3,
            plugged_low_power_idle_minutes: 2,
            battery_low_power_idle_minutes: 3,
            plugged_low_power_enabled: true,
            voice_auto_start_enabled: false,
            voice_auto_stop_enabled: false,
            plugged_auto_shutdown_minutes: 0,
            battery_auto_shutdown_minutes: 30,
            settings_revision: 42,
            knob_rotation_action: "screen_brightness".to_string(),
            ble_name: "listener-dev".to_string(),
            ble_name_pending_restart: true,
            external_power_present: true,
            usb_power_present: true,
            charging: false,
            charge_full: false,
            raw_line: "~DEVICE:SETTINGS result=OK".to_string(),
        });

    assert_eq!(snapshot.source, "firmware");
    assert_eq!(snapshot.status_led_brightness_percent, 70);
    assert_eq!(snapshot.key_led_brightness_percent, 65);
    assert_eq!(snapshot.knob_led_brightness_percent, 60);
    assert_eq!(snapshot.edge_led_brightness_percent, 55);
    assert!(snapshot.led_zone_brightness_supported);
    assert_eq!(snapshot.low_power_idle_minutes, 3);
    assert_eq!(snapshot.plugged_low_power_idle_minutes, 2);
    assert_eq!(snapshot.battery_low_power_idle_minutes, 3);
    assert_eq!(snapshot.plugged_auto_shutdown_ms, 0);
    assert_eq!(snapshot.battery_auto_shutdown_ms, 30 * 60_000);
    assert_eq!(snapshot.knob_rotation_action, "screenBrightness");
    assert_eq!(snapshot.ble_name, "listener-dev");
    assert!(snapshot.ble_name_pending_restart);
    assert_eq!(snapshot.active_power_source, "plugged");
}

#[test]
fn device_ble_name_change_detection_prefers_authoritative_snapshot() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener-dev".to_string(),
    };
    let mut snapshot =
        device_settings_snapshot_from_status(crate::embedded_ble::DeviceSettingsStatus {
            brightness_percent: 80,
            plugged_brightness_percent: 80,
            battery_brightness_percent: 80,
            status_led_brightness_percent: 70,
            key_led_brightness_percent: 65,
            knob_led_brightness_percent: 60,
            edge_led_brightness_percent: 55,
            led_zone_brightness_supported: true,
            compact_set_supported: false,
            low_power_idle_minutes: 3,
            plugged_low_power_idle_minutes: 2,
            battery_low_power_idle_minutes: 3,
            plugged_low_power_enabled: true,
            voice_auto_start_enabled: false,
            voice_auto_stop_enabled: false,
            plugged_auto_shutdown_minutes: 0,
            battery_auto_shutdown_minutes: 30,
            settings_revision: 42,
            knob_rotation_action: "screen_brightness".to_string(),
            ble_name: "listener-dev".to_string(),
            ble_name_pending_restart: false,
            external_power_present: true,
            usb_power_present: true,
            charging: false,
            charge_full: false,
            raw_line: "~DEVICE:SETTINGS result=OK".to_string(),
        });

    assert!(!device_ble_name_changed_for_request(
        &request,
        Some(&snapshot),
        Some("listener-old"),
    ));

    snapshot.ble_name = "listener-old".to_string();
    assert!(device_ble_name_changed_for_request(
        &request,
        Some(&snapshot),
        Some("listener-dev"),
    ));

    snapshot.source = "defaults";
    assert!(!device_ble_name_changed_for_request(
        &request,
        Some(&snapshot),
        Some("listener-dev"),
    ));
    assert!(device_ble_name_changed_for_request(
        &request,
        Some(&snapshot),
        Some("listener-old"),
    ));
}

#[test]
fn device_ble_name_apply_runs_for_pending_same_name() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "Blistener".to_string(),
    };
    let mut snapshot =
        device_settings_snapshot_from_status(crate::embedded_ble::DeviceSettingsStatus {
            brightness_percent: 80,
            plugged_brightness_percent: 80,
            battery_brightness_percent: 80,
            status_led_brightness_percent: 70,
            key_led_brightness_percent: 65,
            knob_led_brightness_percent: 60,
            edge_led_brightness_percent: 55,
            led_zone_brightness_supported: true,
            compact_set_supported: false,
            low_power_idle_minutes: 3,
            plugged_low_power_idle_minutes: 2,
            battery_low_power_idle_minutes: 3,
            plugged_low_power_enabled: true,
            voice_auto_start_enabled: false,
            voice_auto_stop_enabled: false,
            plugged_auto_shutdown_minutes: 0,
            battery_auto_shutdown_minutes: 30,
            settings_revision: 42,
            knob_rotation_action: "screen_brightness".to_string(),
            ble_name: "Blistener".to_string(),
            ble_name_pending_restart: true,
            external_power_present: true,
            usb_power_present: true,
            charging: false,
            charge_full: false,
            raw_line: "~DEVICE:SETTINGS result=OK".to_string(),
        });

    assert!(device_ble_name_apply_needed(
        &request,
        Some(&snapshot),
        false,
    ));

    snapshot.ble_name_pending_restart = false;
    assert!(!device_ble_name_apply_needed(
        &request,
        Some(&snapshot),
        false,
    ));
    assert!(device_ble_name_apply_needed(
        &request,
        Some(&snapshot),
        true,
    ));
}

#[test]
fn device_ble_name_apply_lost_ack_requires_matching_readback() {
    let mut status = crate::embedded_ble::DeviceSettingsStatus {
        brightness_percent: 80,
        plugged_brightness_percent: 80,
        battery_brightness_percent: 80,
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        led_zone_brightness_supported: true,
        compact_set_supported: false,
        low_power_idle_minutes: 3,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        settings_revision: 42,
        knob_rotation_action: "screen_brightness".to_string(),
        ble_name: "Blistener".to_string(),
        ble_name_pending_restart: false,
        external_power_present: true,
        usb_power_present: true,
        charging: false,
        charge_full: false,
        raw_line: "~DEVICE:SETTINGS result=OK".to_string(),
    };

    assert!(device_ble_name_apply_confirmed_by_status(
        &status,
        "Blistener"
    ));
    assert!(!device_ble_name_apply_confirmed_by_status(
        &status,
        "BlistAI42"
    ));
    status.ble_name_pending_restart = true;
    assert!(!device_ble_name_apply_confirmed_by_status(
        &status,
        "Blistener"
    ));
}

#[test]
fn device_ble_name_apply_transient_ack_loss_allows_deferred_confirmation() {
    assert!(device_ble_name_apply_error_allows_deferred_confirmation(
        "BLE device settings write async error: Some(HRESULT(0x800704C7))"
    ));
    assert!(device_ble_name_apply_error_allows_deferred_confirmation(
        "BLE device settings write async canceled"
    ));
    assert!(device_ble_name_apply_error_allows_deferred_confirmation(
        "BLE device settings write timed out after 10000 ms"
    ));
    assert!(!device_ble_name_apply_error_allows_deferred_confirmation(
        "firmware rejected device settings command: invalid ble_name"
    ));
}

#[test]
fn ble_name_readback_unavailable_still_refreshes_windows_when_name_changed() {
    let source = normalized_commands_source();
    let start = source
        .find("Err(readback_error) =>")
        .expect("BLE name readback error branch should exist");
    let end = source[start..]
        .find("Err(apply_error) =>")
        .map(|offset| start + offset)
        .expect("BLE name apply error branch should follow readback branch");
    let body = &source[start..end];

    assert!(body.contains("set_configured_bluetooth_target_name"));
    assert!(body.contains("if ble_name_changed"));
    assert!(body.contains("coord.refresh_embedded_ble_listener"));
    assert!(
        body.contains("refresh_windows_ble_cache_after_device_ble_name_change"),
        "BLE rename must still refresh Windows Bluetooth cache when apply readback is unavailable but the requested name changed"
    );
}

#[test]
fn ble_name_apply_confirmation_polls_firmware_instead_of_fixed_blind_wait() {
    let source = normalized_commands_source();
    let helper_start = source
        .find("async fn read_device_ble_name_apply_confirmation")
        .expect("BLE name apply confirmation helper should exist");
    let helper_end = source[helper_start..]
        .find("fn device_ble_name_apply_error_allows_deferred_confirmation")
        .map(|offset| helper_start + offset)
        .expect("BLE name apply confirmation helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    assert!(helper.contains("DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_INITIAL_DELAY"));
    assert!(helper.contains("DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_RETRY_DELAY"));
    assert!(helper.contains("DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_READBACK_ATTEMPTS"));
    assert!(
        helper.contains("snapshot.ble_name == expected_ble_name && !snapshot.ble_name_pending_restart"),
        "rename recovery must require a firmware-confirmed name with pending=0 before touching Windows pairing cache"
    );
    let obsolete_fixed_wait = [
        "tokio::time::sleep(",
        "DEVICE_SETTINGS_BLE_NAME_APPLY_SETTLE_DELAY)",
    ]
    .concat();
    assert!(
        source.contains("read_device_ble_name_apply_confirmation(&request.ble_name).await")
            && !source.contains(&obsolete_fixed_wait),
        "the settings write path must use confirmation polling instead of returning to a fixed blind settle delay"
    );
}

#[test]
fn usb_confirmed_ble_name_apply_overlaps_cache_recovery_but_keeps_final_readback() {
    let source = normalized_commands_source();
    let start = source
        .find("pub async fn set_device_settings")
        .expect("set_device_settings should exist");
    let end = source[start..]
        .find("fn device_settings_snapshot_from_status")
        .map(|offset| start + offset)
        .expect("set_device_settings boundary should exist");
    let body = &source[start..end];
    assert!(
        body.contains("DeviceSettingsCommandTransport::UsbSerial")
            && body.contains("USB apply ACK confirms firmware advertising handoff")
            && body.contains("refresh_windows_ble_cache_after_device_ble_name_change"),
        "only a synchronous USB apply acknowledgement may overlap confirmed BLE-name cache recovery"
    );
    assert!(
        body.contains("Ok(_) => match read_device_ble_name_apply_confirmation"),
        "non-USB BLE-name apply paths must retain firmware status confirmation before recovery"
    );
    assert!(
        body.contains("read_device_settings_snapshot_after_write"),
        "the settings command must still complete a final firmware readback before the UI reports success"
    );
}

#[test]
fn device_ble_name_change_path_refreshes_windows_cache_after_apply() {
    let source = normalized_commands_source();
    let start = source
        .find("pub async fn set_device_settings")
        .expect("set_device_settings should exist");
    let end = source[start..]
        .find("fn device_settings_snapshot_from_status")
        .map(|offset| start + offset)
        .expect("set_device_settings boundary should exist");
    let body = &source[start..end];
    assert!(
        body.contains("refresh_windows_ble_cache_after_device_ble_name_change"),
        "different BLE names must refresh Windows Bluetooth cache so the Windows display name matches Type"
    );

    let helper_start = source
        .find("fn apply_device_ble_name_windows_refresh_blocking")
        .expect("BLE name Windows refresh helper should exist");
    let helper_end = source[helper_start..]
        .find("fn device_ble_name_windows_refresh_detail")
        .map(|offset| helper_start + offset)
        .expect("BLE name Windows refresh helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    assert!(helper.contains("send_recording_control_silent_recovery"));
    assert!(!helper.contains("send_recording_control_recovery("));
    assert!(helper.contains("unpair_listener_devices_for_names"));
    assert!(
        helper.contains("unpair_listener_devices_for_known_addresses")
            && helper.contains("if observed_recovery_addresses.is_empty()"),
        "a fresh, verified recovery advertisement address must avoid a redundant full PnP/cache discovery before silent PairAsync"
    );
    assert!(
        helper.contains("BleDeviceUnpairStatus::NeedsUserAction")
            && helper.contains("BleDeviceUnpairStatus::NotFound"),
        "fast exact-address cleanup must retain the full Windows cache cleanup fallback when it cannot prove the stale pairing was handled"
    );
    assert!(helper.contains("embedded_ble_windows_pairing_result"));
    assert!(helper.contains("EmbeddedBleWindowsPairingPromptPolicy::SuppressUserPrompt"));
    assert!(
        helper.contains("observed_recovery_addresses.as_slice()")
            && source.contains(
                "prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses"
            ),
        "rename recovery must carry the first confirmed recovery advertisement address into the silent PairAsync path instead of rescanning Windows BLE"
    );
    assert!(
        helper.contains("let advertised_address =\n            wait_for_device_ble_name_recovery_pairing_ready")
            && helper.contains("advertised_address.or(verified_handoff_address)"),
        "rename recovery must prefer a freshly observed applied-name advertisement before silent PairAsync, retaining the firmware-confirmed address only as a bounded fallback"
    );
    assert!(
        helper.contains("firmware_name_confirmed")
            && helper.contains(".then(")
            && helper.contains("verified_bluetooth_target_rename_handoff_address"),
        "when Windows misses an immediate post-rename advertisement, Type may use only the address handed off before this exact firmware-confirmed name transition"
    );
}

#[test]
fn firmware_confirmed_rename_handoff_falls_back_only_after_applied_name_advertisement_scan() {
    let source = normalized_commands_source();
    let helper_start = source
        .find("fn apply_device_ble_name_windows_refresh_blocking")
        .expect("BLE name Windows refresh helper should exist");
    let helper_end = source[helper_start..]
        .find("fn device_ble_name_windows_refresh_detail")
        .map(|offset| helper_start + offset)
        .expect("BLE name Windows refresh helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    let verified_address = helper
        .find("verified_bluetooth_target_rename_handoff_address")
        .expect(
            "only the verified rename handoff address may be used before new advertising is seen",
        );
    let early_unpair = helper
        .find("early_unpair_result")
        .expect("rename should start exact-address cleanup while waiting for recovery advertising");
    let advertisement_wait = helper
        .find("wait_for_device_ble_name_recovery_pairing_ready(&expected_ble_name)")
        .expect("rename recovery must confirm the applied Listener advertisement before PairAsync");
    let handoff_fallback = helper
        .find("advertised_address.or(verified_handoff_address)")
        .expect(
            "a verified handoff address should remain available only after the advertisement scan",
        );
    assert!(verified_address < early_unpair && early_unpair < advertisement_wait);
    assert!(advertisement_wait < handoff_fallback);
    assert!(helper.contains("firmware_name_confirmed"));
    assert!(helper.contains("if advertised_address.is_none()"));
    assert!(
        helper.contains("observed_recovery_addresses.as_slice()"),
        "the existing silent PairAsync path must receive the fresh advertisement address or the bounded firmware-confirmed fallback"
    );
    assert!(
        helper.contains("early recovery-address BLE cache cleanup status"),
        "failed exact-address cleanup must retain the complete Windows cache cleanup fallback"
    );
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "renames Listener hardware and refreshes Windows Bluetooth cache"]
fn device_ble_name_windows_refresh_hardware_smoke() {
    crate::init_file_logger();
    let old_name = std::env::var("LISTENER_BLE_NAME_RENAME_OLD")
        .expect("set LISTENER_BLE_NAME_RENAME_OLD to the currently advertised BLE name");
    let target_name = std::env::var("LISTENER_BLE_NAME_RENAME_TARGET")
        .expect("set LISTENER_BLE_NAME_RENAME_TARGET to the desired BLE name");

    crate::embedded_ble::set_configured_bluetooth_target_name(&old_name);
    crate::embedded_ble::send_device_settings_command(
        &format!("DEVICE:SET ble_name={target_name}"),
        std::time::Duration::from_secs(4),
    )
    .expect("BLE name write should be acknowledged by firmware");
    apply_pending_ble_name_confirmed(&target_name, std::time::Duration::from_secs(4))
        .expect("BLE name apply should refresh advertising");
    std::thread::sleep(std::time::Duration::from_secs(2));

    crate::embedded_ble::set_configured_bluetooth_target_name(&target_name);
    let outcome = apply_device_ble_name_windows_refresh_blocking(
        target_name.clone(),
        vec![old_name, target_name.clone()],
        true,
    );
    assert_eq!(
        outcome.pairing_prompt_result.failed_devices, 0,
        "Windows pairing failed: {:?}",
        outcome.pairing_prompt_result.details
    );
    assert!(
        device_ble_name_windows_refresh_confirmed(&outcome),
        "Windows cache refresh should complete pairing: {:?}",
        outcome
    );

    let settings =
        crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(4))
            .expect("renamed Listener device settings should be readable");
    assert_eq!(settings.ble_name, target_name);
    assert!(
        !settings.ble_name_pending_restart,
        "firmware should report the refreshed BLE name as applied"
    );
}

#[cfg(target_os = "windows")]
fn apply_ble_name_and_refresh_windows_for_hardware_smoke(
    current_name: &str,
    target_name: &str,
) -> Result<(), String> {
    crate::embedded_ble::set_configured_bluetooth_target_name(current_name);
    crate::embedded_ble::send_device_settings_command(
        &format!("DEVICE:SET ble_name={target_name}"),
        std::time::Duration::from_secs(4),
    )
    .map_err(|err| format!("BLE name write failed {current_name}->{target_name}: {err}"))?;
    apply_pending_ble_name_confirmed(target_name, std::time::Duration::from_secs(4))
        .map_err(|err| format!("BLE name apply failed {current_name}->{target_name}: {err}"))?;
    std::thread::sleep(std::time::Duration::from_secs(2));

    crate::embedded_ble::set_configured_bluetooth_target_name(target_name);
    let outcome = apply_device_ble_name_windows_refresh_blocking(
        target_name.to_string(),
        vec![current_name.to_string(), target_name.to_string()],
        true,
    );
    if !device_ble_name_windows_refresh_confirmed(&outcome) {
        return Err(format!(
            "Windows cache refresh failed {current_name}->{target_name}: {:?}",
            outcome
        ));
    }

    let settings =
        crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
            .map_err(|err| {
                format!("renamed Listener settings unreadable after {target_name}: {err}")
            })?;
    if settings.ble_name != target_name || settings.ble_name_pending_restart {
        return Err(format!(
            "firmware readback mismatch after rename: expected={target_name} actual={} pending={}",
            settings.ble_name, settings.ble_name_pending_restart
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "round-trips Listener BLE name and Windows Bluetooth cache on hardware"]
fn device_ble_name_windows_refresh_roundtrip_hardware_smoke() {
    crate::init_file_logger();
    let old_name = std::env::var("LISTENER_BLE_NAME_RENAME_OLD").unwrap_or_else(|_| {
        crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
            .expect("device settings should be readable before BLE name roundtrip")
            .ble_name
    });
    let target_name = std::env::var("LISTENER_BLE_NAME_RENAME_TARGET").unwrap_or_else(|_| {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_secs()
            % 100_000;
        format!("Blt{suffix:05}")
    });
    assert_ne!(
        old_name, target_name,
        "roundtrip target name must differ from the current BLE name"
    );

    let forward = std::panic::catch_unwind(|| {
        apply_ble_name_and_refresh_windows_for_hardware_smoke(&old_name, &target_name)
            .expect("forward BLE name refresh should pass")
    });

    let restore_from =
        crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
            .map(|status| status.ble_name)
            .unwrap_or_else(|_| target_name.clone());
    let restore = std::panic::catch_unwind(|| {
        if restore_from != old_name {
            apply_ble_name_and_refresh_windows_for_hardware_smoke(&restore_from, &old_name)
                .expect("restore BLE name refresh should pass")
        }
    });

    if let Err(err) = restore {
        std::panic::resume_unwind(err);
    }
    if let Err(err) = forward {
        std::panic::resume_unwind(err);
    }
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "uses Listener hardware to confirm unchanged BLE name does not re-pair"]
fn device_ble_name_same_name_no_repair_hardware_smoke() {
    crate::init_file_logger();
    let status =
        crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
            .expect("device settings should be readable before same-name smoke");
    let snapshot = device_settings_snapshot_from_status(status.clone());
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: status.status_led_brightness_percent,
        key_led_brightness_percent: status.key_led_brightness_percent,
        knob_led_brightness_percent: status.knob_led_brightness_percent,
        edge_led_brightness_percent: status.edge_led_brightness_percent,
        plugged_low_power_idle_minutes: status.plugged_low_power_idle_minutes,
        battery_low_power_idle_minutes: status.battery_low_power_idle_minutes,
        plugged_low_power_enabled: status.plugged_low_power_enabled,
        voice_auto_start_enabled: status.voice_auto_start_enabled,
        voice_auto_stop_enabled: status.voice_auto_stop_enabled,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: status.battery_auto_shutdown_minutes,
        ble_name: status.ble_name.clone(),
    };
    let ble_name_changed =
        device_ble_name_changed_for_request(&request, Some(&snapshot), Some(&status.ble_name));
    assert!(
        !ble_name_changed,
        "same BLE name should not request Windows re-pair"
    );
    assert!(
        !device_ble_name_apply_needed(&request, Some(&snapshot), ble_name_changed),
        "same applied BLE name should not run APPLY_BLE_NAME"
    );

    let commands = device_settings_update_commands(
        &request,
        snapshot.led_zone_brightness_supported,
        request.plugged_low_power_enabled && request.plugged_low_power_idle_minutes > 0,
        ble_name_changed,
        Some(&snapshot),
    )
    .expect("same-name commands should fit BLE control");
    assert!(commands
        .iter()
        .all(|command| !command.starts_with("DEVICE:SET ble_name=")));

    for command in commands {
        crate::embedded_ble::send_device_settings_command(
            &command,
            std::time::Duration::from_secs(4),
        )
        .unwrap_or_else(|err| panic!("same-name command failed command={command}: {err}"));
    }

    let after = crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
        .expect("device settings should remain readable after same-name smoke");
    assert_eq!(after.ble_name, status.ble_name);
    assert!(
        !after.ble_name_pending_restart,
        "same-name settings update must not leave BLE name pending"
    );
}

#[test]
fn device_ble_name_change_serial_batch_only_writes_name() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "OfficeType01".to_string(),
    };

    let commands = device_settings_update_commands(
        &request,
        false,
        request.plugged_low_power_enabled,
        true,
        None,
    )
    .expect("commands");

    assert!(commands
        .iter()
        .any(|command| command == "DEVICE:SET ble_name=OfficeType01"));
    assert!(commands.iter().all(|command| {
        !command.to_ascii_uppercase().contains("RECOVERY")
            && !command.to_ascii_uppercase().contains("PAIR")
            && !command.to_ascii_uppercase().contains("UNPAIR")
    }));
}

#[test]
fn device_settings_update_commands_fit_ble_audio_control() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 100,
        key_led_brightness_percent: 100,
        knob_led_brightness_percent: 100,
        edge_led_brightness_percent: 100,
        plugged_low_power_idle_minutes: 1440,
        battery_low_power_idle_minutes: 1440,
        plugged_low_power_enabled: false,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 1440,
        ble_name: "listener-12345678901234567890123".to_string(),
    };
    let commands = device_settings_update_commands(
        &request,
        true,
        request.plugged_low_power_enabled,
        true,
        None,
    )
    .expect("commands");

    assert_eq!(commands.len(), 7);
    assert!(
        commands.iter().all(|command| {
            !command.contains("plugged_brightness") && !command.contains("battery_brightness")
        }),
        "Type settings writes must not reintroduce a global brightness cap"
    );
    assert!(commands
        .iter()
        .any(|command| command.contains("plugged_low_power_enabled=0")));
    assert!(commands
        .iter()
        .any(|command| command.contains("plugged_auto_shutdown_minutes=off")));
    assert!(commands
        .iter()
        .any(|command| command.contains("voice_auto_start=0")));
    assert!(commands
        .iter()
        .any(|command| command.contains("voice_auto_stop=0")));
    assert!(commands
        .iter()
        .all(|command| { command.as_bytes().len() + 1 <= DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES }));

    let legacy_commands = device_settings_update_commands(
        &request,
        false,
        request.plugged_low_power_enabled,
        true,
        None,
    )
    .expect("commands");
    assert_eq!(legacy_commands.len(), 6);
    assert!(
        legacy_commands.iter().all(|command| {
            !command.contains("plugged_brightness") && !command.contains("battery_brightness")
        }),
        "legacy settings writes must not reintroduce a global brightness cap"
    );
}

#[test]
fn device_settings_update_commands_use_compact_keys_when_firmware_supports_them() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 39,
        key_led_brightness_percent: 52,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 68,
        plugged_low_power_idle_minutes: 10,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 10,
        ble_name: "Billy".to_string(),
    };
    let previous = DeviceSettingsSnapshot {
        schema: DEVICE_SETTINGS_SCHEMA,
        connected: true,
        write_supported: true,
        source: "firmware",
        status_led_brightness_percent: 50,
        key_led_brightness_percent: 80,
        knob_led_brightness_percent: 100,
        edge_led_brightness_percent: 100,
        led_zone_brightness_supported: true,
        compact_set_supported: true,
        low_power_idle_minutes: 3,
        plugged_low_power_idle_minutes: 3,
        battery_low_power_idle_minutes: 1,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_ms: 0,
        battery_auto_shutdown_ms: 10 * 60_000,
        knob_rotation_action: "systemVolume".to_string(),
        ble_name: "Billy".to_string(),
        ble_name_pending_restart: false,
        active_power_source: "plugged",
        battery_percent: None,
        detail: None,
        last_updated_at: None,
    };

    let commands = device_settings_update_commands(
        &request,
        true,
        request.plugged_low_power_enabled,
        false,
        Some(&previous),
    )
    .expect("commands");

    assert_eq!(
        commands,
        vec!["DEVICE:SET ls=39 lk=52 l11=60 le=68 plm=10 blm=3"]
    );
    assert!(commands
        .iter()
        .all(|command| command.as_bytes().len() + 1 <= DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES));
}

#[test]
fn settings_save_packets_led_zone_brightness_and_low_power_changes() {
    let previous = UserPreferences::default();
    let mut next = previous.clone();
    next.device_status_led_brightness_percent = 73;
    next.device_key_led_brightness_percent = 74;
    next.device_knob_led_brightness_percent = 75;
    next.device_edge_led_brightness_percent = 76;
    next.device_plugged_low_power_idle_minutes = 12;
    next.device_battery_low_power_idle_minutes = 7;
    next.device_low_power_idle_minutes = next.device_battery_low_power_idle_minutes;

    assert!(
        device_firmware_settings_changed(&previous, &next),
        "Type settings save must treat four-zone LED brightness as firmware settings"
    );

    let commands = device_setting_packets_for_changes(&previous, &next)
        .into_iter()
        .map(|packet| packet.command)
        .collect::<Vec<_>>();
    assert!(
        commands
            .iter()
            .any(|command| command == "DEVICE:SET led_status=73 led_key=74"),
        "Type settings save must sync status/key brightness through DEVICE:SET"
    );
    assert!(
        commands
            .iter()
            .any(|command| command == "DEVICE:SET led_ec11=75 led_edge=76"),
        "Type settings save must sync EC11/edge brightness through DEVICE:SET"
    );
    assert!(
        commands
            .iter()
            .any(|command| command == "DEVICE:SET plugged_low_power_idle_minutes=12"),
        "Type settings save must sync plugged low-power minutes exactly"
    );
    assert!(
        commands
            .iter()
            .any(|command| command == "DEVICE:SET battery_low_power_idle_minutes=7"),
        "Type settings save must sync battery low-power minutes exactly"
    );
    assert!(
        commands.iter().all(|command| {
            !command.contains("plugged_brightness")
                && !command.contains("battery_brightness")
                && command != "DEVICE:SET low_power_idle_minutes=7"
        }),
        "Type settings save must not reintroduce global brightness or merged low-power writes"
    );
}

#[test]
fn settings_save_syncs_ec11_fast_recording_only_for_dictation() {
    let previous = UserPreferences::default();
    let mut next = previous.clone();
    next.device_custom_keys.knob.action = DeviceCustomKeyAction::Disabled;

    assert!(
        device_firmware_settings_changed(&previous, &next),
        "changing EC11 away from Dictation must update firmware's fast recording gate"
    );
    let commands = device_setting_packets_for_changes(&previous, &next)
        .into_iter()
        .map(|packet| packet.command)
        .collect::<Vec<_>>();
    assert!(
        commands
            .iter()
            .any(|command| command == "DEVICE:SET e11r=0"),
        "non-dictation EC11 mappings must disable immediate firmware recording"
    );
}

#[test]
fn device_settings_readback_mismatch_fails_after_write() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 73,
        key_led_brightness_percent: 74,
        knob_led_brightness_percent: 75,
        edge_led_brightness_percent: 76,
        plugged_low_power_idle_minutes: 12,
        battery_low_power_idle_minutes: 12,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener-dev".to_string(),
    };
    let snapshot = DeviceSettingsSnapshot {
        schema: DEVICE_SETTINGS_SCHEMA,
        connected: true,
        write_supported: true,
        source: "firmware",
        status_led_brightness_percent: 72,
        key_led_brightness_percent: 74,
        knob_led_brightness_percent: 75,
        edge_led_brightness_percent: 76,
        led_zone_brightness_supported: true,
        compact_set_supported: false,
        low_power_idle_minutes: 13,
        plugged_low_power_idle_minutes: 13,
        battery_low_power_idle_minutes: 12,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_ms: 0,
        battery_auto_shutdown_ms: 30 * 60_000,
        knob_rotation_action: "systemVolume".to_string(),
        ble_name: "listener-dev".to_string(),
        ble_name_pending_restart: false,
        active_power_source: "plugged",
        battery_percent: None,
        detail: None,
        last_updated_at: None,
    };

    let err = ensure_device_settings_readback_matches_request(&snapshot, &request, true, true)
        .expect_err("mismatched firmware readback must fail the Type settings write");
    assert!(err.contains("led_status expected=73 actual=72"));
    assert!(err.contains("plugged_low_power_idle_minutes expected=12 actual=13"));
}

#[test]
fn device_settings_readback_mismatch_fails_for_each_written_field() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 41,
        key_led_brightness_percent: 57,
        knob_led_brightness_percent: 63,
        edge_led_brightness_percent: 79,
        plugged_low_power_idle_minutes: 23,
        battery_low_power_idle_minutes: 37,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 45,
        ble_name: "listener-dev".to_string(),
    };
    let matching = DeviceSettingsSnapshot {
        schema: DEVICE_SETTINGS_SCHEMA,
        connected: true,
        write_supported: true,
        source: "firmware",
        status_led_brightness_percent: request.status_led_brightness_percent,
        key_led_brightness_percent: request.key_led_brightness_percent,
        knob_led_brightness_percent: request.knob_led_brightness_percent,
        edge_led_brightness_percent: request.edge_led_brightness_percent,
        led_zone_brightness_supported: true,
        compact_set_supported: false,
        low_power_idle_minutes: request.plugged_low_power_idle_minutes,
        plugged_low_power_idle_minutes: request.plugged_low_power_idle_minutes,
        battery_low_power_idle_minutes: request.battery_low_power_idle_minutes,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_ms: 0,
        battery_auto_shutdown_ms: request.battery_auto_shutdown_minutes * 60_000,
        knob_rotation_action: "systemVolume".to_string(),
        ble_name: request.ble_name.clone(),
        ble_name_pending_restart: false,
        active_power_source: "plugged",
        battery_percent: None,
        detail: None,
        last_updated_at: None,
    };

    let assert_mismatch = |snapshot: DeviceSettingsSnapshot, fragment: &str| {
        let err = ensure_device_settings_readback_matches_request(&snapshot, &request, true, true)
            .expect_err("any written field readback mismatch must fail the Type settings write");
        assert!(
            err.contains(fragment),
            "expected mismatch fragment {fragment:?} in {err:?}"
        );
    };

    let mut snapshot = matching.clone();
    snapshot.status_led_brightness_percent = 40;
    assert_mismatch(snapshot, "led_status expected=41 actual=40");

    let mut snapshot = matching.clone();
    snapshot.key_led_brightness_percent = 56;
    assert_mismatch(snapshot, "led_key expected=57 actual=56");

    let mut snapshot = matching.clone();
    snapshot.knob_led_brightness_percent = 62;
    assert_mismatch(snapshot, "led_ec11 expected=63 actual=62");

    let mut snapshot = matching.clone();
    snapshot.edge_led_brightness_percent = 78;
    assert_mismatch(snapshot, "led_edge expected=79 actual=78");

    for actual in [0, 1, 12, 24, 1440] {
        if actual == request.plugged_low_power_idle_minutes {
            continue;
        }
        let mut snapshot = matching.clone();
        snapshot.plugged_low_power_idle_minutes = actual;
        assert_mismatch(
            snapshot,
            &format!(
                "plugged_low_power_idle_minutes expected={} actual={actual}",
                request.plugged_low_power_idle_minutes
            ),
        );
    }

    for actual in [0, 1, 12, 38, 1440] {
        if actual == request.battery_low_power_idle_minutes {
            continue;
        }
        let mut snapshot = matching.clone();
        snapshot.battery_low_power_idle_minutes = actual;
        assert_mismatch(
            snapshot,
            &format!(
                "battery_low_power_idle_minutes expected={} actual={actual}",
                request.battery_low_power_idle_minutes
            ),
        );
    }

    let mut snapshot = matching.clone();
    snapshot.plugged_low_power_enabled = false;
    assert_mismatch(
        snapshot,
        "plugged_low_power_enabled expected=true actual=false",
    );

    let mut snapshot = matching.clone();
    snapshot.battery_auto_shutdown_ms = 46 * 60_000;
    assert_mismatch(
        snapshot,
        "battery_auto_shutdown_ms expected=2700000 actual=2760000",
    );

    let mut snapshot = matching.clone();
    snapshot.ble_name = "listener-alt".to_string();
    assert_mismatch(
        snapshot,
        "ble_name expected=\"listener-dev\" actual=\"listener-alt\"",
    );
}

#[test]
fn device_settings_plain_writes_require_confirmed_readback() {
    assert!(
        !device_settings_readback_unavailable_allowed_after_write(false, false),
        "brightness and low-power writes must not be reported as saved without firmware readback"
    );
    assert!(
        device_settings_readback_unavailable_allowed_after_write(true, false),
        "BLE rename can temporarily lose readback while Windows cache refresh follows the new name"
    );
    assert!(
        device_settings_readback_unavailable_allowed_after_write(false, true),
        "pending BLE-name apply can use the existing deferred confirmation path"
    );
    let source = normalized_commands_source();
    assert!(
        source.contains("无法确认写入是否生效"),
        "plain settings write failures must tell the UI that firmware readback did not confirm persistence"
    );
}

#[test]
fn device_settings_write_retries_stale_readback_before_failing() {
    let source = normalized_commands_source();
    let settings_start = source
        .find("pub async fn set_device_settings")
        .expect("set_device_settings should exist");
    let settings_end = source[settings_start..]
        .find("fn device_settings_snapshot_from_status")
        .map(|offset| settings_start + offset)
        .expect("set_device_settings boundary should exist");
    let settings_body = &source[settings_start..settings_end];

    assert!(
        settings_body.contains("read_device_settings_snapshot_after_write"),
        "Type settings writes must verify readback through the retry helper, not a single immediate USB read"
    );
    assert!(
        source.contains("DEVICE_SETTINGS_READBACK_VERIFY_ATTEMPTS")
            && source.contains("DEVICE_SETTINGS_READBACK_VERIFY_RETRY_DELAY"),
        "Type settings readback verification must keep bounded retry constants"
    );
    assert!(
        source.contains("[device-settings] write readback mismatch attempt=")
            && source.contains("[device-settings] write readback unavailable attempt="),
        "Type settings readback retries must log expected/actual mismatch or unavailable evidence"
    );
}

#[test]
fn device_settings_update_commands_skip_unchanged_ble_name() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 70,
        key_led_brightness_percent: 65,
        knob_led_brightness_percent: 60,
        edge_led_brightness_percent: 55,
        plugged_low_power_idle_minutes: 2,
        battery_low_power_idle_minutes: 3,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 30,
        ble_name: "listener-dev".to_string(),
    };

    let commands = device_settings_update_commands(
        &request,
        false,
        request.plugged_low_power_enabled,
        false,
        None,
    )
    .expect("commands");

    assert!(
        !commands
            .iter()
            .any(|command| command.starts_with("DEVICE:SET ble_name=")),
        "unchanged BLE name writes must not trigger re-pair recovery"
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.contains("plugged_brightness")
                || command.contains("battery_brightness")),
        "unchanged BLE name writes must stay scoped and must not touch global brightness"
    );
}

#[test]
fn device_settings_zero_low_power_disables_plugged_low_power_command() {
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: 100,
        key_led_brightness_percent: 100,
        knob_led_brightness_percent: 100,
        edge_led_brightness_percent: 100,
        plugged_low_power_idle_minutes: 0,
        battery_low_power_idle_minutes: 0,
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: 0,
        ble_name: "listener-dev".to_string(),
    };

    assert!(validate_device_settings_request(&request).is_ok());
    let plugged_low_power_enabled =
        request.plugged_low_power_enabled && request.plugged_low_power_idle_minutes > 0;
    let commands =
        device_settings_update_commands(&request, false, plugged_low_power_enabled, true, None)
            .expect("commands");

    assert!(commands
        .iter()
        .any(|command| { command == "DEVICE:SET plugged_low_power_idle_minutes=0" }));
    assert!(commands
        .iter()
        .any(|command| { command == "DEVICE:SET battery_low_power_idle_minutes=0" }));
    assert!(commands
        .iter()
        .any(|command| command.contains("plugged_low_power_enabled=0")));
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "writes random Listener LED brightness and low-power settings through Type"]
fn device_settings_random_led_and_low_power_write_hardware_smoke() {
    crate::init_file_logger();
    let before =
        crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
            .expect("device settings should be readable before Type settings smoke");
    assert!(
        before.led_zone_brightness_supported,
        "firmware must report led_status/led_key support before Type writes LED brightness"
    );
    assert!(
        !before.ble_name_pending_restart,
        "hardware smoke expects no pending BLE-name apply before the settings write"
    );

    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after Unix epoch")
        .as_nanos();
    let pick_brightness = |salt: u128, avoid: u8| -> u8 {
        let mut value = 33 + (((seed / salt) % 53) as u8);
        if value == avoid {
            value = if value < 80 { value + 5 } else { value - 5 };
        }
        value
    };
    let pick_minutes = |salt: u128, avoid: u32| -> u32 {
        let mut value = 5 + (((seed / salt) % 19) as u32);
        if value == avoid {
            value = if value < 20 { value + 1 } else { value - 1 };
        }
        value
    };
    let request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: pick_brightness(3, before.status_led_brightness_percent),
        key_led_brightness_percent: pick_brightness(7, before.key_led_brightness_percent),
        knob_led_brightness_percent: before.knob_led_brightness_percent,
        edge_led_brightness_percent: before.edge_led_brightness_percent,
        plugged_low_power_idle_minutes: pick_minutes(11, before.plugged_low_power_idle_minutes),
        battery_low_power_idle_minutes: pick_minutes(17, before.battery_low_power_idle_minutes),
        plugged_low_power_enabled: true,
        voice_auto_start_enabled: true,
        voice_auto_stop_enabled: true,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: before.battery_auto_shutdown_minutes,
        ble_name: before.ble_name.clone(),
    };
    let plugged_low_power_enabled =
        request.plugged_low_power_enabled && request.plugged_low_power_idle_minutes > 0;
    let before_snapshot = device_settings_snapshot_from_status(before.clone());
    let commands = device_settings_update_commands(
        &request,
        before.led_zone_brightness_supported,
        plugged_low_power_enabled,
        false,
        Some(&before_snapshot),
    )
    .expect("Type device-settings commands should fit BLE audio control");
    println!(
        "type_simulated_device_settings_write led_status={} led_key={} plugged_low_power={} battery_low_power={} commands={:?}",
        request.status_led_brightness_percent,
        request.key_led_brightness_percent,
        request.plugged_low_power_idle_minutes,
        request.battery_low_power_idle_minutes,
        commands
    );

    let smoke_result = std::panic::catch_unwind(|| {
        for command in &commands {
            crate::embedded_ble::send_device_settings_command(
                command,
                std::time::Duration::from_secs(4),
            )
            .unwrap_or_else(|err| {
                panic!("Type device-settings command failed command={command}: {err}")
            });
        }

        let after =
            crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
                .expect("device settings should be readable after Type settings smoke");
        println!(
            "type_device_settings_readback led_status={} led_key={} led_ec11={} led_edge={} plugged_low_power={} battery_low_power={} raw={}",
            after.status_led_brightness_percent,
            after.key_led_brightness_percent,
            after.knob_led_brightness_percent,
            after.edge_led_brightness_percent,
            after.plugged_low_power_idle_minutes,
            after.battery_low_power_idle_minutes,
            after.raw_line
        );
        let snapshot = device_settings_snapshot_from_status(after);
        ensure_device_settings_readback_matches_request(
            &snapshot,
            &request,
            before.led_zone_brightness_supported,
            plugged_low_power_enabled,
        )
        .expect("firmware readback must match the Type random write exactly");
        assert_eq!(
            snapshot.plugged_low_power_idle_minutes, request.plugged_low_power_idle_minutes,
            "Type must show exactly the plugged low-power minutes that it wrote"
        );
        assert_eq!(
            snapshot.battery_low_power_idle_minutes, request.battery_low_power_idle_minutes,
            "Type must show exactly the battery low-power minutes that it wrote"
        );
        let brightness_log = crate::embedded_ble::read_status_led_brightness_status(
            std::time::Duration::from_secs(8),
        )
        .expect("status LED brightness detail should be readable after Type random write");
        println!(
            "type_device_settings_led_brightness_log led_status={} led_key={} raw={}",
            request.status_led_brightness_percent,
            request.key_led_brightness_percent,
            brightness_log
        );
        assert!(
            brightness_log.contains(&format!(
                "status_zone_brightness_percent={}",
                request.status_led_brightness_percent
            )),
            "firmware LED status brightness log must reflect Type's random status cap: {brightness_log}"
        );
        assert!(
            brightness_log.contains(&format!(
                "key_zone_brightness_percent={}",
                request.key_led_brightness_percent
            )),
            "firmware LED status brightness log must reflect Type's random key cap: {brightness_log}"
        );
    });

    let restore_request = DeviceSettingsUpdateRequest {
        status_led_brightness_percent: before.status_led_brightness_percent,
        key_led_brightness_percent: before.key_led_brightness_percent,
        knob_led_brightness_percent: before.knob_led_brightness_percent,
        edge_led_brightness_percent: before.edge_led_brightness_percent,
        plugged_low_power_idle_minutes: before.plugged_low_power_idle_minutes,
        battery_low_power_idle_minutes: before.battery_low_power_idle_minutes,
        plugged_low_power_enabled: before.plugged_low_power_enabled,
        voice_auto_start_enabled: before.voice_auto_start_enabled,
        voice_auto_stop_enabled: before.voice_auto_stop_enabled,
        plugged_auto_shutdown_minutes: 0,
        battery_auto_shutdown_minutes: before.battery_auto_shutdown_minutes,
        ble_name: before.ble_name.clone(),
    };
    let restore_plugged_low_power_enabled = restore_request.plugged_low_power_enabled
        && restore_request.plugged_low_power_idle_minutes > 0;
    let restore_commands = device_settings_update_commands(
        &restore_request,
        before.led_zone_brightness_supported,
        restore_plugged_low_power_enabled,
        false,
        None,
    )
    .expect("restore commands should fit BLE audio control");
    for command in &restore_commands {
        crate::embedded_ble::send_device_settings_command(
            command,
            std::time::Duration::from_secs(4),
        )
        .unwrap_or_else(|err| {
            panic!("failed to restore Type device settings command={command}: {err}")
        });
    }
    let restored =
        crate::embedded_ble::read_device_settings_status(std::time::Duration::from_secs(8))
            .expect("device settings should be readable after restore");
    println!(
        "type_device_settings_restored led_status={} led_key={} plugged_low_power={} battery_low_power={} raw={}",
        restored.status_led_brightness_percent,
        restored.key_led_brightness_percent,
        restored.plugged_low_power_idle_minutes,
        restored.battery_low_power_idle_minutes,
        restored.raw_line
    );
    assert_eq!(
        restored.status_led_brightness_percent,
        before.status_led_brightness_percent
    );
    assert_eq!(
        restored.key_led_brightness_percent,
        before.key_led_brightness_percent
    );
    assert_eq!(
        restored.plugged_low_power_idle_minutes,
        before.plugged_low_power_idle_minutes
    );
    assert_eq!(
        restored.battery_low_power_idle_minutes,
        before.battery_low_power_idle_minutes
    );
    if let Err(err) = smoke_result {
        std::panic::resume_unwind(err);
    }
}

#[test]
fn repair_failure_maps_stale_gatt_to_repair_user_action() {
    let stale = crate::embedded_ble::classify_ble_failure(
        "Unknown GATT service from stale cached service table after customer repair",
    );
    assert_eq!(
        stale.kind,
        crate::embedded_ble::BleFailureKind::StaleGattService
    );
    assert!(stale.automatic_recovery);

    let (user_action_required, open_bluetooth_settings) =
        embedded_ble_repair_failure_action(&stale);
    assert!(user_action_required);
    assert!(open_bluetooth_settings);
}

#[test]
fn repair_failure_keeps_transient_disconnect_automatic() {
    let transient = crate::embedded_ble::classify_ble_failure(
        "BLE device disconnected while waiting for reconnect",
    );
    assert_eq!(
        transient.kind,
        crate::embedded_ble::BleFailureKind::PairedButDisconnected
    );
    assert!(transient.automatic_recovery);

    let (user_action_required, open_bluetooth_settings) =
        embedded_ble_repair_failure_action(&transient);
    assert!(!user_action_required);
    assert!(!open_bluetooth_settings);
}

#[test]
fn repair_failure_escalates_cccd_timeout_to_repair_user_action() {
    let cccd = crate::embedded_ble::classify_ble_failure(
        "BLE CCCD write timed out after 8000 ms after customer repair",
    );
    assert_eq!(
        cccd.kind,
        crate::embedded_ble::BleFailureKind::CccdProtocolError
    );
    assert!(cccd.automatic_recovery);

    let (user_action_required, open_bluetooth_settings) = embedded_ble_repair_failure_action(&cccd);
    assert!(user_action_required);
    assert!(open_bluetooth_settings);
}

#[test]
fn one_click_recovery_attempts_unpair_only_for_stale_pairing_failures() {
    let cccd = crate::embedded_ble::classify_ble_failure(
        "BLE CCCD write timed out after 8000 ms after customer repair",
    );
    let stale = crate::embedded_ble::classify_ble_failure(
        "Unknown GATT service from stale cached service table after customer repair",
    );
    let missing_pairing =
        crate::embedded_ble::classify_ble_failure("No paired BLE device for Listener");
    let transient = crate::embedded_ble::classify_ble_failure(
        "BLE device disconnected while waiting for reconnect",
    );
    let asleep = crate::embedded_ble::classify_ble_failure(
        "Listener BLE device asleep; press KEY4 wake key",
    );

    assert!(should_attempt_embedded_ble_auto_unpair(&cccd));
    assert!(should_attempt_embedded_ble_auto_unpair(&stale));
    assert!(should_attempt_embedded_ble_auto_unpair(&missing_pairing));
    assert!(!should_attempt_embedded_ble_auto_unpair(&transient));
    assert!(!should_attempt_embedded_ble_auto_unpair(&asleep));
    assert_eq!(
        embedded_ble_recovery_action_for_failure(&transient),
        EmbeddedBleRecoveryAction::WaitForAutomaticRecovery
    );
    assert_eq!(
        embedded_ble_recovery_action_for_failure(&cccd),
        EmbeddedBleRecoveryAction::RePairRequired
    );
}

#[test]
fn ble_name_refresh_and_one_click_use_type_controlled_pairasync_recovery() {
    let source = normalized_commands_source();
    let helper_start = source
        .find("fn embedded_ble_windows_pairing_result")
        .expect("Windows pairing helper should exist");
    let helper_end = source[helper_start..]
        .find("async fn embedded_ble_runtime_and_firmware")
        .map(|offset| helper_start + offset)
        .expect("Windows pairing helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    assert!(helper.contains("EmbeddedBleWindowsPairingPromptPolicy"));
    assert!(helper.contains("pre_pair_stale_cleanup"));
    assert!(helper.contains(
        "prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup"
    ));
    assert!(helper.contains("prompt_listener_pairing_after_type_recovery_without_user_prompt"));
    assert!(helper.contains("prompt_listener_pairing_after_type_recovery"));
    assert!(helper.contains("prompt_listener_pairing_for_recovery"));
    assert!(helper.contains("PairAsync"));

    let settings_start = source
        .find("pub async fn set_device_settings")
        .expect("device settings command should exist");
    let settings_end = source[settings_start..]
        .find("fn device_settings_snapshot_from_status")
        .map(|offset| settings_start + offset)
        .expect("device settings command boundary should exist");
    let settings_body = &source[settings_start..settings_end];
    assert!(
        settings_body.contains("refresh_windows_ble_cache_after_device_ble_name_change"),
        "BLE rename must refresh this PC's Windows Bluetooth cache so display name and Type target stay aligned"
    );

    let rename_helper_start = source
        .find("fn apply_device_ble_name_windows_refresh_blocking")
        .expect("BLE name Windows refresh helper should exist");
    let rename_helper_end = source[rename_helper_start..]
        .find("fn device_ble_name_windows_refresh_detail")
        .map(|offset| rename_helper_start + offset)
        .expect("BLE name Windows refresh helper boundary should exist");
    let rename_helper = &source[rename_helper_start..rename_helper_end];
    assert!(rename_helper.contains("send_recording_control_silent_recovery"));
    assert!(rename_helper.contains("unpair_listener_devices_for_names"));
    assert!(rename_helper.contains("EmbeddedBleWindowsPairingPromptPolicy::SuppressUserPrompt"));
    assert!(
        rename_helper.contains(
            "EmbeddedBleWindowsPairingPromptPolicy::SuppressUserPrompt,\n        false,\n        observed_recovery_addresses.as_slice(),"
        ),
        "BLE rename already cleaned Windows cache for old/new names and must not repeat Type pre-pair stale cleanup before PairAsync"
    );
    let cleanup_start = rename_helper
        .find("unpair_listener_devices_for_names")
        .expect("rename cleanup call should exist");
    let pairing_start = rename_helper[cleanup_start..]
        .find("embedded_ble_windows_pairing_result")
        .map(|offset| cleanup_start + offset)
        .expect("rename pairing call should exist after cleanup");
    assert!(
        !rename_helper[cleanup_start..pairing_start]
            .contains("std::thread::sleep(DEVICE_SETTINGS_BLE_NAME_PAIRING_SETTLE_DELAY)"),
        "a completed Windows cache cleanup must start observed-address PairAsync immediately instead of adding an unconditional settle delay"
    );
    assert!(
        rename_helper.contains("embedded_ble_windows_pairing_result"),
        "BLE rename refresh must run the bounded Windows pairing/cache refresh path after firmware applies a different name"
    );
    assert!(
        rename_helper.contains("wait_for_device_ble_name_recovery_pairing_ready(&expected_ble_name)")
            && rename_helper.contains("advertised_address.or(verified_handoff_address)"),
        "rename recovery must prefer a newly observed applied-name advertisement and retain the exact handoff only as its bounded fallback"
    );

    let one_click_start = source
        .match_indices("pub async fn recover_embedded_ble_device")
        .nth(1)
        .expect("one-click recovery device impl should exist")
        .0;
    let one_click_end = source[one_click_start..]
        .find("pub fn get_embedded_ble_runtime_status")
        .map(|offset| one_click_start + offset)
        .expect("one-click recovery command boundary should exist");
    let one_click = &source[one_click_start..one_click_end];
    assert!(
        one_click.contains("embedded_ble_windows_pairing_result"),
        "one-click recovery should use the same bounded Type PairAsync recovery path as BLE rename"
    );
    assert!(
        one_click.contains("EmbeddedBleWindowsPairingPromptPolicy::AllowUserPrompt"),
        "one-click recovery keeps the user-prompt-capable path for computer switching"
    );
    assert!(
        one_click
            .contains("EmbeddedBleWindowsPairingPromptPolicy::AllowUserPrompt,\n                        true,\n                        &[],"),
        "one-click/double-click Type recovery must retain pre-pair stale cleanup because it did not run the BLE rename cache-cleanup stage first"
    );
    assert!(
        one_click.contains("hold_embedded_ble_listener_for_native_pairing_handoff"),
        "one-click recovery should hold Type BLE while PairAsync/GATT recovery is in progress"
    );
    assert!(
        one_click.contains("will stop if another host completes pairing before this Type instance"),
        "one-click recovery logs must prove a different computer can win the pairing race without old Type looping"
    );
}

#[test]
fn ble_name_refresh_uses_silent_recovery_while_one_click_keeps_user_prompt() {
    let source = normalized_commands_source();
    let rename_helper_start = source
        .find("fn apply_device_ble_name_windows_refresh_blocking")
        .expect("BLE name Windows refresh helper should exist");
    let rename_helper_end = source[rename_helper_start..]
        .find("fn device_ble_name_windows_refresh_detail")
        .map(|offset| rename_helper_start + offset)
        .expect("BLE name Windows refresh helper boundary should exist");
    let rename_helper = &source[rename_helper_start..rename_helper_end];
    assert!(
        rename_helper.contains("send_recording_control_silent_recovery")
            && rename_helper.contains("EmbeddedBleWindowsPairingPromptPolicy::SuppressUserPrompt"),
        "BLE rename must use the silent firmware recovery command and suppress local user pairing prompts"
    );
    assert!(
        !rename_helper.contains("send_recording_control_recovery("),
        "BLE rename must not use the Swift-Pair-capable Type recovery command"
    );

    let one_click_start = source
        .match_indices("pub async fn recover_embedded_ble_device")
        .nth(1)
        .expect("one-click recovery device impl should exist")
        .0;
    let one_click_end = source[one_click_start..]
        .find("pub fn get_embedded_ble_runtime_status")
        .map(|offset| one_click_start + offset)
        .expect("one-click recovery command boundary should exist");
    let one_click = &source[one_click_start..one_click_end];
    assert!(
        one_click.contains("EmbeddedBleWindowsPairingPromptPolicy::AllowUserPrompt"),
        "one-click recovery keeps the user-prompt-capable PairAsync path for computer switching"
    );

    let coordinator = [
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/coordinator.rs")),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/coordinator/embedded_ble_runtime.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/coordinator/hotkey_device_runtime.rs"
        )),
    ]
    .join("\n");
    let lib = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"));
    assert!(
        coordinator.contains("send_recording_control_recovery")
            && !coordinator.contains("send_recording_control_silent_recovery")
            && lib.contains("send_recording_control_recovery")
            && !lib.contains("send_recording_control_silent_recovery"),
        "explicit/double-click Type recovery must keep the Swift-Pair-capable firmware recovery command"
    );
}

#[test]
fn one_click_recovery_escalates_runtime_cccd_history_after_repair_timeout() {
    let wake_recovery = crate::coordinator::EmbeddedBleWakeRecoverySnapshot {
        status: crate::coordinator::EmbeddedBleWakeRecoveryStatus::Reconnecting,
        user_guidance: "正在重连 Listener BLE 并恢复音频 notify".to_string(),
        recent_disconnect_reason: Some(
            "嵌入式 BLE 流式抓音中断: cause=BLE CCCD write timed out after 8000 ms".to_string(),
        ),
        reconnect_attempts: 6,
        notify_subscription_state: crate::coordinator::EmbeddedBleNotifySubscriptionState::Opening,
        ..Default::default()
    };

    assert!(runtime_suggests_embedded_ble_auto_unpair(
        "Listener BLE notify subscription did not recover within 15000 ms after foreground probe",
        None,
        &wake_recovery,
    ));

    let idle_recovery = crate::coordinator::EmbeddedBleWakeRecoverySnapshot {
        recent_disconnect_reason: Some(
            "Windows GATT disconnected reason=546 after low-power idle; transport_not_ready"
                .to_string(),
        ),
        reconnect_attempts: 6,
        notify_subscription_state: crate::coordinator::EmbeddedBleNotifySubscriptionState::Opening,
        usb_powered: Some(false),
        ..wake_recovery
    };
    assert!(!runtime_suggests_embedded_ble_auto_unpair(
        "Listener BLE notify subscription did not recover within 15000 ms after foreground probe",
        None,
        &idle_recovery,
    ));

    let powered_idle_recovery = crate::coordinator::EmbeddedBleWakeRecoverySnapshot {
        usb_powered: Some(true),
        ..idle_recovery
    };
    assert!(runtime_suggests_embedded_ble_auto_unpair(
        "Listener BLE notify subscription did not recover within 15000 ms after foreground probe",
        None,
        &powered_idle_recovery,
    ));
}

#[test]
fn one_click_recovery_message_hides_transport_details() {
    let failure = crate::embedded_ble::classify_ble_failure(
        "BLE CCCD write timed out after 8000 ms after customer repair",
    );
    let unpair = crate::embedded_ble::BleDeviceUnpairResult {
        status: crate::embedded_ble::BleDeviceUnpairStatus::Removed,
        attempted: true,
        matched_devices: 1,
        unpaired_devices: 1,
        already_unpaired_devices: 0,
        failed_devices: 0,
        needs_user_action: true,
        details: vec!["Removed stale Listener pairing".to_string()],
    };

    let message = embedded_ble_recovery_message(&failure, Some(&unpair), None);
    assert!(message.contains("旧 Listener 配对"));
    assert!(message.contains("自动恢复"));
    assert!(!message.to_ascii_lowercase().contains("cccd"));
    assert!(!message.to_ascii_lowercase().contains("gatt"));
    assert!(!message.to_ascii_lowercase().contains("pairasync"));
}

#[test]
fn repair_failure_keeps_idle_disconnect_automatic_but_repairable() {
    let idle = crate::embedded_ble::classify_ble_failure(
        "Windows GATT disconnected reason=546 after low-power idle; transport_not_ready",
    );
    assert_eq!(
        idle.kind,
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
    );
    assert!(idle.automatic_recovery);

    let (user_action_required, open_bluetooth_settings) = embedded_ble_repair_failure_action(&idle);
    assert!(!user_action_required);
    assert!(!open_bluetooth_settings);
}

#[test]
fn repair_failure_distinguishes_pairing_from_sleep() {
    let missing_pairing =
        crate::embedded_ble::classify_ble_failure("No paired BLE device for Listener");
    let asleep = crate::embedded_ble::classify_ble_failure(
        "Listener BLE device asleep; press KEY4 wake key",
    );

    assert_eq!(
        missing_pairing.kind,
        crate::embedded_ble::BleFailureKind::MissingPairing
    );
    assert_eq!(
        asleep.kind,
        crate::embedded_ble::BleFailureKind::DeviceAsleep
    );

    assert_eq!(
        embedded_ble_repair_failure_action(&missing_pairing),
        (true, true)
    );
    assert_eq!(embedded_ble_repair_failure_action(&asleep), (true, false));
}

#[test]
fn load_firmware_ota_package_reads_directory() {
    let root = std::env::temp_dir().join(format!("listener-ota-dir-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create temp package dir");
    std::fs::write(root.join("ota_manifest.json"), "{\"schema_version\":2}")
        .expect("write manifest");
    std::fs::write(root.join("firmware_ota.bin"), [1u8, 2, 3]).expect("write firmware");

    let payload = super::device::load_firmware_ota_package(root.to_string_lossy().to_string())
        .expect("load directory OTA package");
    assert_eq!(payload.manifest_text, "{\"schema_version\":2}");
    assert_eq!(payload.firmware_bytes, vec![1, 2, 3]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_firmware_ota_package_reads_zip() {
    let zip_path =
        std::env::temp_dir().join(format!("listener-ota-zip-test-{}.zip", std::process::id()));
    let _ = std::fs::remove_file(&zip_path);
    {
        let file = std::fs::File::create(&zip_path).expect("create temp zip");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("listener-ota/ota_manifest.json", options)
            .expect("start manifest");
        zip.write_all(b"{\"schema_version\":2}")
            .expect("write manifest");
        zip.start_file("listener-ota/firmware_ota.bin", options)
            .expect("start firmware");
        zip.write_all(&[4u8, 5, 6]).expect("write firmware");
        zip.finish().expect("finish zip");
    }

    let payload = super::device::load_firmware_ota_package(zip_path.to_string_lossy().to_string())
        .expect("load zip OTA package");
    assert_eq!(payload.manifest_text, "{\"schema_version\":2}");
    assert_eq!(payload.firmware_bytes, vec![4, 5, 6]);
    let _ = std::fs::remove_file(&zip_path);
}

#[test]
fn combined_firmware_release_zip_supports_ota_and_wired_factory() {
    let root = std::env::temp_dir().join(format!(
        "listener-combined-release-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let factory_dir = root.join("factory").join("listener-factory-test");
    let zip_path = root.with_extension("zip");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&zip_path);
    std::fs::create_dir_all(&factory_dir).expect("create nested factory dir");
    write_test_factory_package(&factory_dir, "v1.2.6");

    {
        let file = std::fs::File::create(&zip_path).expect("create temp combined zip");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("ota_manifest.json", options)
            .expect("start OTA manifest");
        zip.write_all(b"{\"schema_version\":2}")
            .expect("write OTA manifest");
        zip.start_file("firmware_ota.bin", options)
            .expect("start OTA firmware");
        zip.write_all(&[4u8, 5, 6]).expect("write OTA firmware");
        for file_name in [
            "manifest.json",
            "FLASHING.md",
            "bootloader.bin",
            "partition-table.bin",
            "voice-keyboard-firmware.bin",
        ] {
            let source = factory_dir.join(file_name);
            if file_name == "FLASHING.md" && !source.is_file() {
                std::fs::write(&source, "# Factory flashing\n").expect("write flashing doc");
            }
            zip.start_file(
                format!("factory/listener-factory-test/{file_name}"),
                options,
            )
            .expect("start factory file");
            zip.write_all(&std::fs::read(&source).expect("read factory file"))
                .expect("write factory file");
        }
        zip.finish().expect("finish combined zip");
    }

    let ota_payload =
        super::device::load_firmware_ota_package(zip_path.to_string_lossy().to_string())
            .expect("load OTA files from combined release zip");
    assert_eq!(ota_payload.manifest_text, "{\"schema_version\":2}");
    assert_eq!(ota_payload.firmware_bytes, vec![4, 5, 6]);

    let wired = load_wired_firmware_package_internal(&zip_path)
        .expect("load factory files from combined release zip");
    assert_eq!(wired.kind, WiredFirmwarePackageKind::Factory);
    assert_eq!(wired.version, "v1.2.6");
    assert_eq!(wired.artifacts.len(), 3);
    assert!(wired.files.contains_key("bootloader.bin"));

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&zip_path);
}

#[test]
fn load_wired_firmware_package_reads_factory_directory() {
    let root = std::env::temp_dir().join(format!(
        "listener-wired-factory-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create temp factory package dir");

    let bootloader = vec![0xe9, 1, 2, 3];
    let partition_table = vec![0xaa, 0xbb, 0xcc];
    let app = vec![0xe9, 9, 8, 7, 6];
    std::fs::write(root.join("bootloader.bin"), &bootloader).expect("write bootloader");
    std::fs::write(root.join("partition-table.bin"), &partition_table)
        .expect("write partition table");
    std::fs::write(root.join("voice-keyboard-firmware.bin"), &app).expect("write app");

    let manifest = format!(
        r#"{{
  "schema_version": 1,
  "project": "voice-keyboard-firmware",
  "version": "v1.2.3",
  "target": "esp32s3",
  "git_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "flash": {{
"baud": "921600",
"partition_table": [
  {{"name": "otadata", "offset": "0xf000", "size": "8K"}}
]
  }},
  "artifacts": [
{{"role": "bootloader", "file": "bootloader.bin", "offset": "0x0", "size_bytes": {}, "sha256": "{}"}},
{{"role": "partition_table", "file": "partition-table.bin", "offset": "0x8000", "size_bytes": {}, "sha256": "{}"}},
{{"role": "app", "file": "voice-keyboard-firmware.bin", "offset": "0x20000", "size_bytes": {}, "sha256": "{}"}}
  ]
}}"#,
        bootloader.len(),
        crate::firmware_ota::sha256_hex(&bootloader),
        partition_table.len(),
        crate::firmware_ota::sha256_hex(&partition_table),
        app.len(),
        crate::firmware_ota::sha256_hex(&app)
    );
    std::fs::write(root.join("manifest.json"), manifest).expect("write factory manifest");

    let loaded =
        load_wired_firmware_package_internal(&root).expect("load factory wired firmware package");
    assert_eq!(loaded.kind, WiredFirmwarePackageKind::Factory);
    assert_eq!(loaded.version, "v1.2.3");
    assert_eq!(
        loaded.otadata_region,
        Some(("0xf000".to_string(), "8192".to_string()))
    );
    assert_eq!(loaded.artifacts.len(), 3);
    assert_eq!(loaded.files.get("bootloader.bin"), Some(&bootloader));

    let payload = loaded.to_payload();
    assert_eq!(payload.kind, "factory");
    assert!(payload.supports_full_flash);
    assert!(payload.supports_boot_repair);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_wired_firmware_package_reads_nested_factory_directory() {
    let root = std::env::temp_dir().join(format!(
        "listener-wired-nested-factory-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let factory_dir = root.join("factory").join("listener-factory-test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&factory_dir).expect("create nested factory dir");
    write_test_factory_package(&factory_dir, "v1.2.5");

    let loaded = load_wired_firmware_package_internal(&root)
        .expect("load nested factory wired firmware package");
    assert_eq!(loaded.kind, WiredFirmwarePackageKind::Factory);
    assert_eq!(loaded.version, "v1.2.5");
    let payload = loaded.to_payload();
    assert_eq!(payload.kind, "factory");
    assert!(payload.supports_full_flash);
    assert!(payload.supports_boot_repair);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_wired_firmware_package_rejects_ota_zip() {
    let zip_path = std::env::temp_dir().join(format!(
        "listener-wired-reject-ota-zip-test-{}-{}.zip",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let _ = std::fs::remove_file(&zip_path);
    {
        let file = std::fs::File::create(&zip_path).expect("create temp OTA zip");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("listener-ota/ota_manifest.json", options)
            .expect("start OTA manifest");
        zip.write_all(b"{\"schema_version\":2}")
            .expect("write OTA manifest");
        zip.start_file("listener-ota/firmware_ota.bin", options)
            .expect("start OTA firmware");
        zip.write_all(&[0xe9, 1, 2, 3]).expect("write OTA firmware");
        zip.finish().expect("finish OTA zip");
    }

    let err = load_wired_firmware_package_internal(&zip_path)
        .expect_err("OTA zip should not be accepted for wired factory flashing");
    assert!(err.contains("factory package format"), "{err}");
    let _ = std::fs::remove_file(&zip_path);
}

#[test]
fn normalize_esptool_region_arg_accepts_idf_size_units() {
    assert_eq!(
        normalize_esptool_region_arg("8K", "size", false).unwrap(),
        "8192"
    );
    assert_eq!(
        normalize_esptool_region_arg("6M", "size", false).unwrap(),
        "6291456"
    );
    assert_eq!(
        normalize_esptool_region_arg("0xf000", "offset", true).unwrap(),
        "0xf000"
    );
    assert!(normalize_esptool_region_arg("0K", "size", false).is_err());
}

#[test]
fn parse_flash_u32_arg_accepts_idf_size_units() {
    assert_eq!(parse_flash_u32_arg("8K", "size", false).unwrap(), 8192);
    assert_eq!(
        parse_flash_u32_arg("0xf000", "offset", true).unwrap(),
        0xf000
    );
    assert!(parse_flash_u32_arg("5G", "size", false).is_err());
}

#[test]
fn prepare_esp_image_for_wired_flash_updates_header_and_digest() {
    let mut image = vec![0_u8; 96];
    image[0] = 0xe9;
    image[1] = 1;
    image[2] = 0;
    image[3] = 0;
    image[23] = 1;
    let digest_start = image.len() - 32;
    let digest = sha256_digest_bytes(&image[..digest_start]);
    image[digest_start..].copy_from_slice(&digest);

    let (patched, report) =
        super::prepare_esp_image_for_wired_flash("app", &image, super::Chip::Esp32s3)
            .expect("patch ESP image");

    assert!(report.changed);
    assert!(report.digest_recalculated);
    assert_eq!(patched[2], super::WIRED_FLASH_MODE as u8);
    assert_eq!(patched[3], 0x4f);
    let patched_digest = sha256_digest_bytes(&patched[..digest_start]);
    assert_eq!(&patched[digest_start..], &patched_digest);
}

fn write_test_factory_package(root: &std::path::Path, version: &str) {
    let bootloader = vec![0xe9, 1, 2, 3];
    let partition_table = vec![0xaa, 0xbb, 0xcc];
    let app = vec![0xe9, 9, 8, 7, 6];
    std::fs::write(root.join("bootloader.bin"), &bootloader).expect("write bootloader");
    std::fs::write(root.join("partition-table.bin"), &partition_table)
        .expect("write partition table");
    std::fs::write(root.join("voice-keyboard-firmware.bin"), &app).expect("write app");
    let manifest = format!(
        r#"{{
  "schema_version": 1,
  "project": "voice-keyboard-firmware",
  "version": "{version}",
  "target": "esp32s3",
  "git_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "flash": {{
"baud": "921600",
"partition_table": [
  {{"name": "otadata", "offset": "0xf000", "size": "0x2000"}}
]
  }},
  "artifacts": [
{{"role": "bootloader", "file": "bootloader.bin", "offset": "0x0", "size_bytes": {}, "sha256": "{}"}},
{{"role": "partition_table", "file": "partition-table.bin", "offset": "0x8000", "size_bytes": {}, "sha256": "{}"}},
{{"role": "app", "file": "voice-keyboard-firmware.bin", "offset": "0x20000", "size_bytes": {}, "sha256": "{}"}}
  ]
}}"#,
        bootloader.len(),
        crate::firmware_ota::sha256_hex(&bootloader),
        partition_table.len(),
        crate::firmware_ota::sha256_hex(&partition_table),
        app.len(),
        crate::firmware_ota::sha256_hex(&app)
    );
    std::fs::write(root.join("manifest.json"), manifest).expect("write factory manifest");
}

#[derive(Default)]
struct FakeSettingsWriter {
    saved: Mutex<Option<UserPreferences>>,
    dictation_refreshes: Mutex<u32>,
    qa_refreshes: Mutex<u32>,
    combo_refreshes: Mutex<u32>,
    device_key_refreshes: Mutex<u32>,
}

fn snapshot() -> CredentialsSnapshot {
    CredentialsSnapshot::default()
}

#[test]
fn diagnostic_redaction_masks_common_secret_tokens() {
    let line = "Authorization: Bearer sk-test api_key=abc access_token=def ok";
    let redacted = sanitize_diagnostic_log_line(line).expect("line retained");

    assert!(!redacted.contains("sk-test"));
    assert!(!redacted.contains("api_key=abc"));
    assert!(!redacted.contains("access_token=def"));
    assert!(redacted.contains("[redacted] [redacted]"));
    assert!(redacted.ends_with("ok"));
}

#[test]
fn diagnostic_recent_errors_filters_tail_without_transcript_fields() {
    let lines = vec![
        "INFO startup ok".to_string(),
        "WARN BLE timed out".to_string(),
        "INFO rawTranscript should not be selected by keyword alone".to_string(),
        "ERROR polish failed".to_string(),
    ];

    let errors = diagnostic_recent_errors(&lines, 10);

    assert_eq!(
        errors,
        vec![
            "WARN BLE timed out".to_string(),
            "ERROR polish failed".to_string()
        ]
    );
    assert!(is_diagnostic_error_line("BLE notify timeout"));
}

#[test]
fn diagnostic_recent_session_excludes_transcript_text_but_keeps_stats() {
    let stats = SessionStats {
        session_id: Some(7),
        explicit_start_received: true,
        start_inferred_from_audio: false,
        start_origin: None,
        terminal_received: true,
        end_reason: Some(SessionEndReason::Error(SessionErrorCode::QueueFull)),
        stop_origin: None,
        expected_packet_count: Some(4),
        received_packet_count: 3,
        missing_packet_count: 1,
        missing_packet_indices: vec![2],
        received_pcm_bytes: 1440,
        reconstructed_pcm_bytes: 1920,
        silence_filled_bytes: 480,
        duplicate_packet_count: 0,
        replaced_packet_count: 0,
        ignored_foreign_packet_count: 0,
        duration_seconds: 0.06,
        asr_boundary_pcm_bytes: 1440,
        asr_boundary_duration_seconds: 0.045,
        post_stop_packet_count: 1,
        post_stop_pcm_bytes: 480,
        post_stop_duration_seconds: 0.015,
    };
    let session = DictationSession {
        id: "session-1".into(),
        created_at: "2026-05-20T12:00:00Z".into(),
        raw_transcript: "do not export raw".into(),
        final_text: "do not export final".into(),
        mode: PolishMode::Light,
        app_bundle_id: Some("secret.app".into()),
        app_name: Some("Secret App".into()),
        insert_status: InsertStatus::Failed,
        error_code: Some("bleTimeout".into()),
        duration_ms: Some(60),
        dictionary_entry_count: Some(2),
        has_audio_recording: Some(true),
        embedded_audio_stats: Some(stats),
    };

    let diagnostic = super::diagnostic_recent_session(&session);
    let value = serde_json::to_value(&diagnostic).expect("serialize diagnostic session");

    assert_eq!(diagnostic.id, "session-1");
    assert_eq!(
        diagnostic
            .embedded_audio_stats
            .as_ref()
            .and_then(|stats| stats.session_id),
        Some(7)
    );
    assert_eq!(
        diagnostic
            .embedded_audio_stats
            .as_ref()
            .map(|stats| stats.missing_packet_count),
        Some(1)
    );
    assert!(value.get("rawTranscript").is_none());
    assert!(value.get("finalText").is_none());
    assert!(!value.to_string().contains("do not export"));
}

#[test]
fn diagnostic_package_includes_ble_wake_recovery_without_sensitive_text() {
    let coordinator = Arc::new(Coordinator::new());
    let package = super::build_diagnostic_package_with_ble_snapshot(
        &coordinator,
        crate::embedded_ble::BleDiagnosticSnapshot {
            captured_at: "2026-05-28T00:00:00Z".to_string(),
            platform: "windows",
            audio_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3091a",
            ota_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3092a",
            diagnostic_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3093a",
            dis_service_uuid: "0000180a-0000-1000-8000-00805f9b34fb",
            configured_device_address: Some("14C19F48FE72".to_string()),
            audio_services: vec![crate::embedded_ble::BleDiagnosticServiceEntry {
                selector: "audio",
                service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3091a",
                index: 0,
                name: "listener".to_string(),
                id: r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3091A}_14C19F48FE72".to_string(),
                bluetooth_address: Some("14C19F48FE72".to_string()),
            }],
            ota_services: Vec::new(),
            diagnostic_services: Vec::new(),
            firmware_snapshot: FirmwareOtaDeviceSnapshot {
                connected: true,
                hardware_revision: Some("keyboard-v1".to_string()),
                firmware_version: Some("v1.2.3".to_string()),
                capabilities: vec![
                    crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY.to_string()
                ],
                battery_percent: Some(88),
                usb_powered: Some(true),
                detail: None,
            },
            errors: Vec::new(),
        },
    )
    .expect("diagnostic package");
    let value = serde_json::to_value(&package).expect("serialize diagnostic package");

    assert_eq!(value["schemaVersion"], 3);
    assert_eq!(value["firmware"]["wakePolicy"]["policy"], "key4_only");
    assert_eq!(
        value["firmware"]["wakePolicy"]["voiceKeyDeepSleepWake"],
        false
    );
    assert!(value["ble"]["reconnectAttempts"].is_number());
    assert!(value["ble"]["notifySubscriptionState"].is_string());
    assert!(value["ble"]["backgroundListenerGeneration"].is_number());
    assert!(value["ble"]["diagnosticSnapshot"]["audioServices"].is_array());
    assert!(value["ble"]["diagnosticSnapshot"]["otaServices"].is_array());
    assert_eq!(
        value["ble"]["diagnosticSnapshot"]["audioServiceUuid"],
        "710af845-6d9f-6583-0c4d-9e5b3bc3091a"
    );
    assert_eq!(
        value["ble"]["diagnosticSnapshot"]["diagnosticServiceUuid"],
        "710af845-6d9f-6583-0c4d-9e5b3bc3093a"
    );
    assert!(value["ble"]["diagnosticSnapshot"]["diagnosticServices"].is_array());
    assert!(value["ble"]["failureTaxonomy"].is_array());
    assert!(value["ble"]["sessionActorHistory"].is_array());
    assert!(value["ble"].get("deviceAddress").is_some());
    assert!(value["ble"].get("firmwareVersion").is_some());
    assert!(value["ble"].get("batteryPercent").is_some());
    assert!(value["ble"]["capabilities"].is_array());
    assert!(
        value["ble"]["wakeRecovery"]["firmwareWakePolicy"]["readiness"]
            .as_str()
            .unwrap_or_default()
            .contains("voice_key_cannot_wake")
    );
    assert_eq!(value["privacy"]["excludesRawTranscripts"], true);
    assert_eq!(value["privacy"]["excludesApiKeyValues"], true);
    assert!(!value.to_string().to_lowercase().contains("api_key\":\""));
}

fn diagnostic_export_test_package() -> super::DiagnosticPackage {
    let coordinator = Arc::new(Coordinator::new());
    super::build_diagnostic_package_with_ble_snapshot(
        &coordinator,
        crate::embedded_ble::BleDiagnosticSnapshot {
            captured_at: "2026-05-28T00:00:00Z".to_string(),
            platform: "windows",
            audio_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3091a",
            ota_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3092a",
            diagnostic_service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3093a",
            dis_service_uuid: "0000180a-0000-1000-8000-00805f9b34fb",
            configured_device_address: Some("14C19F48FE72".to_string()),
            audio_services: Vec::new(),
            ota_services: Vec::new(),
            diagnostic_services: vec![crate::embedded_ble::BleDiagnosticServiceEntry {
                selector: "diagnostic",
                service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3093a",
                index: 0,
                name: "listener".to_string(),
                id: r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3093A}_14C19F48FE72".to_string(),
                bluetooth_address: Some("14C19F48FE72".to_string()),
            }],
            firmware_snapshot: FirmwareOtaDeviceSnapshot {
                connected: true,
                hardware_revision: Some("keyboard-v1".to_string()),
                firmware_version: Some("v1.2.3".to_string()),
                capabilities: vec![
                    crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY.to_string(),
                    "diag_export_v1".to_string(),
                ],
                battery_percent: Some(88),
                usb_powered: Some(true),
                detail: None,
            },
            errors: Vec::new(),
        },
    )
    .expect("diagnostic package")
}

#[test]
fn diagnostic_export_filename_includes_device_and_timestamp() {
    let package = diagnostic_export_test_package();
    let file_name = super::diagnostic_package_file_name(&package);
    assert!(file_name.starts_with("listener-type-diagnostic-v1.2.3-14c19f48fe72-"));
    assert!(file_name.ends_with(".zip"));
}

#[test]
fn diagnostic_zip_export_writes_desktop_data_and_offline_firmware_summary() {
    let package = diagnostic_export_test_package();
    let firmware_log = crate::embedded_ble::FirmwareDiagnosticLogPull::offline(
        "windows",
        "diagnostic service offline",
    );
    let zip_path = std::env::temp_dir().join(format!(
        "listener-diagnostic-offline-test-{}.zip",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&zip_path);
    super::write_diagnostic_package_zip(&zip_path, &package, &firmware_log)
        .expect("write diagnostic zip");

    let file = std::fs::File::open(&zip_path).expect("open diagnostic zip");
    let mut archive = zip::ZipArchive::new(file).expect("read diagnostic zip");
    assert!(archive.by_name("manifest.json").is_ok());
    assert!(archive.by_name("desktop/diagnostic_package.json").is_ok());
    assert!(archive
        .by_name("desktop/listener-type-log-tail.txt")
        .is_ok());
    assert!(archive
        .by_name("desktop/ble_connection_history.json")
        .is_ok());
    assert!(archive
        .by_name("desktop/audio_samples_manifest.json")
        .is_ok());
    assert!(archive.by_name("firmware/diag_log_summary.json").is_ok());
    assert!(archive.by_name("firmware/diag_log.bin").is_err());

    let mut summary = String::new();
    archive
        .by_name("firmware/diag_log_summary.json")
        .expect("firmware summary")
        .read_to_string(&mut summary)
        .expect("read summary");
    let summary: serde_json::Value =
        serde_json::from_str(&summary).expect("parse firmware summary");
    assert_eq!(summary["status"], "offline");
    let _ = std::fs::remove_file(&zip_path);
}

#[test]
fn diagnostic_zip_export_includes_firmware_diag_log_bin_when_available() {
    let package = diagnostic_export_test_package();
    let raw_events = vec![7u8; crate::embedded_ble::DIAGNOSTIC_EVENT_BYTES * 2];
    let firmware_log = crate::embedded_ble::FirmwareDiagnosticLogPull::from_events(
        "windows",
        2,
        2,
        2,
        vec![crate::embedded_ble::FirmwareDiagnosticLogChunk {
            offset: 0,
            event_count: 2,
            value_bytes: crate::embedded_ble::DIAGNOSTIC_CHUNK_HEADER_BYTES + raw_events.len(),
            events_crc32: "0x00000000".to_string(),
        }],
        raw_events.clone(),
    );
    let zip_path = std::env::temp_dir().join(format!(
        "listener-diagnostic-firmware-test-{}.zip",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&zip_path);
    super::write_diagnostic_package_zip(&zip_path, &package, &firmware_log)
        .expect("write diagnostic zip");

    let file = std::fs::File::open(&zip_path).expect("open diagnostic zip");
    let mut archive = zip::ZipArchive::new(file).expect("read diagnostic zip");
    let mut firmware_bytes = Vec::new();
    archive
        .by_name("firmware/diag_log.bin")
        .expect("firmware diag log")
        .read_to_end(&mut firmware_bytes)
        .expect("read firmware bytes");
    assert_eq!(firmware_bytes, raw_events);
    let _ = std::fs::remove_file(&zip_path);
}

#[test]
fn diagnostic_audio_manifest_marks_included_debug_wav_samples() {
    let package = diagnostic_export_test_package();
    let samples = vec![super::DiagnosticAudioSampleFile {
        session_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
        zip_path: "desktop/audio_samples/123e4567-e89b-12d3-a456-426614174000.wav".to_string(),
        bytes: vec![1, 2, 3, 4],
    }];
    let manifest = super::diagnostic_audio_samples_manifest(&package, &samples);
    assert_eq!(manifest["audioContentsIncluded"], true);
    assert_eq!(manifest["audioRecordingsIncluded"], true);
    assert_eq!(manifest["includedRecordings"][0]["bytes"], 4);
    assert_eq!(
        manifest["includedRecordings"][0]["path"],
        "desktop/audio_samples/123e4567-e89b-12d3-a456-426614174000.wav"
    );
}

#[test]
fn diagnostic_ble_failure_taxonomy_deduplicates_sources() {
    let snapshot = crate::embedded_ble::BleDiagnosticSnapshot {
        captured_at: "2026-05-28T00:00:00Z".to_string(),
        platform: "windows",
        audio_service_uuid: "audio",
        ota_service_uuid: "ota",
        diagnostic_service_uuid: "diagnostic",
        dis_service_uuid: "dis",
        configured_device_address: Some("14C19F48FE72".to_string()),
        audio_services: Vec::new(),
        ota_services: Vec::new(),
        diagnostic_services: Vec::new(),
        firmware_snapshot: FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: Some("keyboard-v1".to_string()),
            firmware_version: None,
            capabilities: vec![crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY.to_string()],
            battery_percent: Some(70),
            usb_powered: Some(true),
            detail: None,
        },
        errors: vec!["Unknown GATT service from stale cached table".to_string()],
    };

    let taxonomy = super::diagnostic_ble_failure_taxonomy(
        Some("BLE CCCD notify write returned status=ProtocolError".to_string()),
        Some("BLE CCCD notify write returned status=ProtocolError".to_string()),
        &[
            "background listener already active; foreground probe skipped".to_string(),
            "OTA reboot window still confirming version".to_string(),
            "Windows Bluetooth service reset needed after radio error".to_string(),
        ],
        &snapshot,
    );
    let kinds: Vec<_> = taxonomy
        .iter()
        .map(|classification| classification.kind)
        .collect();

    assert!(kinds.contains(&crate::embedded_ble::BleFailureKind::CccdProtocolError));
    assert!(kinds.contains(&crate::embedded_ble::BleFailureKind::BackgroundListenerContention));
    assert!(kinds.contains(&crate::embedded_ble::BleFailureKind::OtaRebootWindow));
    assert!(
        kinds.contains(&crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded)
    );
    assert!(kinds.contains(&crate::embedded_ble::BleFailureKind::StaleGattService));
    assert!(kinds.contains(&crate::embedded_ble::BleFailureKind::MissingDisFirmwareRevision));
    assert_eq!(
        taxonomy
            .iter()
            .filter(|classification| classification.kind
                == crate::embedded_ble::BleFailureKind::CccdProtocolError)
            .count(),
        1
    );
}

#[test]
fn credentials_status_follows_active_asr_provider_requirements() {
    let volcengine = CredentialsSnapshot {
        volcengine_app_key: Some("app".into()),
        volcengine_access_key: Some("access".into()),
        volcengine_resource_id: Some("resource".into()),
        ..snapshot()
    };
    assert!(asr_configured_for_provider("volcengine", &volcengine));

    let whisper_key_only = CredentialsSnapshot {
        asr_api_key: Some("key".into()),
        ..snapshot()
    };
    assert!(!asr_configured_for_provider("whisper", &whisper_key_only));
    assert!(asr_configured_for_provider(
        crate::asr::bailian::PROVIDER_ID,
        &whisper_key_only
    ));

    let whisper_keyless_ready = CredentialsSnapshot {
        asr_endpoint: Some("https://api.openai.com/v1".into()),
        asr_model: Some("whisper-1".into()),
        ..snapshot()
    };
    assert!(asr_configured_for_provider(
        "whisper",
        &whisper_keyless_ready
    ));
    assert!(!asr_configured_for_provider(
        crate::asr::bailian::PROVIDER_ID,
        &whisper_keyless_ready
    ));

    assert!(asr_configured_for_provider(
        crate::asr::local::PROVIDER_ID,
        &snapshot()
    ));
    #[cfg(target_os = "windows")]
    assert!(asr_configured_for_provider(
        crate::asr::local::foundry::PROVIDER_ID,
        &snapshot()
    ));
    #[cfg(not(target_os = "windows"))]
    assert!(!asr_configured_for_provider(
        crate::asr::local::foundry::PROVIDER_ID,
        &snapshot()
    ));
}

#[test]
fn credentials_status_treats_foundry_local_asr_as_configured() {
    #[cfg(target_os = "windows")]
    {
        assert!(asr_configured_for_provider(
            crate::asr::local::foundry::PROVIDER_ID,
            &CredentialsSnapshot::default()
        ));
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert!(!asr_configured_for_provider(
            crate::asr::local::foundry::PROVIDER_ID,
            &CredentialsSnapshot::default()
        ));
    }
}

#[test]
fn local_asr_providers_skip_external_validation() {
    assert!(active_asr_is_keyless_for_validation(
        crate::asr::local::PROVIDER_ID
    ));
    #[cfg(target_os = "windows")]
    assert!(active_asr_is_keyless_for_validation(
        crate::asr::local::foundry::PROVIDER_ID
    ));
    #[cfg(not(target_os = "windows"))]
    assert!(!active_asr_is_keyless_for_validation(
        crate::asr::local::foundry::PROVIDER_ID
    ));
    assert!(!active_asr_is_keyless_for_validation("volcengine"));
    assert!(!active_asr_is_keyless_for_validation("whisper"));
}

#[test]
fn provider_switch_release_plan_covers_inactive_local_runtimes() {
    let qwen = local_asr_release_plan_for_provider(crate::asr::local::PROVIDER_ID);
    assert!(!qwen.qwen);
    assert!(qwen.foundry);

    let foundry = local_asr_release_plan_for_provider(crate::asr::local::foundry::PROVIDER_ID);
    assert!(foundry.qwen);
    assert!(!foundry.foundry);

    let cloud = local_asr_release_plan_for_provider("volcengine");
    assert!(cloud.qwen);
    assert!(cloud.foundry);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn provider_switch_release_requests_foundry_prepare_cancel_first() {
    let runtime = std::sync::Arc::new(crate::asr::local::FoundryLocalRuntime::new());

    release_foundry_runtime_if_inactive(&runtime, true).await;

    assert!(runtime.cancel_prepare_requested_for_tests());
}

#[test]
fn foundry_language_hint_accepts_empty_and_lowercase_iso_639_1() {
    assert_eq!(normalize_foundry_language_hint("").unwrap(), "");
    assert_eq!(normalize_foundry_language_hint("   ").unwrap(), "");
    assert_eq!(normalize_foundry_language_hint("zh").unwrap(), "zh");
    assert_eq!(normalize_foundry_language_hint(" en ").unwrap(), "en");
}

#[test]
fn foundry_language_hint_rejects_non_lowercase_iso_639_1() {
    assert!(normalize_foundry_language_hint("ZH").is_err());
    assert!(normalize_foundry_language_hint("zho").is_err());
    assert!(normalize_foundry_language_hint("z1").is_err());
}

#[test]
fn foundry_model_alias_validation_rejects_unknown_alias() {
    assert!(validate_foundry_model_alias(crate::asr::local::foundry::DEFAULT_MODEL_ALIAS).is_ok());
    assert!(validate_foundry_model_alias("whisper-large").is_err());
}

#[test]
fn foundry_active_model_pref_falls_back_to_default_for_unknown_alias() {
    let prefs = UserPreferences {
        foundry_local_asr_model: "whisper-large".to_string(),
        ..Default::default()
    };

    assert_eq!(
        active_foundry_model_from_prefs(&prefs),
        crate::asr::local::foundry::DEFAULT_MODEL_ALIAS
    );
}

#[test]
fn credentials_status_accepts_keyless_custom_llm_only() {
    let keyless_ready = CredentialsSnapshot {
        ark_endpoint: Some("http://localhost:11434/v1".into()),
        ark_model_id: Some("qwen".into()),
        ..snapshot()
    };
    assert!(llm_configured_for_provider("custom", &keyless_ready));
    assert!(llm_configured_for_provider("self-hosted", &keyless_ready));
    assert!(llm_configured_for_provider(
        "openrouterFree",
        &keyless_ready
    ));

    let hosted_keyless = CredentialsSnapshot {
        ark_endpoint: Some("https://openrouter.ai/api/v1".into()),
        ark_model_id: Some("qwen/qwen3-coder:free".into()),
        ..snapshot()
    };
    assert!(!llm_configured_for_provider(
        "openrouterFree",
        &hosted_keyless
    ));

    let hosted_ready = CredentialsSnapshot {
        ark_api_key: Some("key".into()),
        ark_endpoint: Some("https://openrouter.ai/api/v1/chat/completions".into()),
        ark_model_id: Some("qwen/qwen3-coder:free".into()),
        ..snapshot()
    };
    assert!(llm_configured_for_provider("openrouterFree", &hosted_ready));

    let key_without_endpoint = CredentialsSnapshot {
        ark_api_key: Some("key".into()),
        ark_model_id: Some("qwen".into()),
        ..snapshot()
    };
    assert!(!llm_configured_for_provider(
        "custom",
        &key_without_endpoint
    ));

    let endpoint_without_model = CredentialsSnapshot {
        ark_endpoint: Some("http://localhost:11434/v1".into()),
        ..snapshot()
    };
    assert!(!llm_configured_for_provider(
        "custom",
        &endpoint_without_model
    ));
}

impl SettingsWriter for FakeSettingsWriter {
    fn sync_active_providers(
        &self,
        _active_asr_provider: &str,
        _active_llm_provider: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    fn write_settings(&self, prefs: UserPreferences) -> Result<(), String> {
        *self.saved.lock().unwrap() = Some(prefs);
        Ok(())
    }

    fn refresh_dictation_hotkey(&self) {
        *self.dictation_refreshes.lock().unwrap() += 1;
    }

    fn refresh_qa_hotkey(&self) {
        *self.qa_refreshes.lock().unwrap() += 1;
    }

    fn refresh_combo_hotkey(&self) {
        *self.combo_refreshes.lock().unwrap() += 1;
    }

    fn refresh_translation_hotkey(&self) {}
    fn refresh_switch_style_hotkey(&self) {}
    fn refresh_open_app_hotkey(&self) {}
    fn refresh_device_custom_key_hotkeys(&self) {
        *self.device_key_refreshes.lock().unwrap() += 1;
    }
}

#[test]
fn models_url_accepts_base_or_chat_endpoint() {
    assert_eq!(
        models_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/models"
    );
    assert_eq!(
        models_url("https://api.openai.com/v1/chat/completions"),
        "https://api.openai.com/v1/models"
    );
}

#[test]
fn asr_transcriptions_url_accepts_base_or_transcriptions_endpoint() {
    assert_eq!(
        asr_transcriptions_url("https://api.openai.com/v1").unwrap(),
        "https://api.openai.com/v1/audio/transcriptions"
    );
    assert_eq!(
        asr_transcriptions_url("https://api.openai.com/v1/chat/completions").unwrap(),
        "https://api.openai.com/v1/audio/transcriptions"
    );
    assert_eq!(
        asr_transcriptions_url("https://api.openai.com/v1/audio").unwrap(),
        "https://api.openai.com/v1/audio/transcriptions"
    );
    assert_eq!(
        asr_transcriptions_url("https://api.openai.com/v1/audio/transcriptions").unwrap(),
        "https://api.openai.com/v1/audio/transcriptions"
    );
    assert_eq!(
        asr_transcriptions_url("https://api.openai.com/v1?api-version=2024-12-01").unwrap(),
        "https://api.openai.com/v1/audio/transcriptions?api-version=2024-12-01"
    );
}

#[test]
fn parse_model_ids_sorts_and_deduplicates() {
    let models =
        parse_model_ids(r#"{ "data": [{ "id": "b" }, { "id": "a" }, { "id": "b" }] }"#).unwrap();
    assert_eq!(models, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn parse_gemini_model_ids_strips_models_prefix_and_dedups() {
    // Google v1beta/models 真实响应的子集——name 字段带 `models/` 前缀，
    // ProviderTools 选中后写入 ark.model_id 时不能带这个前缀（generateContent
    // URL 拼接已经会加 `models/`，不去前缀就会变成 `models/models/...`）。
    // 字段缺失时保守保留（视为支持 generateContent）。
    let body = r#"{"models":[
        {"name":"models/gemini-2.5-pro"},
        {"name":"models/gemini-2.5-flash"},
        {"name":"models/gemini-2.5-flash"},
        {"name":"models/gemini-3-flash-preview"}
    ]}"#;
    let ids = parse_gemini_model_ids(body).unwrap();
    assert_eq!(
        ids,
        vec![
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-pro".to_string(),
            "gemini-3-flash-preview".to_string(),
        ]
    );
}

#[test]
fn parse_gemini_model_ids_filters_out_non_generate_content_families() {
    // 真实 Google v1beta/models 响应里同时有 generateContent / embedContent /
    // generateMessage 等多种家族。用户选中 embedding/TTS/image 模型写入
    // ark.model_id → polish 必败。这里是 PR #398 pr_agent advisory 的回归用例：
    // 只把 supportedGenerationMethods 里含 generateContent 的过滤出来。
    let body = r#"{"models":[
        {"name":"models/gemini-2.5-flash","supportedGenerationMethods":["generateContent","streamGenerateContent","countTokens"]},
        {"name":"models/gemini-embedding-2","supportedGenerationMethods":["embedContent"]},
        {"name":"models/text-embedding-004","supportedGenerationMethods":["embedContent","countTextTokens"]},
        {"name":"models/gemini-2.5-pro-preview-tts","supportedGenerationMethods":["generateContent"]},
        {"name":"models/gemini-2.5-flash-image","supportedGenerationMethods":["predict"]}
    ]}"#;
    let ids = parse_gemini_model_ids(body).unwrap();
    // 只剩两条声明 generateContent 的；embedding 与 image (predict-only) 必须被过滤。
    assert_eq!(
        ids,
        vec![
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-pro-preview-tts".to_string(),
        ]
    );
}

#[test]
fn is_gemini_base_url_matches_official_domain() {
    assert!(is_gemini_base_url(
        "https://generativelanguage.googleapis.com/v1beta"
    ));
    assert!(is_gemini_base_url(
        "https://generativelanguage.googleapis.com/v1beta/"
    ));
    assert!(!is_gemini_base_url("https://api.openai.com/v1"));
    assert!(!is_gemini_base_url(
        "https://ark.cn-beijing.volces.com/api/v3"
    ));
}

#[test]
fn persist_settings_refreshes_both_hotkey_pipelines() {
    let writer = FakeSettingsWriter::default();
    let prefs = UserPreferences {
        hotkey: HotkeyBinding {
            trigger: HotkeyTrigger::RightControl,
            mode: HotkeyMode::Toggle,
            ..Default::default()
        },
        qa_hotkey: Some(ShortcutBinding {
            primary: ";".to_string(),
            modifiers: vec!["ctrl".to_string(), "shift".to_string()],
        }),
        ..Default::default()
    };

    persist_settings(&writer, prefs.clone()).unwrap();

    let saved = writer
        .saved
        .lock()
        .unwrap()
        .clone()
        .expect("settings saved");
    assert_eq!(saved.hotkey.trigger, HotkeyBinding::default().trigger);
    assert_eq!(saved.hotkey.mode, prefs.hotkey.mode);
    assert_eq!(
        saved.qa_hotkey.unwrap().primary,
        prefs.qa_hotkey.unwrap().primary
    );
    assert_eq!(*writer.dictation_refreshes.lock().unwrap(), 1);
    assert_eq!(*writer.qa_refreshes.lock().unwrap(), 1);
    assert_eq!(*writer.combo_refreshes.lock().unwrap(), 1);
    assert_eq!(*writer.device_key_refreshes.lock().unwrap(), 1);
}

#[test]
fn validate_device_shortcut_rejects_self_triggering_fallback() {
    let mapping = DeviceCustomKeyMapping {
        action: DeviceCustomKeyAction::SendShortcut,
        shortcut: Some(ShortcutBinding {
            primary: "F13".into(),
            modifiers: vec![],
        }),
        ..Default::default()
    };

    assert_eq!(
        super::validate_device_custom_key_mapping(&mapping),
        Err("设备自定义键不能转发为设备 fallback 快捷键，避免重复触发自身".into())
    );
}

#[test]
fn validate_device_shortcut_rejects_ec11_fallback_combo() {
    let mapping = DeviceCustomKeyMapping {
        action: DeviceCustomKeyAction::SendShortcut,
        shortcut: Some(ShortcutBinding {
            primary: "F13".into(),
            modifiers: vec!["shift".into()],
        }),
        ..Default::default()
    };

    assert_eq!(
        super::validate_device_custom_key_mapping(&mapping),
        Err("设备自定义键不能转发为设备 fallback 快捷键，避免重复触发自身".into())
    );
}

#[test]
fn validate_device_shortcut_accepts_modified_shortcut() {
    let mapping = DeviceCustomKeyMapping {
        action: DeviceCustomKeyAction::SendShortcut,
        shortcut: Some(ShortcutBinding {
            primary: "K".into(),
            modifiers: vec!["ctrl".into(), "shift".into()],
        }),
        ..Default::default()
    };

    assert!(super::validate_device_custom_key_mapping(&mapping).is_ok());
}

#[test]
fn validate_device_shortcut_accepts_modifier_only_key_output() {
    let mapping = DeviceCustomKeyMapping {
        action: DeviceCustomKeyAction::SendShortcut,
        shortcut: Some(ShortcutBinding {
            primary: "RightControl".into(),
            modifiers: vec![],
        }),
        ..Default::default()
    };

    assert!(super::validate_device_custom_key_mapping(&mapping).is_ok());
}

#[test]
fn validate_shortcut_binding_rejects_device_fallback_hotkey() {
    let binding = ShortcutBinding {
        primary: "F14".into(),
        modifiers: vec![],
    };

    assert_eq!(
        super::validate_shortcut_binding(binding),
        Err("设备 fallback 快捷键已保留给 KEY1-KEY4 和 EC11 单击入口".into())
    );
}

#[test]
fn validate_shortcut_binding_rejects_ec11_fallback_hotkey() {
    let binding = ShortcutBinding {
        primary: "F13".into(),
        modifiers: vec!["shift".into()],
    };

    assert_eq!(
        super::validate_shortcut_binding(binding),
        Err("设备 fallback 快捷键已保留给 KEY1-KEY4 和 EC11 单击入口".into())
    );
}

#[test]
fn validate_shortcut_binding_allows_modified_function_hotkey() {
    let binding = ShortcutBinding {
        primary: "F14".into(),
        modifiers: vec!["ctrl".into()],
    };

    assert!(super::validate_shortcut_binding(binding).is_ok());
}

#[test]
fn sync_dictation_hotkey_sets_modifier_trigger_and_clears_combo() {
    let mut prefs = UserPreferences {
        hotkey: HotkeyBinding {
            trigger: HotkeyTrigger::Custom,
            mode: HotkeyMode::Toggle,
            keys: None,
        },
        custom_combo_hotkey: Some(ComboBinding {
            primary: "D".into(),
            modifiers: vec!["cmd".into(), "shift".into()],
        }),
        dictation_hotkey: ShortcutBinding {
            primary: "RightControl".into(),
            modifiers: vec![],
        },
        ..Default::default()
    };

    super::sync_dictation_hotkey_legacy_fields(&mut prefs);

    assert_eq!(prefs.hotkey.trigger, HotkeyTrigger::RightControl);
    assert!(prefs.custom_combo_hotkey.is_none());
}

#[test]
fn sync_dictation_hotkey_normalizes_legacy_hold_mode() {
    let mut prefs = UserPreferences {
        hotkey: HotkeyBinding {
            trigger: HotkeyTrigger::RightControl,
            mode: HotkeyMode::Hold,
            keys: None,
        },
        dictation_hotkey: ShortcutBinding {
            primary: "RightControl".into(),
            modifiers: vec![],
        },
        ..Default::default()
    };

    super::sync_dictation_hotkey_legacy_fields(&mut prefs);

    assert_eq!(prefs.hotkey.mode, HotkeyMode::Toggle);
}

#[test]
fn sync_dictation_hotkey_sets_custom_trigger_and_combo_binding() {
    let mut prefs = UserPreferences {
        hotkey: HotkeyBinding {
            trigger: HotkeyTrigger::RightControl,
            mode: HotkeyMode::Toggle,
            keys: None,
        },
        dictation_hotkey: ShortcutBinding {
            primary: "D".into(),
            modifiers: vec!["cmd".into(), "shift".into()],
        },
        ..Default::default()
    };

    super::sync_dictation_hotkey_legacy_fields(&mut prefs);

    assert_eq!(prefs.hotkey.trigger, HotkeyTrigger::Custom);
    let combo = prefs.custom_combo_hotkey.expect("combo binding saved");
    assert_eq!(combo.primary, "D");
    assert_eq!(
        combo.modifiers,
        vec!["cmd".to_string(), "shift".to_string()]
    );
}

#[test]
fn sync_dictation_hotkey_clears_empty_custom_binding() {
    let mut prefs = UserPreferences {
        hotkey: HotkeyBinding {
            trigger: HotkeyTrigger::RightControl,
            mode: HotkeyMode::Toggle,
            keys: None,
        },
        custom_combo_hotkey: Some(ComboBinding {
            primary: "D".into(),
            modifiers: vec!["cmd".into(), "shift".into()],
        }),
        dictation_hotkey: ShortcutBinding {
            primary: " ".into(),
            modifiers: vec!["cmd".into()],
        },
        ..Default::default()
    };

    super::sync_dictation_hotkey_legacy_fields(&mut prefs);

    assert_eq!(prefs.hotkey.trigger, HotkeyTrigger::Custom);
    assert!(prefs.custom_combo_hotkey.is_none());
}

#[test]
fn validate_combo_hotkey_rejects_bare_shift() {
    let result = super::validate_combo_hotkey(ComboBinding {
        primary: "Shift".into(),
        modifiers: vec![],
    });

    assert!(result.is_err());
}

#[test]
fn validate_combo_hotkey_rejects_device_fallback_hotkey() {
    let result = super::validate_combo_hotkey(ComboBinding {
        primary: "F15".into(),
        modifiers: vec![],
    });

    assert_eq!(
        result,
        Err("设备 fallback 快捷键已保留给 KEY1-KEY4 和 EC11 单击入口".into())
    );
}

#[test]
fn validate_combo_hotkey_rejects_ec11_fallback_hotkey() {
    let result = super::validate_combo_hotkey(ComboBinding {
        primary: "F13".into(),
        modifiers: vec!["shift".into()],
    });

    assert_eq!(
        result,
        Err("设备 fallback 快捷键已保留给 KEY1-KEY4 和 EC11 单击入口".into())
    );
}

#[test]
fn combo_hotkey_bare_shift_rejection_matches_dictation_setter() {
    let binding = ShortcutBinding {
        primary: "Shift".into(),
        modifiers: vec![],
    };

    assert_eq!(
        super::reject_bare_shift_dictation_shortcut(&binding),
        Err("Shift 单键目前只能用于翻译快捷键".into())
    );
}

#[test]
fn dictation_qa_overlap_rejects_same_modifier_only_binding() {
    let binding = ShortcutBinding {
        primary: "RightControl".into(),
        modifiers: vec![],
    };

    assert_eq!(
        super::reject_dictation_qa_hotkey_overlap(&binding, &binding),
        Err("QA 快捷键不能和听写快捷键相同".into())
    );
}

#[test]
fn dictation_qa_overlap_rejects_same_combo_binding() {
    let dictation = ShortcutBinding {
        primary: ";".into(),
        modifiers: vec!["ctrl".into(), "shift".into()],
    };
    let qa = ShortcutBinding {
        primary: ";".into(),
        modifiers: vec!["control".into(), "shift".into()],
    };

    assert_eq!(
        super::reject_dictation_qa_hotkey_overlap(&dictation, &qa),
        Err("QA 快捷键不能和听写快捷键相同".into())
    );
}

#[test]
fn dictation_qa_overlap_allows_distinct_bindings() {
    let dictation = ShortcutBinding {
        primary: "RightControl".into(),
        modifiers: vec![],
    };
    let qa = ShortcutBinding {
        primary: ";".into(),
        modifiers: vec!["ctrl".into(), "shift".into()],
    };

    assert!(super::reject_dictation_qa_hotkey_overlap(&dictation, &qa).is_ok());
}

#[test]
fn dictation_translation_overlap_rejects_same_modifier_only_binding() {
    let binding = ShortcutBinding {
        primary: "RightControl".into(),
        modifiers: vec![],
    };

    assert_eq!(
        super::reject_dictation_translation_hotkey_overlap(&binding, &binding),
        Err("翻译快捷键不能和听写快捷键相同".into())
    );
}

#[test]
fn dictation_translation_overlap_rejects_same_combo_binding() {
    let dictation = ShortcutBinding {
        primary: "T".into(),
        modifiers: vec!["ctrl".into(), "shift".into()],
    };
    let translation = ShortcutBinding {
        primary: "T".into(),
        modifiers: vec!["control".into(), "shift".into()],
    };

    assert_eq!(
        super::reject_dictation_translation_hotkey_overlap(&dictation, &translation),
        Err("翻译快捷键不能和听写快捷键相同".into())
    );
}

#[test]
fn dictation_translation_overlap_allows_distinct_bindings() {
    let dictation = ShortcutBinding {
        primary: "RightControl".into(),
        modifiers: vec![],
    };
    let translation = ShortcutBinding {
        primary: "Shift".into(),
        modifiers: vec![],
    };

    assert!(super::reject_dictation_translation_hotkey_overlap(&dictation, &translation).is_ok());
}

#[test]
fn persist_settings_rejects_dictation_translation_overlap() {
    let writer = FakeSettingsWriter::default();
    let binding = ShortcutBinding {
        primary: "RightControl".into(),
        modifiers: vec![],
    };
    let prefs = UserPreferences {
        dictation_hotkey: binding.clone(),
        translation_hotkey: binding,
        ..Default::default()
    };

    assert_eq!(
        persist_settings(&writer, prefs),
        Err("翻译快捷键不能和听写快捷键相同".into())
    );
    assert!(writer.saved.lock().unwrap().is_none());
}

#[test]
fn persist_settings_rejects_translation_switch_style_overlap() {
    let writer = FakeSettingsWriter::default();
    let binding = ShortcutBinding {
        primary: "T".into(),
        modifiers: vec!["cmd".into(), "shift".into()],
    };
    let prefs = UserPreferences {
        translation_hotkey: binding.clone(),
        switch_style_hotkey: binding,
        ..Default::default()
    };

    assert_eq!(
        persist_settings(&writer, prefs),
        Err("切换风格快捷键不能和翻译快捷键相同".into())
    );
    assert!(writer.saved.lock().unwrap().is_none());
}

#[test]
fn persist_settings_rejects_switch_style_open_app_overlap() {
    let writer = FakeSettingsWriter::default();
    let binding = ShortcutBinding {
        primary: "K".into(),
        modifiers: vec!["cmd".into(), "shift".into()],
    };
    let prefs = UserPreferences {
        switch_style_hotkey: binding.clone(),
        open_app_hotkey: binding,
        ..Default::default()
    };

    assert_eq!(
        persist_settings(&writer, prefs),
        Err("打开应用快捷键不能和切换风格快捷键相同".into())
    );
    assert!(writer.saved.lock().unwrap().is_none());
}

#[test]
fn persist_settings_rejects_device_fallback_dictation_hotkey() {
    let writer = FakeSettingsWriter::default();
    let prefs = UserPreferences {
        dictation_hotkey: ShortcutBinding {
            primary: "F13".into(),
            modifiers: vec![],
        },
        ..Default::default()
    };

    assert_eq!(
        persist_settings(&writer, prefs),
        Err("设备 fallback 快捷键已保留给 KEY1-KEY4 和 EC11 单击入口".into())
    );
    assert!(writer.saved.lock().unwrap().is_none());
}

#[test]
fn persist_settings_rejects_device_fallback_qa_hotkey() {
    let writer = FakeSettingsWriter::default();
    let prefs = UserPreferences {
        qa_hotkey: Some(ShortcutBinding {
            primary: "F16".into(),
            modifiers: vec![],
        }),
        ..Default::default()
    };

    assert_eq!(
        persist_settings(&writer, prefs),
        Err("设备 fallback 快捷键已保留给 KEY1-KEY4 和 EC11 单击入口".into())
    );
    assert!(writer.saved.lock().unwrap().is_none());
}

#[tokio::test]
async fn fetch_provider_models_omits_authorization_when_api_key_is_empty() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let mut request = Vec::new();
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        assert!(!request_text.contains("Authorization: Bearer"));

        let body = r#"{"data":[{"id":"m1"},{"id":"m2"}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let models = fetch_provider_models(&ProviderConfig {
        provider_id: "custom".to_string(),
        base_url: format!("http://{}", addr),
        api_key: String::new(),
        proxy_config: ProviderProxyConfig::provider_default("custom"),
    })
    .await
    .unwrap();

    assert_eq!(models, vec!["m1".to_string(), "m2".to_string()]);
    server.join().unwrap();
}

#[tokio::test]
async fn fetch_provider_models_sends_bearer_token_for_openai_compatible_providers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let mut request = Vec::new();
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        let request_text_lower = request_text.to_ascii_lowercase();
        assert!(request_text.starts_with("GET /openai/v1/models "));
        assert!(request_text_lower.contains("authorization: bearer test-token"));

        let body = r#"{"data":[{"id":"deepseek-chat"},{"id":"deepseek-reasoner"}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let models = fetch_provider_models(&ProviderConfig {
        provider_id: "deepseek".to_string(),
        base_url: format!("http://{addr}/openai/v1"),
        api_key: "test-token".to_string(),
        proxy_config: ProviderProxyConfig::provider_default("deepseek"),
    })
    .await
    .unwrap();

    assert_eq!(
        models,
        vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()]
    );
    server.join().unwrap();
}

#[tokio::test]
async fn fetch_provider_models_cached_reuses_recent_result_for_same_credentials() {
    let _cache_test_guard = provider_models_cache_test_lock().lock().await;
    provider_models_cache().lock().clear();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 8192];
        let mut request = Vec::new();
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        let body = r#"{"data":[{"id":"cached-model"}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let config = ProviderConfig {
        provider_id: "openai".to_string(),
        base_url: format!("http://{addr}/v1"),
        api_key: "cache-key".to_string(),
        proxy_config: ProviderProxyConfig::provider_default("openai"),
    };

    let first = fetch_provider_models_cached("llm", &config).await.unwrap();
    let second = fetch_provider_models_cached("llm", &config).await.unwrap();

    assert_eq!(first, vec!["cached-model".to_string()]);
    assert_eq!(second, first);
    server.join().unwrap();
    provider_models_cache().lock().clear();
}

#[tokio::test]
async fn fetch_provider_models_cached_coalesces_concurrent_misses() {
    let _cache_test_guard = provider_models_cache_test_lock().lock().await;
    provider_models_cache().lock().clear();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let server_done = Arc::clone(&done);

    let server = thread::spawn(move || {
        let mut request_count = 0usize;
        let mut buf = [0u8; 8192];
        while !server_done.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    request_count += 1;
                    stream.set_nonblocking(false).unwrap();
                    let mut request = Vec::new();
                    loop {
                        let n = stream.read(&mut buf).unwrap();
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buf[..n]);
                        if request.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }

                    let body = r#"{"data":[{"id":"concurrent-model"}]}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("provider model test server failed: {error}"),
            }
        }
        request_count
    });

    let config = ProviderConfig {
        provider_id: "openai".to_string(),
        base_url: format!("http://{addr}/v1"),
        api_key: "cache-key".to_string(),
        proxy_config: ProviderProxyConfig::provider_default("openai"),
    };

    let (first, second) = tokio::join!(
        fetch_provider_models_cached("llm", &config),
        fetch_provider_models_cached("llm", &config)
    );

    done.store(true, Ordering::SeqCst);
    assert_eq!(first.unwrap(), vec!["concurrent-model".to_string()]);
    assert_eq!(second.unwrap(), vec!["concurrent-model".to_string()]);
    assert_eq!(server.join().unwrap(), 1);
    provider_models_cache().lock().clear();
}

#[test]
fn is_valid_session_id_accepts_canonical_uuid_v4() {
    // canonical UUID-v4 字面：8-4-4-4-12，全小写、全大写、混合都接受。
    assert!(is_valid_session_id("550e8400-e29b-41d4-a716-446655440000"));
    assert!(is_valid_session_id("550E8400-E29B-41D4-A716-446655440000"));
    assert!(is_valid_session_id("Abc12345-6789-abcd-EF01-234567890abc"));
}

#[test]
fn is_valid_session_id_rejects_path_traversal_and_garbage() {
    assert!(!is_valid_session_id(""));
    assert!(!is_valid_session_id("../../etc/passwd"));
    assert!(!is_valid_session_id("..\\..\\windows\\system32"));
    // 长度对但含 `/`：dash 位置错或非 hex 字符都不通过
    assert!(!is_valid_session_id("550e8400-e29b-41d4-a716-44665544/000"));
    assert!(!is_valid_session_id("550e8400_e29b_41d4_a716_446655440000")); // 用 _ 代 -
                                                                           // 非 hex 字符
    assert!(!is_valid_session_id("550e8400-e29b-41d4-a716-44665544000g"));
    // 长度不对（35 / 37）
    assert!(!is_valid_session_id("550e8400-e29b-41d4-a716-44665544000"));
    assert!(!is_valid_session_id(
        "550e8400-e29b-41d4-a716-4466554400000"
    ));
    // NUL 字节
    assert!(!is_valid_session_id(
        "550e8400-e29b-41d4-a716-44665544\x00000"
    ));
    // 百分号编码与绝对路径
    assert!(!is_valid_session_id("%2e%2e/recordings/x"));
    assert!(!is_valid_session_id("/Users/attacker/secret.wav"));
}

#[test]
fn is_valid_local_pack_id_accepts_realistic_ids() {
    assert!(is_valid_local_pack_id("builtin.light"));
    assert!(is_valid_local_pack_id("builtin.structured"));
    assert!(is_valid_local_pack_id("custom.meeting"));
    assert!(is_valid_local_pack_id(
        "550e8400-e29b-41d4-a716-446655440000"
    ));
    assert!(is_valid_local_pack_id("my_pack_v2"));
    assert!(is_valid_local_pack_id("Pack-2026.05"));
}

#[test]
fn is_valid_local_pack_id_rejects_path_traversal() {
    assert!(!is_valid_local_pack_id(""));
    assert!(!is_valid_local_pack_id("../etc/passwd"));
    assert!(!is_valid_local_pack_id("..\\windows\\system32"));
    assert!(!is_valid_local_pack_id("pack/../../etc"));
    assert!(!is_valid_local_pack_id("/abs/path"));
    assert!(!is_valid_local_pack_id("with space"));
    assert!(!is_valid_local_pack_id("with\x00null"));
    assert!(!is_valid_local_pack_id(&"a".repeat(129)));
}
