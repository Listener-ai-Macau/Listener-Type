//! Device-domain helpers for Listener Type IPC.
//! Parent module: `crate::commands::device`.

use super::super::{
    emit_prefs_changed, persist_settings, settings_update_lock, CoordinatorState,
};
use super::ble::{
    embedded_ble_windows_pairing_result, EmbeddedBleWindowsPairingPromptPolicy,
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::coordinator::Coordinator;
use crate::types::{
    device_ble_name_is_valid, DeviceCustomKeyAction, DeviceKnobRotationAction, DictationInputSource,
    UserPreferences, DEFAULT_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES,
    DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES, DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
    DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES, MAX_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES,
    MAX_DEVICE_LOW_POWER_IDLE_MINUTES,
};

pub fn device_firmware_settings_changed(previous: &UserPreferences, next: &UserPreferences) -> bool {
    previous.device_status_led_brightness_percent != next.device_status_led_brightness_percent
        || previous.device_key_led_brightness_percent != next.device_key_led_brightness_percent
        || previous.device_knob_led_brightness_percent != next.device_knob_led_brightness_percent
        || previous.device_edge_led_brightness_percent != next.device_edge_led_brightness_percent
        || previous.device_knob_rotation_action != next.device_knob_rotation_action
        || previous.device_low_power_idle_minutes != next.device_low_power_idle_minutes
        || previous.device_plugged_low_power_idle_minutes
            != next.device_plugged_low_power_idle_minutes
        || previous.device_battery_low_power_idle_minutes
            != next.device_battery_low_power_idle_minutes
        || previous.device_plugged_low_power_enabled != next.device_plugged_low_power_enabled
        || previous.device_battery_auto_shutdown_minutes
            != next.device_battery_auto_shutdown_minutes
        || device_ec11_fast_recording_enabled(previous) != device_ec11_fast_recording_enabled(next)
        || previous.device_ble_name != next.device_ble_name
}

pub fn validate_device_firmware_preferences(prefs: &UserPreferences) -> Result<(), String> {
    if prefs.device_status_led_brightness_percent > 100
        || prefs.device_key_led_brightness_percent > 100
        || prefs.device_knob_led_brightness_percent > 100
        || prefs.device_edge_led_brightness_percent > 100
    {
        return Err("设备灯光亮度必须在 0-100 之间。".to_string());
    }
    if prefs.device_low_power_idle_minutes > MAX_DEVICE_LOW_POWER_IDLE_MINUTES
        || prefs.device_plugged_low_power_idle_minutes > MAX_DEVICE_LOW_POWER_IDLE_MINUTES
        || prefs.device_battery_low_power_idle_minutes > MAX_DEVICE_LOW_POWER_IDLE_MINUTES
    {
        return Err(format!(
            "低功耗等待时间必须在 0-{} 分钟之间。",
            MAX_DEVICE_LOW_POWER_IDLE_MINUTES
        ));
    }
    if prefs.device_battery_auto_shutdown_minutes > MAX_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES {
        return Err(format!(
            "电池自动关机时间必须在 0-{} 分钟之间。",
            MAX_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES
        ));
    }
    if !device_ble_name_is_valid(&prefs.device_ble_name) {
        return Err(
            "蓝牙名称只支持 1-29 个可见 ASCII 字符，不能包含空格、引号、分号、等号或反斜杠。"
                .to_string(),
        );
    }
    Ok(())
}

pub fn firmware_mode_for_device_knob_rotation_action(action: DeviceKnobRotationAction) -> &'static str {
    match action {
        DeviceKnobRotationAction::SystemVolume => "system_volume",
        DeviceKnobRotationAction::ScreenBrightness => "screen_brightness",
        DeviceKnobRotationAction::Disabled => "disabled",
    }
}

pub fn device_ec11_fast_recording_enabled(prefs: &UserPreferences) -> bool {
    prefs.dictation_input_source == DictationInputSource::EmbeddedBle
        && prefs.device_custom_keys.knob.action == DeviceCustomKeyAction::Dictation
}

pub struct DeviceSettingPacket {
    pub id: &'static str,
    pub command: String,
}

pub fn sync_device_setting_packet_to_firmware(packet: DeviceSettingPacket) -> Result<(), String> {
    crate::embedded_ble::send_device_settings_command(&packet.command, Duration::from_secs(2))
        .map_err(|err| format!("设备设置写入固件失败（{}）：{err}", packet.id))
}

pub fn sync_device_ble_name_apply_to_firmware() -> Result<(), String> {
    crate::embedded_ble::apply_pending_ble_name(Duration::from_secs(2))
        .map(|_| ())
        .map_err(|err| format!("蓝牙名称应用到固件失败：{err}"))
}

pub fn pack_device_setting_assignments(assignments: Vec<String>) -> Result<Vec<String>, String> {
    let mut commands = Vec::new();
    let mut current: Option<String> = None;

    for assignment in assignments {
        let candidate = match current.as_deref() {
            Some(command) => format!("{command} {assignment}"),
            None => format!("DEVICE:SET {assignment}"),
        };
        if candidate.as_bytes().len() + 1 <= DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES {
            current = Some(candidate);
            continue;
        }

        if let Some(command) = current.take() {
            commands.push(command);
        }

        let command = format!("DEVICE:SET {assignment}");
        let bytes_with_newline = command.as_bytes().len() + 1;
        if bytes_with_newline > DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES {
            return Err(format!(
                "Device settings command is too long for BLE audio control: {bytes_with_newline} bytes."
            ));
        }
        current = Some(command);
    }

    if let Some(command) = current {
        commands.push(command);
    }
    Ok(commands)
}

pub fn compact_device_setting_key(key: &'static str) -> &'static str {
    match key {
        "led_status" => "ls",
        "led_key" => "lk",
        "led_ec11" => "l11",
        "led_edge" => "le",
        "plugged_low_power_idle_minutes" => "plm",
        "battery_low_power_idle_minutes" => "blm",
        "plugged_low_power_enabled" => "ple",
        "plugged_auto_shutdown_minutes" => "pam",
        "battery_auto_shutdown_minutes" => "bam",
        _ => key,
    }
}

pub fn device_setting_assignment(
    key: &'static str,
    value: impl std::fmt::Display,
    compact_set_supported: bool,
) -> String {
    let key = if compact_set_supported {
        compact_device_setting_key(key)
    } else {
        key
    };
    format!("{key}={value}")
}

pub fn device_setting_packets_for_changes(
    previous: &UserPreferences,
    next: &UserPreferences,
) -> Vec<DeviceSettingPacket> {
    let mut packets = Vec::new();
    if previous.device_status_led_brightness_percent != next.device_status_led_brightness_percent
        || previous.device_key_led_brightness_percent != next.device_key_led_brightness_percent
    {
        packets.push(DeviceSettingPacket {
            id: "status_key_led_brightness",
            command: format!(
                "DEVICE:SET led_status={} led_key={}",
                next.device_status_led_brightness_percent, next.device_key_led_brightness_percent
            ),
        });
    }
    if previous.device_knob_led_brightness_percent != next.device_knob_led_brightness_percent
        || previous.device_edge_led_brightness_percent != next.device_edge_led_brightness_percent
    {
        packets.push(DeviceSettingPacket {
            id: "knob_edge_led_brightness",
            command: format!(
                "DEVICE:SET led_ec11={} led_edge={}",
                next.device_knob_led_brightness_percent, next.device_edge_led_brightness_percent
            ),
        });
    }
    if previous.device_knob_rotation_action != next.device_knob_rotation_action {
        let mode = firmware_mode_for_device_knob_rotation_action(next.device_knob_rotation_action);
        packets.push(DeviceSettingPacket {
            id: "knob_rotation",
            command: format!("DEVICE:SET knob_rotation={mode}"),
        });
    }
    if device_ec11_fast_recording_enabled(previous) != device_ec11_fast_recording_enabled(next) {
        packets.push(DeviceSettingPacket {
            id: "ec11_fast_recording",
            command: format!(
                "DEVICE:SET e11r={}",
                if device_ec11_fast_recording_enabled(next) {
                    1
                } else {
                    0
                }
            ),
        });
    }
    if previous.device_plugged_low_power_idle_minutes != next.device_plugged_low_power_idle_minutes
    {
        packets.push(DeviceSettingPacket {
            id: "plugged_low_power_idle_minutes",
            command: format!(
                "DEVICE:SET plugged_low_power_idle_minutes={}",
                next.device_plugged_low_power_idle_minutes
            ),
        });
    }
    if previous.device_battery_low_power_idle_minutes != next.device_battery_low_power_idle_minutes
        || previous.device_low_power_idle_minutes != next.device_low_power_idle_minutes
    {
        packets.push(DeviceSettingPacket {
            id: "battery_low_power_idle_minutes",
            command: format!(
                "DEVICE:SET battery_low_power_idle_minutes={}",
                next.device_battery_low_power_idle_minutes
            ),
        });
    }
    if previous.device_plugged_low_power_enabled != next.device_plugged_low_power_enabled {
        packets.push(DeviceSettingPacket {
            id: "plugged_low_power_enabled",
            command: format!(
                "DEVICE:SET plugged_low_power_enabled={}",
                if next.device_plugged_low_power_enabled {
                    1
                } else {
                    0
                }
            ),
        });
    }
    if previous.device_battery_auto_shutdown_minutes != next.device_battery_auto_shutdown_minutes {
        packets.push(DeviceSettingPacket {
            id: "auto_shutdown_minutes",
            command: format!(
                "DEVICE:SET auto_shutdown_minutes={}",
                next.device_battery_auto_shutdown_minutes
            ),
        });
    }
    if previous.device_ble_name != next.device_ble_name {
        packets.push(DeviceSettingPacket {
            id: "ble_name",
            command: format!("DEVICE:SET ble_name={}", next.device_ble_name),
        });
    }
    packets
}

pub fn sync_device_firmware_preferences(
    previous: &UserPreferences,
    next: &UserPreferences,
) -> Result<(), String> {
    let ble_name_changed = previous.device_ble_name != next.device_ble_name;
    let packets = device_setting_packets_for_changes(previous, next);
    match crate::device_control_platform::BleDeviceSettingsTransaction::begin(Duration::from_secs(
        2,
    )) {
        Ok(mut transaction) => {
            for packet in &packets {
                transaction.write_setting(&packet.id, &packet.command, Duration::from_secs(2))?;
            }
            if ble_name_changed {
                transaction.invoke_command("apply_ble_name", || {
                    sync_device_ble_name_apply_to_firmware()
                })?;
            }
            return Ok(());
        }
        Err(err) => {
            // Older firmware and an active USB-only setup retain the existing
            // settings path; a BLE transaction never silently degrades after it starts.
            log::info!(
                "[device-control] BLE settings transaction unavailable; preserving fallback: {err}"
            );
        }
    }
    for packet in packets {
        sync_device_setting_packet_to_firmware(packet)?;
    }
    if ble_name_changed {
        sync_device_ble_name_apply_to_firmware()?;
    }
    Ok(())
}

pub async fn refresh_device_settings_status(
) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::embedded_ble::read_device_settings_status(Duration::from_secs(4))
    })
    .await
    .map_err(|err| format!("device settings refresh task failed: {err}"))?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettingsSnapshot {
    pub schema: &'static str,
    pub connected: bool,
    pub write_supported: bool,
    pub source: &'static str,
    pub status_led_brightness_percent: u8,
    pub key_led_brightness_percent: u8,
    pub knob_led_brightness_percent: u8,
    pub edge_led_brightness_percent: u8,
    pub led_zone_brightness_supported: bool,
    pub compact_set_supported: bool,
    pub low_power_idle_minutes: u32,
    pub plugged_low_power_idle_minutes: u32,
    pub battery_low_power_idle_minutes: u32,
    pub plugged_low_power_enabled: bool,
    pub voice_auto_start_enabled: bool,
    pub voice_auto_stop_enabled: bool,
    pub plugged_auto_shutdown_ms: u32,
    pub battery_auto_shutdown_ms: u32,
    pub knob_rotation_action: String,
    pub ble_name: String,
    pub ble_name_pending_restart: bool,
    pub active_power_source: &'static str,
    pub battery_percent: Option<u8>,
    pub detail: Option<String>,
    pub last_updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettingsUpdateRequest {
    pub status_led_brightness_percent: u8,
    pub key_led_brightness_percent: u8,
    pub knob_led_brightness_percent: u8,
    pub edge_led_brightness_percent: u8,
    pub plugged_low_power_idle_minutes: u32,
    pub battery_low_power_idle_minutes: u32,
    pub plugged_low_power_enabled: bool,
    pub voice_auto_start_enabled: bool,
    pub voice_auto_stop_enabled: bool,
    pub plugged_auto_shutdown_minutes: u32,
    pub battery_auto_shutdown_minutes: u32,
    pub ble_name: String,
}

pub const DEVICE_SETTINGS_SCHEMA: &str = "listener.device_settings.v1";
pub const DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS: u32 = 0;
pub const DEVICE_SETTINGS_DEFAULT_BATTERY_AUTO_SHUTDOWN_MS: u32 =
    DEFAULT_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES * 60 * 1000;
pub const DEVICE_SETTINGS_MIN_AUTO_SHUTDOWN_MINUTES: u32 = 0;
pub const DEVICE_SETTINGS_MAX_AUTO_SHUTDOWN_MINUTES: u32 = 1440;
pub const DEVICE_SETTINGS_DEFAULT_BLE_NAME: &str = "listener";
pub const DEVICE_SETTINGS_BLE_WRITE_TIMEOUT: Duration = Duration::from_secs(4);
pub const DEVICE_SETTINGS_BLE_NAME_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEVICE_SETTINGS_BLE_TASK_TIMEOUT: Duration = Duration::from_secs(20);
pub const DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_INITIAL_DELAY: Duration = Duration::from_millis(300);
pub const DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_RETRY_DELAY: Duration = Duration::from_millis(250);
pub const DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_READBACK_ATTEMPTS: u8 = 3;
pub const DEVICE_SETTINGS_BLE_NAME_APPLY_ACK_LOST_SETTLE_DELAY: Duration = Duration::from_millis(700);
pub const DEVICE_SETTINGS_BLE_NAME_APPLY_ACK_LOST_READBACK_ATTEMPTS: u8 = 3;
pub const DEVICE_SETTINGS_BLE_NAME_CACHE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(45);
pub const DEVICE_SETTINGS_BLE_RECOVERY_PAIRING_SETTLE_DELAY: Duration = Duration::from_millis(2200);
pub const DEVICE_SETTINGS_BLE_RECOVERY_PAIRING_STACK_SETTLE_DELAY: Duration =
    Duration::from_millis(180);
pub const DEVICE_SETTINGS_BLE_NAME_PAIRING_SETTLE_DELAY: Duration = Duration::from_millis(1200);
pub const DEVICE_SETTINGS_BLE_NAME_PAIRING_RETRY_DELAY: Duration = Duration::from_millis(2200);
pub const DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES: usize = 63;
pub const DEVICE_SETTINGS_READBACK_VERIFY_ATTEMPTS: u8 = 5;
pub const DEVICE_SETTINGS_READBACK_VERIFY_RETRY_DELAY: Duration = Duration::from_millis(220);

pub async fn read_device_settings_snapshot_from_firmware() -> Result<DeviceSettingsSnapshot, String> {
    let started = Instant::now();
    let status = run_device_settings_blocking("readback", || {
        crate::embedded_ble::read_device_settings_status(DEVICE_SETTINGS_BLE_WRITE_TIMEOUT)
    })
    .await?;
    log::info!(
        "[device-settings] readback succeeded elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(device_settings_snapshot_from_status(status))
}

pub async fn run_device_settings_blocking<T, F>(label: &'static str, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let joined = tokio::time::timeout(
        DEVICE_SETTINGS_BLE_TASK_TIMEOUT,
        tauri::async_runtime::spawn_blocking(task),
    )
    .await
    .map_err(|_| {
        format!(
            "Listener device settings {label} timed out after {} ms",
            DEVICE_SETTINGS_BLE_TASK_TIMEOUT.as_millis()
        )
    })?;
    joined.map_err(|err| format!("Listener device settings {label} task failed: {err}"))?
}

pub fn device_ble_name_apply_confirmed_by_status(
    status: &crate::embedded_ble::DeviceSettingsStatus,
    expected_ble_name: &str,
) -> bool {
    status.ble_name == expected_ble_name && !status.ble_name_pending_restart
}

pub async fn read_device_ble_name_apply_confirmation(
    expected_ble_name: &str,
) -> Result<DeviceSettingsSnapshot, String> {
    let mut last_snapshot = None;
    let mut readback_errors = Vec::new();
    for attempt in 1..=DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_READBACK_ATTEMPTS {
        let delay = if attempt == 1 {
            DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_INITIAL_DELAY
        } else {
            DEVICE_SETTINGS_BLE_NAME_APPLY_CONFIRM_RETRY_DELAY
        };
        tokio::time::sleep(delay).await;
        match read_device_settings_snapshot_from_firmware().await {
            Ok(snapshot)
                if snapshot.ble_name == expected_ble_name && !snapshot.ble_name_pending_restart =>
            {
                log::info!(
                    "[device-settings] BLE name apply confirmation readback matched name={expected_ble_name:?} attempt={attempt}"
                );
                return Ok(snapshot);
            }
            Ok(snapshot) => {
                log::info!(
                    "[device-settings] BLE name apply confirmation still pending name={:?} expected={expected_ble_name:?} pending={} attempt={attempt}",
                    snapshot.ble_name,
                    snapshot.ble_name_pending_restart,
                );
                last_snapshot = Some(snapshot);
            }
            Err(error) => {
                readback_errors.push(format!("attempt {attempt}: {error}"));
            }
        }
    }
    if let Some(snapshot) = last_snapshot {
        return Ok(snapshot);
    }
    Err(format!(
        "BLE name apply confirmation readback failed: {}",
        readback_errors.join("; ")
    ))
}

pub fn device_ble_name_apply_error_allows_deferred_confirmation(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("0x800704c7")
        || lower.contains("write async canceled")
        || lower.contains("write async cancelled")
        || lower.contains("gattcommunicationstatus(1)")
        || lower.contains("operation canceled")
        || lower.contains("operation cancelled")
        || (lower.contains("device settings") && lower.contains("timed out"))
}

pub fn apply_pending_ble_name_confirmed(
    expected_ble_name: &str,
    timeout: Duration,
) -> Result<crate::embedded_ble::DeviceSettingsCommandTransport, String> {
    match crate::embedded_ble::apply_pending_ble_name(timeout) {
        Ok(transport) => return Ok(transport),
        Err(apply_error) => {
            log::warn!(
                "[device-settings] BLE name apply ACK failed; checking firmware readback before failing: {apply_error}"
            );
            std::thread::sleep(DEVICE_SETTINGS_BLE_NAME_APPLY_ACK_LOST_SETTLE_DELAY);
            let mut readback_errors = Vec::new();
            let readback_timeout = timeout.max(Duration::from_secs(8));
            for attempt in 1..=DEVICE_SETTINGS_BLE_NAME_APPLY_ACK_LOST_READBACK_ATTEMPTS {
                match crate::embedded_ble::read_device_settings_status(readback_timeout) {
                    Ok(status)
                        if device_ble_name_apply_confirmed_by_status(
                            &status,
                            expected_ble_name,
                        ) =>
                    {
                        log::warn!(
                            "[device-settings] BLE name apply ACK was lost, but firmware readback confirms name={expected_ble_name} pending=0 attempt={attempt}"
                        );
                        return Ok(
                            crate::embedded_ble::DeviceSettingsCommandTransport::StatusReadback,
                        );
                    }
                    Ok(status) => {
                        return Err(format!(
                            "{apply_error}; firmware readback after apply reports ble_name={} pending={}",
                            status.ble_name, status.ble_name_pending_restart
                        ));
                    }
                    Err(readback_error) => {
                        readback_errors.push(format!("attempt {attempt}: {readback_error}"));
                        std::thread::sleep(DEVICE_SETTINGS_BLE_NAME_APPLY_ACK_LOST_SETTLE_DELAY);
                    }
                }
            }
            if device_ble_name_apply_error_allows_deferred_confirmation(&apply_error) {
                log::warn!(
                    "[device-settings] BLE name apply lost ACK and readback is unavailable, but error is BLE-restart transient; deferring confirmation to Windows pairing/final readback name={expected_ble_name} errors={}",
                    readback_errors.join("; ")
                );
                return Ok(crate::embedded_ble::DeviceSettingsCommandTransport::StatusReadback);
            }
            Err(format!(
                "{apply_error}; confirmation readback after lost ACK failed: {}",
                readback_errors.join("; ")
            ))
        }
    }
}

pub async fn get_device_settings(
    coord: CoordinatorState<'_>,
    app: AppHandle,
) -> Result<DeviceSettingsSnapshot, String> {
    match read_device_settings_snapshot_from_firmware().await {
        Ok(snapshot) => {
            sync_device_settings_snapshot_to_preferences(&coord, &app, &snapshot)?;
            return Ok(snapshot);
        }
        Err(readback_error) => {
            let device = tauri::async_runtime::spawn_blocking(
                crate::embedded_ble::firmware_ota_device_snapshot,
            )
            .await
            .map_err(|err| format!("Listener BLE device settings probe task failed: {err}"))?;
            let mut snapshot = device_settings_snapshot_from_device(device);
            if snapshot.connected {
                snapshot.detail =
                    Some(device_settings_readback_unavailable_detail(&readback_error));
            }
            Ok(snapshot)
        }
    }
}

pub fn sync_device_settings_snapshot_to_preferences(
    coord: &Coordinator,
    app: &AppHandle,
    snapshot: &DeviceSettingsSnapshot,
) -> Result<(), String> {
    let _settings_guard = settings_update_lock().lock();
    let mut prefs = coord.prefs().get();
    if !apply_device_settings_snapshot_to_preferences(&mut prefs, snapshot) {
        return Ok(());
    }
    coord
        .prefs()
        .set(prefs.clone())
        .map_err(|err| err.to_string())?;
    if !snapshot.ble_name_pending_restart {
        crate::embedded_ble::set_configured_bluetooth_target_name(&prefs.device_ble_name);
    }
    emit_prefs_changed(app, &prefs);
    Ok(())
}

pub fn apply_device_settings_snapshot_to_preferences(
    prefs: &mut UserPreferences,
    snapshot: &DeviceSettingsSnapshot,
) -> bool {
    let mut changed = false;

    macro_rules! update_field {
        ($field:ident, $value:expr) => {{
            let value = $value;
            if prefs.$field != value {
                prefs.$field = value;
                changed = true;
            }
        }};
    }

    if snapshot.led_zone_brightness_supported {
        update_field!(
            device_status_led_brightness_percent,
            snapshot.status_led_brightness_percent
        );
        update_field!(
            device_key_led_brightness_percent,
            snapshot.key_led_brightness_percent
        );
        update_field!(
            device_knob_led_brightness_percent,
            snapshot.knob_led_brightness_percent
        );
        update_field!(
            device_edge_led_brightness_percent,
            snapshot.edge_led_brightness_percent
        );
    }
    update_field!(
        device_knob_rotation_action,
        device_knob_rotation_action_from_snapshot(snapshot)
    );
    update_field!(
        device_low_power_idle_minutes,
        snapshot.battery_low_power_idle_minutes
    );
    update_field!(
        device_plugged_low_power_idle_minutes,
        snapshot.plugged_low_power_idle_minutes
    );
    update_field!(
        device_battery_low_power_idle_minutes,
        snapshot.battery_low_power_idle_minutes
    );
    update_field!(
        device_plugged_low_power_enabled,
        snapshot.plugged_low_power_enabled
    );
    update_field!(
        device_battery_auto_shutdown_minutes,
        device_settings_minutes_from_ms_floor(
            snapshot.battery_auto_shutdown_ms,
            MAX_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES,
        )
    );
    if !snapshot.ble_name_pending_restart && device_ble_name_is_valid(&snapshot.ble_name) {
        update_field!(device_ble_name, snapshot.ble_name.clone());
    }

    changed
}

pub fn device_settings_minutes_from_ms_floor(ms: u32, max_minutes: u32) -> u32 {
    (ms / 60_000).min(max_minutes)
}

pub fn device_knob_rotation_action_from_snapshot(
    snapshot: &DeviceSettingsSnapshot,
) -> DeviceKnobRotationAction {
    match snapshot.knob_rotation_action.as_str() {
        "screen_brightness" | "screenBrightness" => DeviceKnobRotationAction::ScreenBrightness,
        "disabled" => DeviceKnobRotationAction::Disabled,
        _ => DeviceKnobRotationAction::SystemVolume,
    }
}

pub fn device_settings_readback_unavailable_detail(error: &str) -> String {
    let mut chars = error.trim().chars();
    let mut summary: String = chars.by_ref().take(180).collect();
    if chars.next().is_some() {
        summary.push_str("...");
    }
    if summary.is_empty() {
        summary = "unknown error".to_string();
    }
    format!(
        "Firmware DEVICE readback is unavailable ({summary}); showing defaults. Writes still use DEVICE:SET."
    )
}

pub fn device_settings_sent_but_readback_unavailable_detail(error: &str) -> String {
    let mut chars = error.trim().chars();
    let mut summary: String = chars.by_ref().take(180).collect();
    if chars.next().is_some() {
        summary.push_str("...");
    }
    if summary.is_empty() {
        summary = "unknown error".to_string();
    }
    format!(
        "Device settings were sent, but firmware readback is unavailable ({summary}); displayed values are last-known."
    )
}

pub fn device_settings_readback_unavailable_allowed_after_write(
    ble_name_changed: bool,
    ble_name_apply_needed: bool,
) -> bool {
    ble_name_changed || ble_name_apply_needed
}

#[derive(Debug, Clone)]
pub struct DeviceBleNameWindowsRefreshOutcome {
    pub recovery_error: Option<String>,
    pub unpair_result: crate::embedded_ble::BleDeviceUnpairResult,
    pub pairing_prompt_result: crate::embedded_ble::BleDevicePairingPromptResult,
}

pub fn push_unique_device_ble_name(names: &mut Vec<String>, name: &str) {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return;
    }
    if names
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(trimmed))
    {
        return;
    }
    names.push(trimmed.to_string());
}

pub fn device_ble_name_windows_refresh_target_names(
    request: &DeviceSettingsUpdateRequest,
    previous_snapshot: Option<&DeviceSettingsSnapshot>,
    previous_prefs_ble_name: Option<&str>,
) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = previous_prefs_ble_name {
        push_unique_device_ble_name(&mut names, name);
    }
    if let Some(snapshot) = previous_snapshot {
        push_unique_device_ble_name(&mut names, &snapshot.ble_name);
    }
    push_unique_device_ble_name(&mut names, &request.ble_name);
    names
}

pub fn wait_for_device_ble_name_recovery_pairing_ready(expected_ble_name: &str) -> Option<u64> {
    let started = Instant::now();
    std::thread::sleep(DEVICE_SETTINGS_BLE_RECOVERY_PAIRING_STACK_SETTLE_DELAY);
    let remaining =
        DEVICE_SETTINGS_BLE_RECOVERY_PAIRING_SETTLE_DELAY.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return None;
    }

    match crate::embedded_ble::wait_for_bluetooth_target_advertisement_by_name(
        expected_ble_name,
        remaining,
        "BLE name recovery pairing ready",
    ) {
        Ok(address) => {
            log::info!(
                "[device-settings] BLE name recovery pairing-ready advertisement seen name={expected_ble_name:?} address={address:012X} elapsed_ms={}",
                started.elapsed().as_millis()
            );
            Some(address)
        }
        Err(err) => {
            let elapsed = started.elapsed();
            let fallback_sleep =
                DEVICE_SETTINGS_BLE_RECOVERY_PAIRING_SETTLE_DELAY.saturating_sub(elapsed);
            log::warn!(
                "[device-settings] BLE name recovery pairing-ready advertisement not confirmed before cache cleanup: {err}; preserving settle fallback remaining_ms={}",
                fallback_sleep.as_millis()
            );
            if !fallback_sleep.is_zero() {
                std::thread::sleep(fallback_sleep);
            }
            None
        }
    }
}

pub fn apply_device_ble_name_windows_refresh_blocking(
    expected_ble_name: String,
    cleanup_target_names: Vec<String>,
    firmware_name_confirmed: bool,
) -> DeviceBleNameWindowsRefreshOutcome {
    let recovery_error = match crate::embedded_ble::send_recording_control_silent_recovery(
        DEVICE_SETTINGS_BLE_WRITE_TIMEOUT,
    ) {
        Ok(()) => None,
        Err(err) => {
            log::warn!("[device-settings] BLE name pairing recovery command failed: {err}");
            Some(err)
        }
    };

    let verified_handoff_address = recovery_error
        .is_none()
        .then(|| {
            firmware_name_confirmed
                .then(|| crate::embedded_ble::verified_bluetooth_target_rename_handoff_address())
                .flatten()
        })
        .flatten();
    let early_unpair_result = verified_handoff_address.map(|address| {
        log::info!(
            "[device-settings] BLE name Windows cache cleanup starting from firmware-confirmed handoff address before recovery advertisement confirmation address={address:012X}"
        );
        crate::embedded_ble::unpair_listener_devices_for_known_addresses(
            &cleanup_target_names,
            &[address],
        )
    });
    let observed_recovery_addresses = if recovery_error.is_none() {
        let advertised_address =
            wait_for_device_ble_name_recovery_pairing_ready(&expected_ble_name);
        let pairing_address = advertised_address.or(verified_handoff_address);
        if advertised_address.is_none() {
            if let Some(address) = verified_handoff_address {
                log::warn!(
                    "[device-settings] BLE name recovery advertisement was not observed; falling back to the firmware-confirmed handoff address={address:012X}"
                );
            }
        }
        if pairing_address.is_some() && firmware_name_confirmed {
            log::info!(
                "[device-settings] BLE name recovery has a pairing address for the applied name={expected_ble_name:?}"
            );
        }
        pairing_address.into_iter().collect::<Vec<_>>()
    } else {
        std::thread::sleep(DEVICE_SETTINGS_BLE_NAME_PAIRING_SETTLE_DELAY);
        Vec::new()
    };
    log::info!(
        "[device-settings] BLE name Windows cache refresh settle complete recovery_error={}",
        recovery_error.as_deref().unwrap_or("none")
    );
    let unpair_result = if let Some(fast_result) = early_unpair_result {
        if matches!(
            fast_result.status,
            crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction
                | crate::embedded_ble::BleDeviceUnpairStatus::NotFound
        ) {
            log::warn!(
                "[device-settings] early recovery-address BLE cache cleanup status={:?}; falling back to complete Windows cache cleanup",
                fast_result.status
            );
            crate::embedded_ble::unpair_listener_devices_for_names(&cleanup_target_names)
        } else {
            fast_result
        }
    } else if observed_recovery_addresses.is_empty() {
        crate::embedded_ble::unpair_listener_devices_for_names(&cleanup_target_names)
    } else {
        log::info!(
            "[device-settings] BLE name Windows cache cleanup using firmware-confirmed recovery address(es)={observed_recovery_addresses:?}"
        );
        let fast_result = crate::embedded_ble::unpair_listener_devices_for_known_addresses(
            &cleanup_target_names,
            observed_recovery_addresses.as_slice(),
        );
        if matches!(
            fast_result.status,
            crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction
                | crate::embedded_ble::BleDeviceUnpairStatus::NotFound
        ) {
            log::warn!(
                "[device-settings] recovery-address BLE cache cleanup status={:?}; falling back to complete Windows cache cleanup",
                fast_result.status
            );
            crate::embedded_ble::unpair_listener_devices_for_names(&cleanup_target_names)
        } else {
            fast_result
        }
    };
    log::info!(
        "[device-settings] BLE name Windows cache cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={}",
        unpair_result.status,
        unpair_result.matched_devices,
        unpair_result.unpaired_devices,
        unpair_result.already_unpaired_devices,
        unpair_result.failed_devices,
        unpair_result.needs_user_action,
    );
    let mut pairing_prompt_result = embedded_ble_windows_pairing_result(
        "device BLE name change",
        expected_ble_name.as_str(),
        recovery_error.is_none(),
        EmbeddedBleWindowsPairingPromptPolicy::SuppressUserPrompt,
        false,
        observed_recovery_addresses.as_slice(),
    );
    if recovery_error.is_none()
        && device_ble_name_windows_refresh_pairing_retry_needed(&pairing_prompt_result)
    {
        log::warn!(
            "[device-settings] BLE name Windows PairAsync did not reach a user confirmation or paired state; retrying once after {} ms",
            DEVICE_SETTINGS_BLE_NAME_PAIRING_RETRY_DELAY.as_millis()
        );
        std::thread::sleep(DEVICE_SETTINGS_BLE_NAME_PAIRING_RETRY_DELAY);
        let retry_pairing_prompt_result = embedded_ble_windows_pairing_result(
            "device BLE name change retry",
            expected_ble_name.as_str(),
            true,
            EmbeddedBleWindowsPairingPromptPolicy::SuppressUserPrompt,
            false,
            observed_recovery_addresses.as_slice(),
        );
        if device_ble_name_pairing_retry_result_is_better(
            &retry_pairing_prompt_result,
            &pairing_prompt_result,
        ) {
            pairing_prompt_result = retry_pairing_prompt_result;
        } else {
            pairing_prompt_result.details.push(format!(
                "Retry PairAsync result status={:?} matched={} prompted={} already_paired={} failed={} open_settings={}",
                retry_pairing_prompt_result.status,
                retry_pairing_prompt_result.matched_devices,
                retry_pairing_prompt_result.prompted_devices,
                retry_pairing_prompt_result.already_paired_devices,
                retry_pairing_prompt_result.failed_devices,
                retry_pairing_prompt_result.open_bluetooth_settings,
            ));
            pairing_prompt_result
                .details
                .extend(retry_pairing_prompt_result.details);
        }
    }
    DeviceBleNameWindowsRefreshOutcome {
        recovery_error,
        unpair_result,
        pairing_prompt_result,
    }
}

pub fn device_ble_name_windows_refresh_detail(outcome: &DeviceBleNameWindowsRefreshOutcome) -> String {
    let cleanup_detail = match outcome.unpair_result.status {
        crate::embedded_ble::BleDeviceUnpairStatus::Removed => {
            "Old Windows Bluetooth entries were removed."
        }
        crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean => {
            "Windows Bluetooth entries were already clean."
        }
        crate::embedded_ble::BleDeviceUnpairStatus::NotFound => {
            "No old Windows Bluetooth entry was found."
        }
        crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction => {
            "Windows did not allow every old Bluetooth entry to be removed automatically."
        }
    };
    let pairing_detail = match outcome.pairing_prompt_result.status {
        crate::embedded_ble::BleDevicePairingPromptStatus::Paired => {
            "Windows pairing completed for the new name."
        }
        crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired => {
            "Windows already has the new Listener name paired."
        }
        crate::embedded_ble::BleDevicePairingPromptStatus::NotFound => {
            "Windows did not see the new pairable Listener name yet."
        }
        crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction => {
            "Windows still needs Bluetooth pairing confirmation."
        }
    };
    if outcome.recovery_error.is_some() {
        format!(
            "BLE name was applied; Windows cache refresh could not fully prepare the pairing window. {cleanup_detail} {pairing_detail}"
        )
    } else {
        format!(
            "BLE name was applied and Windows Bluetooth cache refresh ran. {cleanup_detail} {pairing_detail}"
        )
    }
}

pub fn device_ble_name_windows_refresh_confirmed(outcome: &DeviceBleNameWindowsRefreshOutcome) -> bool {
    matches!(
        outcome.pairing_prompt_result.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::Paired
            | crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
    ) && !outcome.pairing_prompt_result.open_bluetooth_settings
        && outcome.pairing_prompt_result.failed_devices == 0
}

pub fn device_ble_name_windows_refresh_pairing_retry_needed(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    !matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::Paired
            | crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
    ) && pairing.attempted
        && pairing.matched_devices > 0
        && pairing.prompted_devices == 0
        && pairing.already_paired_devices == 0
        && pairing.failed_devices > 0
}

pub fn device_ble_name_pairing_retry_result_is_better(
    retry: &crate::embedded_ble::BleDevicePairingPromptResult,
    previous: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    matches!(
        retry.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::Paired
            | crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
    ) || retry.prompted_devices > previous.prompted_devices
        || retry.already_paired_devices > previous.already_paired_devices
        || (retry.failed_devices < previous.failed_devices && retry.matched_devices > 0)
}

pub fn device_ble_name_windows_refresh_should_release_hold_for_followup(
    outcome: &DeviceBleNameWindowsRefreshOutcome,
) -> bool {
    outcome.recovery_error.is_none()
        && !device_ble_name_windows_refresh_confirmed(outcome)
        && (matches!(
            outcome.pairing_prompt_result.status,
            crate::embedded_ble::BleDevicePairingPromptStatus::NotFound
        ) || device_ble_name_windows_refresh_pairing_retry_needed(
            &outcome.pairing_prompt_result,
        ))
}

pub async fn refresh_windows_ble_cache_after_device_ble_name_change(
    coord: &CoordinatorState<'_>,
    expected_ble_name: String,
    cleanup_target_names: Vec<String>,
    firmware_name_confirmed: bool,
) -> Result<String, String> {
    log::info!(
        "[device-settings] BLE name changed; refreshing Windows Bluetooth cache before final readback"
    );
    let capture_stopped = coord
        .pause_embedded_ble_listener_for_ble_name_apply_handoff(
            DEVICE_SETTINGS_BLE_NAME_CACHE_CLEANUP_TIMEOUT,
        )
        .await;
    log::info!(
        "[device-settings] background Listener capture stopped before BLE name Windows cache refresh={capture_stopped}"
    );
    coord.hold_embedded_ble_listener_for_pairing_confirmation("BLE name Windows cache refresh");
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        apply_device_ble_name_windows_refresh_blocking(
            expected_ble_name,
            cleanup_target_names,
            firmware_name_confirmed,
        )
    })
    .await
    .map_err(|err| format!("Listener BLE name Windows cache refresh task failed: {err}"))?;
    if device_ble_name_windows_refresh_confirmed(&outcome) {
        coord.clear_embedded_ble_pairing_confirmation_hold("BLE name Windows cache refreshed");
    } else if device_ble_name_windows_refresh_should_release_hold_for_followup(&outcome) {
        log::warn!(
            "[device-settings] BLE name Windows cache refresh still needs pairing follow-up after Type recovery; releasing listener hold so bounded background recovery can continue"
        );
        coord.clear_embedded_ble_pairing_confirmation_hold(
            "BLE name Windows cache refresh follow-up",
        );
    } else {
        log::info!(
            "[device-settings] BLE name Windows cache refresh did not finish pairing; keeping background BLE listener held briefly to avoid connect/disconnect churn"
        );
    }
    coord.refresh_embedded_ble_listener();
    Ok(device_ble_name_windows_refresh_detail(&outcome))
}

pub fn device_settings_snapshot_has_authoritative_ble_name(snapshot: &DeviceSettingsSnapshot) -> bool {
    snapshot.source == "firmware" || snapshot.source == "lastKnown"
}

pub fn device_ble_name_changed_for_request(
    request: &DeviceSettingsUpdateRequest,
    previous_snapshot: Option<&DeviceSettingsSnapshot>,
    previous_prefs_ble_name: Option<&str>,
) -> bool {
    if let Some(snapshot) = previous_snapshot
        .filter(|snapshot| device_settings_snapshot_has_authoritative_ble_name(snapshot))
    {
        return snapshot.ble_name != request.ble_name;
    }
    previous_prefs_ble_name.is_some_and(|name| name != request.ble_name)
}

pub fn device_ble_name_apply_needed(
    request: &DeviceSettingsUpdateRequest,
    previous_snapshot: Option<&DeviceSettingsSnapshot>,
    ble_name_changed: bool,
) -> bool {
    if ble_name_changed {
        return true;
    }
    previous_snapshot
        .filter(|snapshot| device_settings_snapshot_has_authoritative_ble_name(snapshot))
        .is_some_and(|snapshot| {
            snapshot.ble_name == request.ble_name && snapshot.ble_name_pending_restart
        })
}

pub async fn set_device_settings(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    request: DeviceSettingsUpdateRequest,
) -> Result<DeviceSettingsSnapshot, String> {
    validate_device_settings_request(&request)?;
    let previous_snapshot = match read_device_settings_snapshot_from_firmware().await {
        Ok(snapshot) => Some(snapshot),
        Err(_) => {
            tauri::async_runtime::spawn_blocking(crate::embedded_ble::firmware_ota_device_snapshot)
                .await
                .ok()
                .map(device_settings_snapshot_from_device)
        }
    };
    let plugged_low_power_enabled =
        request.plugged_low_power_enabled && request.plugged_low_power_idle_minutes > 0;
    let led_zone_brightness_supported = previous_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.led_zone_brightness_supported);
    let previous_prefs_ble_name = coord.prefs().get().device_ble_name;
    let ble_name_changed = device_ble_name_changed_for_request(
        &request,
        previous_snapshot.as_ref(),
        Some(&previous_prefs_ble_name),
    );
    let ble_name_windows_refresh_target_names = device_ble_name_windows_refresh_target_names(
        &request,
        previous_snapshot.as_ref(),
        Some(&previous_prefs_ble_name),
    );
    let ble_name_apply_needed =
        device_ble_name_apply_needed(&request, previous_snapshot.as_ref(), ble_name_changed);
    let commands = device_settings_update_commands(
        &request,
        led_zone_brightness_supported,
        plugged_low_power_enabled,
        ble_name_changed,
        previous_snapshot
            .as_ref()
            .filter(|snapshot| snapshot.source == "firmware"),
    )?;
    let settings_write_started = Instant::now();
    let diff_readback_used = previous_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.source == "firmware");
    log::info!(
        "[device-settings] write plan commands={} led_zone_supported={} diff_readback_used={} ble_name_changed={} apply_needed={}",
        commands.len(),
        led_zone_brightness_supported,
        diff_readback_used,
        ble_name_changed,
        ble_name_apply_needed
    );
    if ble_name_changed || ble_name_apply_needed {
        let capture_stopped = if ble_name_changed {
            coord
                .pause_embedded_ble_listener_for_ble_name_apply_handoff(
                    DEVICE_SETTINGS_BLE_NAME_CACHE_CLEANUP_TIMEOUT,
                )
                .await
        } else {
            coord
                .pause_embedded_ble_listener_for_recovery_cleanup(
                    DEVICE_SETTINGS_BLE_NAME_CACHE_CLEANUP_TIMEOUT,
                )
                .await
        };
        log::info!(
            "[device-settings] background Listener capture stopped before BLE name settings write/apply={capture_stopped} changed={ble_name_changed} apply_needed={ble_name_apply_needed}"
        );
    }
    let command_count = commands.len();
    for (index, command) in commands.into_iter().enumerate() {
        let command_for_error = command.clone();
        let timeout = if command.contains("ble_name=") || command.contains("name=") {
            DEVICE_SETTINGS_BLE_NAME_WRITE_TIMEOUT
        } else {
            DEVICE_SETTINGS_BLE_WRITE_TIMEOUT
        };
        let command_started = Instant::now();
        if let Err(err) = run_device_settings_blocking("write", move || {
            crate::embedded_ble::send_device_settings_command(&command, timeout)
        })
        .await
        {
            if ble_name_changed || ble_name_apply_needed {
                coord.refresh_embedded_ble_listener();
            }
            return Err(format!("{err}; command={command_for_error}"));
        }
        log::info!(
            "[device-settings] write command {}/{} acknowledged elapsed_ms={} command={}",
            index + 1,
            command_count,
            command_started.elapsed().as_millis(),
            command_for_error
        );
    }
    {
        let _settings_guard = settings_update_lock().lock();
        let mut prefs = coord.prefs().get();
        if led_zone_brightness_supported {
            prefs.device_status_led_brightness_percent = request.status_led_brightness_percent;
            prefs.device_key_led_brightness_percent = request.key_led_brightness_percent;
            prefs.device_knob_led_brightness_percent = request.knob_led_brightness_percent;
            prefs.device_edge_led_brightness_percent = request.edge_led_brightness_percent;
        }
        prefs.device_low_power_idle_minutes = request.battery_low_power_idle_minutes;
        prefs.device_plugged_low_power_idle_minutes = request.plugged_low_power_idle_minutes;
        prefs.device_battery_low_power_idle_minutes = request.battery_low_power_idle_minutes;
        prefs.device_plugged_low_power_enabled = plugged_low_power_enabled;
        prefs.device_battery_auto_shutdown_minutes = request.battery_auto_shutdown_minutes;
        prefs.device_ble_name = request.ble_name.clone();
        persist_settings(&*coord, prefs.clone())?;
        emit_prefs_changed(&app, &prefs);
    }
    let mut post_apply_snapshot = None;
    let ble_name_detail = if ble_name_apply_needed {
        log::info!(
            "[device-settings] applying BLE name and refreshing Windows Bluetooth cache when changed={ble_name_changed}"
        );
        let expected_ble_name_for_apply = request.ble_name.clone();
        let apply_result = run_device_settings_blocking("apply_ble_name", move || {
            apply_pending_ble_name_confirmed(
                &expected_ble_name_for_apply,
                DEVICE_SETTINGS_BLE_NAME_WRITE_TIMEOUT,
            )
        })
        .await;
        match apply_result {
            Ok(crate::embedded_ble::DeviceSettingsCommandTransport::UsbSerial)
                if ble_name_changed =>
            {
                log::info!(
                    "[device-settings] BLE name USB apply ACK confirms firmware advertising handoff; starting Windows cache refresh before final readback"
                );
                // The USB apply acknowledgement already leaves the firmware's settings
                // endpoint available. Read it before pausing for Windows cache cleanup so
                // the post-write verification can reuse this confirmed snapshot instead of
                // paying for a second serial round-trip after pairing recovery.
                if let Ok(snapshot) = read_device_settings_snapshot_from_firmware().await {
                    if snapshot.ble_name == request.ble_name && !snapshot.ble_name_pending_restart {
                        post_apply_snapshot = Some(snapshot);
                        log::info!(
                            "[device-settings] BLE name USB apply readback confirmed before Windows cache refresh"
                        );
                    } else {
                        log::warn!(
                            "[device-settings] BLE name USB apply readback did not confirm requested name before Windows cache refresh"
                        );
                    }
                } else {
                    log::warn!(
                        "[device-settings] BLE name USB apply early readback unavailable; final readback remains required"
                    );
                }
                crate::embedded_ble::set_configured_bluetooth_target_name(&request.ble_name);
                match refresh_windows_ble_cache_after_device_ble_name_change(
                    &coord,
                    request.ble_name.clone(),
                    ble_name_windows_refresh_target_names.clone(),
                    true,
                )
                .await
                {
                    Ok(detail) => Some(detail),
                    Err(err) => {
                        log::warn!(
                            "[device-settings] BLE name Windows cache refresh failed after USB-confirmed apply: {err}"
                        );
                        coord.refresh_embedded_ble_listener();
                        Some(format!(
                            "BLE name was applied, but Windows Bluetooth cache refresh failed ({err})."
                        ))
                    }
                }
            }
            Ok(_) => match read_device_ble_name_apply_confirmation(&request.ble_name).await {
                Ok(snapshot) => {
                    let confirmed =
                        snapshot.ble_name == request.ble_name && !snapshot.ble_name_pending_restart;
                    post_apply_snapshot = Some(snapshot);
                    if confirmed {
                        crate::embedded_ble::set_configured_bluetooth_target_name(
                            &request.ble_name,
                        );
                        if ble_name_changed {
                            match refresh_windows_ble_cache_after_device_ble_name_change(
                                &coord,
                                request.ble_name.clone(),
                                ble_name_windows_refresh_target_names.clone(),
                                true,
                            )
                            .await
                            {
                                Ok(detail) => Some(detail),
                                Err(err) => {
                                    log::warn!(
                                            "[device-settings] BLE name Windows cache refresh failed: {err}"
                                        );
                                    coord.refresh_embedded_ble_listener();
                                    Some(format!(
                                            "BLE name was applied, but Windows Bluetooth cache refresh failed ({err})."
                                        ))
                                }
                            }
                        } else {
                            coord.refresh_embedded_ble_listener();
                            Some(
                                    "BLE name was applied without resetting Windows pairing because the requested name was already current."
                                        .to_string(),
                                )
                        }
                    } else {
                        crate::embedded_ble::set_configured_bluetooth_target_name(
                            &previous_prefs_ble_name,
                        );
                        coord.refresh_embedded_ble_listener();
                        Some(
                                "BLE name was saved, but firmware still reports it pending; Type kept the previous BLE target until Listener applies the new advertising name."
                                    .to_string(),
                            )
                    }
                }
                Err(readback_error) => {
                    log::warn!(
                            "[device-settings] BLE name apply sent, but confirmation readback failed: {readback_error}"
                        );
                    crate::embedded_ble::set_configured_bluetooth_target_name(&request.ble_name);
                    if ble_name_changed {
                        match refresh_windows_ble_cache_after_device_ble_name_change(
                                &coord,
                                request.ble_name.clone(),
                                ble_name_windows_refresh_target_names.clone(),
                                false,
                            )
                            .await
                            {
                                Ok(detail) => Some(format!(
                                    "BLE name apply confirmation readback was unavailable ({readback_error}). {detail}"
                                )),
                                Err(err) => {
                                    log::warn!(
                                        "[device-settings] BLE name Windows cache refresh after deferred apply failed: {err}"
                                    );
                                    coord.refresh_embedded_ble_listener();
                                    Some(format!(
                                        "BLE name apply was sent, but confirmation readback is unavailable ({readback_error}) and Windows Bluetooth cache refresh failed ({err})."
                                    ))
                                }
                            }
                    } else {
                        coord.refresh_embedded_ble_listener();
                        Some(format!(
                                "BLE name apply was sent without resetting Windows pairing because the requested name was already current; confirmation readback is unavailable ({readback_error})."
                            ))
                    }
                }
            },
            Err(apply_error) => {
                log::warn!("[device-settings] BLE name apply failed: {apply_error}");
                crate::embedded_ble::set_configured_bluetooth_target_name(&previous_prefs_ble_name);
                coord.refresh_embedded_ble_listener();
                Some(format!(
                    "BLE name was saved, but applying it without a pairing prompt failed ({apply_error}). Type kept the previous BLE target for this session."
                ))
            }
        }
    } else {
        None
    };
    match read_device_settings_snapshot_after_write(
        &request,
        post_apply_snapshot,
        led_zone_brightness_supported,
        plugged_low_power_enabled,
    )
    .await
    {
        Ok(mut snapshot) => {
            if let Some(detail) = ble_name_detail {
                snapshot.detail = Some(detail);
            }
            log::info!(
                "[device-settings] write confirmed commands={} elapsed_ms={}",
                command_count,
                settings_write_started.elapsed().as_millis()
            );
            Ok(snapshot)
        }
        Err(DeviceSettingsReadbackAfterWriteError::Mismatch(mismatch_error)) => Err(mismatch_error),
        Err(DeviceSettingsReadbackAfterWriteError::Unavailable(readback_error)) => {
            if !device_settings_readback_unavailable_allowed_after_write(
                ble_name_changed,
                ble_name_apply_needed,
            ) {
                return Err(format!(
                    "设备设置已发送，但固件读回不可用，无法确认写入是否生效：{readback_error}"
                ));
            }
            let mut snapshot = device_settings_snapshot_from_request(
                &request,
                previous_snapshot,
                plugged_low_power_enabled,
            );
            snapshot.detail = Some(device_settings_sent_but_readback_unavailable_detail(
                &readback_error,
            ));
            if let Some(detail) = ble_name_detail {
                snapshot.detail = Some(detail);
            }
            Ok(snapshot)
        }
    }
}

pub enum DeviceSettingsReadbackAfterWriteError {
    Unavailable(String),
    Mismatch(String),
}

pub async fn read_device_settings_snapshot_after_write(
    request: &DeviceSettingsUpdateRequest,
    first_snapshot: Option<DeviceSettingsSnapshot>,
    led_zone_brightness_supported: bool,
    plugged_low_power_enabled: bool,
) -> Result<DeviceSettingsSnapshot, DeviceSettingsReadbackAfterWriteError> {
    let mut first_snapshot = first_snapshot;
    let mut last_error = DeviceSettingsReadbackAfterWriteError::Unavailable(
        "device settings readback was not attempted".to_string(),
    );
    for attempt in 1..=DEVICE_SETTINGS_READBACK_VERIFY_ATTEMPTS {
        let snapshot_result = if attempt == 1 {
            match first_snapshot.take() {
                Some(snapshot) => Ok(snapshot),
                None => read_device_settings_snapshot_from_firmware().await,
            }
        } else {
            read_device_settings_snapshot_from_firmware().await
        };

        match snapshot_result {
            Ok(snapshot) => {
                let mismatches = device_settings_readback_mismatches(
                    &snapshot,
                    request,
                    led_zone_brightness_supported,
                    plugged_low_power_enabled,
                );
                if mismatches.is_empty() {
                    if attempt > 1 {
                        log::info!(
                            "[device-settings] write readback matched after retry attempt={attempt}/{}",
                            DEVICE_SETTINGS_READBACK_VERIFY_ATTEMPTS
                        );
                    }
                    return Ok(snapshot);
                }
                let mismatch_text = mismatches.join("; ");
                log::warn!(
                    "[device-settings] write readback mismatch attempt={attempt}/{}: {mismatch_text}",
                    DEVICE_SETTINGS_READBACK_VERIFY_ATTEMPTS
                );
                last_error = DeviceSettingsReadbackAfterWriteError::Mismatch(format!(
                    "设备设置写入后读回不一致：{mismatch_text}"
                ));
            }
            Err(readback_error) => {
                log::warn!(
                    "[device-settings] write readback unavailable attempt={attempt}/{}: {readback_error}",
                    DEVICE_SETTINGS_READBACK_VERIFY_ATTEMPTS
                );
                last_error = DeviceSettingsReadbackAfterWriteError::Unavailable(readback_error);
            }
        }

        if attempt < DEVICE_SETTINGS_READBACK_VERIFY_ATTEMPTS {
            tokio::time::sleep(DEVICE_SETTINGS_READBACK_VERIFY_RETRY_DELAY).await;
        }
    }
    Err(last_error)
}

pub fn device_settings_snapshot_from_status(
    status: crate::embedded_ble::DeviceSettingsStatus,
) -> DeviceSettingsSnapshot {
    let active_power_source = if status.external_power_present
        || status.usb_power_present
        || status.charging
        || status.charge_full
    {
        "plugged"
    } else {
        "battery"
    };
    DeviceSettingsSnapshot {
        schema: DEVICE_SETTINGS_SCHEMA,
        connected: true,
        write_supported: true,
        source: "firmware",
        status_led_brightness_percent: status.status_led_brightness_percent,
        key_led_brightness_percent: status.key_led_brightness_percent,
        knob_led_brightness_percent: status.knob_led_brightness_percent,
        edge_led_brightness_percent: status.edge_led_brightness_percent,
        led_zone_brightness_supported: status.led_zone_brightness_supported,
        compact_set_supported: status.compact_set_supported,
        low_power_idle_minutes: status.low_power_idle_minutes,
        plugged_low_power_idle_minutes: status.plugged_low_power_idle_minutes,
        battery_low_power_idle_minutes: status.battery_low_power_idle_minutes,
        plugged_low_power_enabled: status.plugged_low_power_enabled,
        voice_auto_start_enabled: status.voice_auto_start_enabled,
        voice_auto_stop_enabled: status.voice_auto_stop_enabled,
        plugged_auto_shutdown_ms: DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS,
        battery_auto_shutdown_ms: status.battery_auto_shutdown_minutes.saturating_mul(60_000),
        knob_rotation_action: ui_knob_rotation_action_from_firmware(&status.knob_rotation_action),
        ble_name: status.ble_name,
        ble_name_pending_restart: status.ble_name_pending_restart,
        active_power_source,
        battery_percent: None,
        detail: Some("Firmware DEVICE settings readback succeeded.".to_string()),
        last_updated_at: None,
    }
}

pub fn ui_knob_rotation_action_from_firmware(action: &str) -> String {
    match action {
        "system_volume" | "systemVolume" => "systemVolume".to_string(),
        "screen_brightness" | "screenBrightness" => "screenBrightness".to_string(),
        "disabled" => "disabled".to_string(),
        other => other.to_string(),
    }
}

pub fn device_settings_snapshot_from_device(
    device: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
) -> DeviceSettingsSnapshot {
    let active_power_source = match device.usb_powered {
        Some(true) => "plugged",
        Some(false) => "battery",
        None => "unknown",
    };
    let detail = if device.connected {
        Some(
            "Type can send DEVICE:SET, but firmware DEVICE readback is not currently available; showing defaults.".to_string(),
        )
    } else {
        device.detail.or_else(|| {
            Some("Listener BLE is not connected; showing firmware defaults.".to_string())
        })
    };
    DeviceSettingsSnapshot {
        schema: DEVICE_SETTINGS_SCHEMA,
        connected: device.connected,
        write_supported: device.connected,
        source: if device.connected {
            "defaults"
        } else {
            "unavailable"
        },
        status_led_brightness_percent: crate::types::DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
        key_led_brightness_percent: crate::types::DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT,
        knob_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        edge_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        led_zone_brightness_supported: false,
        compact_set_supported: false,
        low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_idle_minutes: DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
        battery_low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_enabled: DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_ms: DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS,
        battery_auto_shutdown_ms: DEVICE_SETTINGS_DEFAULT_BATTERY_AUTO_SHUTDOWN_MS,
        knob_rotation_action: ui_knob_rotation_action_from_firmware(
            firmware_mode_for_device_knob_rotation_action(DeviceKnobRotationAction::default()),
        ),
        ble_name: DEVICE_SETTINGS_DEFAULT_BLE_NAME.to_string(),
        ble_name_pending_restart: false,
        active_power_source,
        battery_percent: device.battery_percent,
        detail,
        last_updated_at: None,
    }
}

pub fn device_settings_snapshot_from_request(
    request: &DeviceSettingsUpdateRequest,
    previous: Option<DeviceSettingsSnapshot>,
    plugged_low_power_enabled: bool,
) -> DeviceSettingsSnapshot {
    let mut snapshot = previous.unwrap_or(DeviceSettingsSnapshot {
        schema: DEVICE_SETTINGS_SCHEMA,
        connected: true,
        write_supported: true,
        source: "lastKnown",
        status_led_brightness_percent: crate::types::DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
        key_led_brightness_percent: crate::types::DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT,
        knob_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        edge_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        led_zone_brightness_supported: false,
        compact_set_supported: false,
        low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_idle_minutes: DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
        battery_low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_enabled: DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
        voice_auto_start_enabled: false,
        voice_auto_stop_enabled: false,
        plugged_auto_shutdown_ms: DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS,
        battery_auto_shutdown_ms: DEVICE_SETTINGS_DEFAULT_BATTERY_AUTO_SHUTDOWN_MS,
        knob_rotation_action: ui_knob_rotation_action_from_firmware(
            firmware_mode_for_device_knob_rotation_action(DeviceKnobRotationAction::default()),
        ),
        ble_name: DEVICE_SETTINGS_DEFAULT_BLE_NAME.to_string(),
        ble_name_pending_restart: false,
        active_power_source: "unknown",
        battery_percent: None,
        detail: None,
        last_updated_at: None,
    });
    let old_name = snapshot.ble_name.clone();
    snapshot.connected = true;
    snapshot.write_supported = true;
    snapshot.source = "lastKnown";
    if snapshot.led_zone_brightness_supported {
        snapshot.status_led_brightness_percent = request.status_led_brightness_percent;
        snapshot.key_led_brightness_percent = request.key_led_brightness_percent;
        snapshot.knob_led_brightness_percent = request.knob_led_brightness_percent;
        snapshot.edge_led_brightness_percent = request.edge_led_brightness_percent;
    }
    snapshot.plugged_low_power_idle_minutes = request.plugged_low_power_idle_minutes;
    snapshot.battery_low_power_idle_minutes = request.battery_low_power_idle_minutes;
    snapshot.plugged_low_power_enabled = plugged_low_power_enabled;
    snapshot.voice_auto_start_enabled = request.voice_auto_start_enabled;
    snapshot.voice_auto_stop_enabled = request.voice_auto_stop_enabled;
    snapshot.low_power_idle_minutes = match snapshot.active_power_source {
        "plugged" => request.plugged_low_power_idle_minutes,
        "battery" => request.battery_low_power_idle_minutes,
        _ => request.battery_low_power_idle_minutes,
    };
    snapshot.plugged_auto_shutdown_ms = DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS;
    snapshot.battery_auto_shutdown_ms =
        request.battery_auto_shutdown_minutes.saturating_mul(60_000);
    snapshot.ble_name = request.ble_name.clone();
    snapshot.ble_name_pending_restart = old_name != request.ble_name;
    snapshot.detail = Some(
        "Device settings were sent; displayed values are last-known until firmware DEVICE readback succeeds.".to_string(),
    );
    snapshot
}

pub fn device_settings_readback_mismatches(
    snapshot: &DeviceSettingsSnapshot,
    request: &DeviceSettingsUpdateRequest,
    led_zone_brightness_supported: bool,
    plugged_low_power_enabled: bool,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    if led_zone_brightness_supported {
        if !snapshot.led_zone_brightness_supported {
            mismatches.push("led_zone_brightness_supported=false".to_string());
        } else {
            if snapshot.status_led_brightness_percent != request.status_led_brightness_percent {
                mismatches.push(format!(
                    "led_status expected={} actual={}",
                    request.status_led_brightness_percent, snapshot.status_led_brightness_percent
                ));
            }
            if snapshot.key_led_brightness_percent != request.key_led_brightness_percent {
                mismatches.push(format!(
                    "led_key expected={} actual={}",
                    request.key_led_brightness_percent, snapshot.key_led_brightness_percent
                ));
            }
            if snapshot.knob_led_brightness_percent != request.knob_led_brightness_percent {
                mismatches.push(format!(
                    "led_ec11 expected={} actual={}",
                    request.knob_led_brightness_percent, snapshot.knob_led_brightness_percent
                ));
            }
            if snapshot.edge_led_brightness_percent != request.edge_led_brightness_percent {
                mismatches.push(format!(
                    "led_edge expected={} actual={}",
                    request.edge_led_brightness_percent, snapshot.edge_led_brightness_percent
                ));
            }
        }
    }
    if snapshot.plugged_low_power_idle_minutes != request.plugged_low_power_idle_minutes {
        mismatches.push(format!(
            "plugged_low_power_idle_minutes expected={} actual={}",
            request.plugged_low_power_idle_minutes, snapshot.plugged_low_power_idle_minutes
        ));
    }
    if snapshot.battery_low_power_idle_minutes != request.battery_low_power_idle_minutes {
        mismatches.push(format!(
            "battery_low_power_idle_minutes expected={} actual={}",
            request.battery_low_power_idle_minutes, snapshot.battery_low_power_idle_minutes
        ));
    }
    if snapshot.plugged_low_power_enabled != plugged_low_power_enabled {
        mismatches.push(format!(
            "plugged_low_power_enabled expected={} actual={}",
            plugged_low_power_enabled, snapshot.plugged_low_power_enabled
        ));
    }
    if snapshot.voice_auto_start_enabled != request.voice_auto_start_enabled {
        mismatches.push(format!(
            "voice_auto_start expected={} actual={}",
            request.voice_auto_start_enabled, snapshot.voice_auto_start_enabled
        ));
    }
    if snapshot.voice_auto_stop_enabled != request.voice_auto_stop_enabled {
        mismatches.push(format!(
            "voice_auto_stop expected={} actual={}",
            request.voice_auto_stop_enabled, snapshot.voice_auto_stop_enabled
        ));
    }
    let expected_battery_auto_shutdown_ms =
        request.battery_auto_shutdown_minutes.saturating_mul(60_000);
    if snapshot.battery_auto_shutdown_ms != expected_battery_auto_shutdown_ms {
        mismatches.push(format!(
            "battery_auto_shutdown_ms expected={} actual={}",
            expected_battery_auto_shutdown_ms, snapshot.battery_auto_shutdown_ms
        ));
    }
    if snapshot.plugged_auto_shutdown_ms != DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS {
        mismatches.push(format!(
            "plugged_auto_shutdown_ms expected={} actual={}",
            DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS, snapshot.plugged_auto_shutdown_ms
        ));
    }
    if snapshot.ble_name != request.ble_name {
        mismatches.push(format!(
            "ble_name expected={:?} actual={:?}",
            request.ble_name, snapshot.ble_name
        ));
    }
    if snapshot.ble_name_pending_restart {
        mismatches.push("ble_name_pending_restart=true".to_string());
    }
    mismatches
}

pub fn ensure_device_settings_readback_matches_request(
    snapshot: &DeviceSettingsSnapshot,
    request: &DeviceSettingsUpdateRequest,
    led_zone_brightness_supported: bool,
    plugged_low_power_enabled: bool,
) -> Result<(), String> {
    let mismatches = device_settings_readback_mismatches(
        snapshot,
        request,
        led_zone_brightness_supported,
        plugged_low_power_enabled,
    );
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "设备设置写入后读回不一致：{}",
            mismatches.join("; ")
        ))
    }
}

pub fn device_settings_update_commands(
    request: &DeviceSettingsUpdateRequest,
    led_zone_brightness_supported: bool,
    plugged_low_power_enabled: bool,
    write_ble_name: bool,
    previous_snapshot: Option<&DeviceSettingsSnapshot>,
) -> Result<Vec<String>, String> {
    let mut assignments = Vec::new();
    let compact_set_supported =
        previous_snapshot.is_some_and(|snapshot| snapshot.compact_set_supported);
    if led_zone_brightness_supported {
        if previous_snapshot.map_or(true, |snapshot| {
            !snapshot.led_zone_brightness_supported
                || snapshot.status_led_brightness_percent != request.status_led_brightness_percent
        }) {
            assignments.push(device_setting_assignment(
                "led_status",
                request.status_led_brightness_percent,
                compact_set_supported,
            ));
        }
        if previous_snapshot.map_or(true, |snapshot| {
            !snapshot.led_zone_brightness_supported
                || snapshot.key_led_brightness_percent != request.key_led_brightness_percent
        }) {
            assignments.push(device_setting_assignment(
                "led_key",
                request.key_led_brightness_percent,
                compact_set_supported,
            ));
        }
        if previous_snapshot.map_or(true, |snapshot| {
            !snapshot.led_zone_brightness_supported
                || snapshot.knob_led_brightness_percent != request.knob_led_brightness_percent
        }) {
            assignments.push(device_setting_assignment(
                "led_ec11",
                request.knob_led_brightness_percent,
                compact_set_supported,
            ));
        }
        if previous_snapshot.map_or(true, |snapshot| {
            !snapshot.led_zone_brightness_supported
                || snapshot.edge_led_brightness_percent != request.edge_led_brightness_percent
        }) {
            assignments.push(device_setting_assignment(
                "led_edge",
                request.edge_led_brightness_percent,
                compact_set_supported,
            ));
        }
    }
    if previous_snapshot.map_or(true, |snapshot| {
        snapshot.plugged_low_power_idle_minutes != request.plugged_low_power_idle_minutes
    }) {
        assignments.push(device_setting_assignment(
            "plugged_low_power_idle_minutes",
            request.plugged_low_power_idle_minutes,
            compact_set_supported,
        ));
    }
    if previous_snapshot.map_or(true, |snapshot| {
        snapshot.battery_low_power_idle_minutes != request.battery_low_power_idle_minutes
    }) {
        assignments.push(device_setting_assignment(
            "battery_low_power_idle_minutes",
            request.battery_low_power_idle_minutes,
            compact_set_supported,
        ));
    }
    if previous_snapshot.map_or(true, |snapshot| {
        snapshot.plugged_low_power_enabled != plugged_low_power_enabled
    }) {
        assignments.push(device_setting_assignment(
            "plugged_low_power_enabled",
            if plugged_low_power_enabled { 1 } else { 0 },
            compact_set_supported,
        ));
    }
    if previous_snapshot.map_or(true, |snapshot| {
        snapshot.voice_auto_start_enabled != request.voice_auto_start_enabled
    }) {
        assignments.push(device_setting_assignment(
            "voice_auto_start",
            if request.voice_auto_start_enabled {
                1
            } else {
                0
            },
            compact_set_supported,
        ));
    }
    if previous_snapshot.map_or(true, |snapshot| {
        snapshot.voice_auto_stop_enabled != request.voice_auto_stop_enabled
    }) {
        assignments.push(device_setting_assignment(
            "voice_auto_stop",
            if request.voice_auto_stop_enabled {
                1
            } else {
                0
            },
            compact_set_supported,
        ));
    }
    if previous_snapshot.map_or(true, |snapshot| {
        snapshot.plugged_auto_shutdown_ms != DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS
    }) {
        assignments.push(device_setting_assignment(
            "plugged_auto_shutdown_minutes",
            "off",
            compact_set_supported,
        ));
    }
    let expected_battery_auto_shutdown_ms =
        request.battery_auto_shutdown_minutes.saturating_mul(60_000);
    if previous_snapshot.map_or(true, |snapshot| {
        snapshot.battery_auto_shutdown_ms != expected_battery_auto_shutdown_ms
    }) {
        assignments.push(device_setting_assignment(
            "battery_auto_shutdown_minutes",
            request.battery_auto_shutdown_minutes,
            compact_set_supported,
        ));
    }
    let mut commands = pack_device_setting_assignments(assignments)?;
    if write_ble_name {
        commands.push(format!("DEVICE:SET ble_name={}", request.ble_name));
    }
    for command in &commands {
        let bytes_with_newline = command.as_bytes().len() + 1;
        if bytes_with_newline > DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES {
            return Err(format!(
                "Device settings command is too long for BLE audio control: {bytes_with_newline} bytes."
            ));
        }
    }
    Ok(commands)
}

pub fn validate_device_settings_request(request: &DeviceSettingsUpdateRequest) -> Result<(), String> {
    if request.status_led_brightness_percent > 100
        || request.key_led_brightness_percent > 100
        || request.knob_led_brightness_percent > 100
        || request.edge_led_brightness_percent > 100
    {
        return Err("Device LED zone brightness must be between 0 and 100 percent.".to_string());
    }
    if request.plugged_low_power_idle_minutes > MAX_DEVICE_LOW_POWER_IDLE_MINUTES
        || request.battery_low_power_idle_minutes > MAX_DEVICE_LOW_POWER_IDLE_MINUTES
    {
        return Err(format!(
            "Low-power idle must be between 0 and {MAX_DEVICE_LOW_POWER_IDLE_MINUTES} minutes."
        ));
    }
    if request.plugged_auto_shutdown_minutes != 0 {
        return Err(
            "Plugged auto-shutdown is disabled; use battery auto-shutdown instead.".to_string(),
        );
    }
    if request.battery_auto_shutdown_minutes > DEVICE_SETTINGS_MAX_AUTO_SHUTDOWN_MINUTES {
        return Err(format!(
            "Auto-shutdown must be between {DEVICE_SETTINGS_MIN_AUTO_SHUTDOWN_MINUTES} and {DEVICE_SETTINGS_MAX_AUTO_SHUTDOWN_MINUTES} minutes."
        ));
    }
    validate_device_settings_ble_name(&request.ble_name)
}

pub fn validate_device_settings_ble_name(name: &str) -> Result<(), String> {
    if !device_ble_name_is_valid(name) {
        return Err("BLE name must be 1-29 printable ASCII characters without spaces, quotes, semicolon, equals sign, or backslash.".to_string());
    }
    Ok(())
}
