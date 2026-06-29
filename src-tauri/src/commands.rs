//! Tauri command surface — every IPC entry the React UI invokes lives here.

use std::borrow::Cow;
use std::collections::{hash_map::DefaultHasher, BTreeMap};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use espflash::connection::reset::{ResetAfterOperation, ResetBeforeOperation};
use espflash::elf::RomSegment;
use espflash::flasher::{FlashFrequency, FlashMode, FlashSize, Flasher, ProgressCallbacks};
use espflash::targets::Chip;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serialport::{FlowControl, SerialPortType, UsbPortInfo};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::asr::local::foundry::{
    model_alias_is_known, FoundryCatalogModel, FoundryPrepareProgressPayload, FoundryRuntimeStatus,
    DEFAULT_MODEL_ALIAS, PROVIDER_ID as FOUNDRY_LOCAL_PROVIDER_ID,
};
use crate::asr::local::FoundryLocalRuntime;
use crate::coordinator::{
    Coordinator, EmbeddedBleNotifySubscriptionState, EmbeddedBleSessionActorDiagnosticRecord,
    EmbeddedBleWakeRecoverySnapshot, FirmwareWakePolicySnapshot,
};
use crate::coordinator_state::SessionPhase;
use crate::github_oauth::{
    current_epoch_secs, refresh_token_is_expired, token_needs_refresh, GithubDevicePollStatus,
    GithubDeviceStartResponse, GithubOAuthClient, GithubOAuthError,
};
use crate::marketplace_backend::{
    MarketplaceApiError, MarketplaceApiErrorKind, MarketplaceClient, MarketplaceDetail,
    MarketplaceListPage, MarketplaceMyPackItem,
};
use crate::permissions::{self, PermissionStatus};
use crate::persistence::{
    sync_style_pack_preferences, CredentialAccount, CredentialsSnapshot, CredentialsVault,
    MarketplaceGithubCredentials, PreferencesStore,
};
use crate::polish::{
    http_client_builder_with_proxy, CodexOAuthConfig, CodexOAuthCredentials, CodexOAuthLLMProvider,
    LLMError, OpenAICompatibleConfig, OpenAICompatibleLLMProvider, ProviderProxyConfig,
    CODEX_DEFAULT_MODEL, CODEX_OAUTH_PROVIDER_ID,
};
use crate::recorder::{AudioConsumer, Recorder};
use crate::types::{
    builtin_style_pack_id, default_active_style_pack_id, device_ble_name_is_valid,
    ChineseScriptPreference, ComboBinding, CorrectionRule, CredentialsStatus,
    DeviceCustomKeyAction, DeviceCustomKeyGesture, DeviceCustomKeyId, DeviceCustomKeyMapping,
    DeviceCustomKeys, DeviceKnobRotationAction, DictationInputSource, DictationSession,
    DictionaryEntry, HotkeyCapability, HotkeyStatus, OutputLanguagePreference, PolishMode,
    ShortcutBinding, StylePack, StylePackKind, StylePackRuntimeDiagnostics, StyleSystemPrompts,
    UserPreferences, VocabPresetStore, WindowsImeStatus,
    DEFAULT_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES, DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
    MAX_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES, MAX_DEVICE_LOW_POWER_IDLE_MINUTES,
};

type CoordinatorState<'a> = State<'a, Arc<Coordinator>>;
pub type MicrophoneMonitorState = Mutex<Option<Recorder>>;
pub type TrayMicrophoneMenuState = Mutex<Vec<TrayMicrophoneMenuItem>>;

static SETTINGS_UPDATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static PROVIDER_MODELS_CACHE: OnceLock<Mutex<Vec<ProviderModelsCacheEntry>>> = OnceLock::new();
static PROVIDER_MODELS_FETCH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
const PROVIDER_MODELS_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderModelsCacheKey {
    kind: String,
    provider_id: String,
    base_url: String,
    api_key_hash: u64,
    proxy_config: ProviderProxyConfig,
}

#[derive(Clone, Debug)]
struct ProviderModelsCacheEntry {
    key: ProviderModelsCacheKey,
    models: Vec<String>,
    fetched_at: Instant,
}

pub struct TrayMicrophoneMenuItem {
    pub id: String,
    pub device_name: String,
    pub item: tauri::menu::CheckMenuItem<tauri::Wry>,
}

pub fn sync_tray_microphone_selection(items: &[TrayMicrophoneMenuItem], device_name: &str) {
    for item in items {
        let _ = item.item.set_checked(item.device_name == device_name);
    }
}

struct LevelProbeConsumer;

impl AudioConsumer for LevelProbeConsumer {
    fn consume_pcm_chunk(&self, _pcm: &[u8]) {}
}

fn settings_update_lock() -> &'static Mutex<()> {
    SETTINGS_UPDATE_LOCK.get_or_init(|| Mutex::new(()))
}

// ─────────────────────────── settings + credentials ───────────────────────────

#[tauri::command]
pub fn get_settings(coord: CoordinatorState<'_>) -> UserPreferences {
    coord.prefs().get()
}

#[tauri::command]
pub fn is_main_window_start_hidden(coord: CoordinatorState<'_>) -> bool {
    if std::env::var("LISTENER_TYPE_SHOW_MAIN_ON_START")
        .ok()
        .as_deref()
        == Some("1")
    {
        return false;
    }
    let hide_main_on_start = std::env::var("LISTENER_TYPE_HIDE_MAIN_ON_START")
        .ok()
        .as_deref()
        == Some("1");
    hide_main_on_start || coord.prefs().get().start_minimized
}

#[tauri::command]
pub fn get_default_style_system_prompts() -> StyleSystemPrompts {
    StyleSystemPrompts::default()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApplication {
    pub name: String,
    pub path: String,
    pub source: String,
}

#[tauri::command]
pub fn list_installed_applications() -> Vec<InstalledApplication> {
    collect_installed_applications()
}

#[cfg(target_os = "windows")]
fn collect_installed_applications() -> Vec<InstalledApplication> {
    let mut apps = BTreeMap::<String, InstalledApplication>::new();
    for (root, source) in windows_start_menu_roots() {
        collect_start_menu_shortcuts(&root, source, &mut apps);
    }
    let mut values: Vec<_> = apps.into_values().collect();
    values.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    values.truncate(400);
    values
}

#[cfg(not(target_os = "windows"))]
fn collect_installed_applications() -> Vec<InstalledApplication> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn windows_start_menu_roots() -> Vec<(PathBuf, &'static str)> {
    let mut roots = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        roots.push((
            PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
            "userStartMenu",
        ));
    }
    if let Ok(programdata) = std::env::var("PROGRAMDATA") {
        roots.push((
            PathBuf::from(programdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
            "commonStartMenu",
        ));
    }
    roots
}

#[cfg(target_os = "windows")]
fn collect_start_menu_shortcuts(
    root: &Path,
    source: &'static str,
    apps: &mut BTreeMap<String, InstalledApplication>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_start_menu_shortcuts(&path, source, apps);
            continue;
        }
        let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !matches!(ext.as_str(), "lnk" | "appref-ms" | "exe") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let name = cleanup_start_menu_app_name(name);
        if name.is_empty() || is_non_launch_shortcut_name(&name) {
            continue;
        }
        let path = path.display().to_string();
        let key = format!(
            "{}|{}",
            name.to_ascii_lowercase(),
            path.to_ascii_lowercase()
        );
        apps.entry(key).or_insert(InstalledApplication {
            name,
            path,
            source: source.to_string(),
        });
    }
}

#[cfg(target_os = "windows")]
fn cleanup_start_menu_app_name(raw: &str) -> String {
    raw.trim()
        .trim_end_matches(" - Shortcut")
        .trim_end_matches(" - 快捷方式")
        .trim()
        .to_string()
}

#[cfg(target_os = "windows")]
fn is_non_launch_shortcut_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "uninstall",
        "readme",
        "read me",
        "license",
        "documentation",
        "website",
        "help",
        "卸载",
        "解除安装",
        "解除安裝",
        "说明",
        "說明",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiTimelineEvent {
    source: String,
    event: String,
    state: Option<String>,
    elapsed_ms: Option<u64>,
    detail: Option<Value>,
}

#[tauri::command]
pub fn record_ui_timeline_event(payload: UiTimelineEvent) {
    crate::capsule_log::record_ui_event(
        &payload.source,
        &payload.event,
        payload.state.as_deref(),
        payload.elapsed_ms,
        payload.detail.as_ref(),
    );
    crate::timeline::mark(
        &payload.source,
        &payload.event,
        format!(
            "state={} elapsed_ms={} detail={}",
            payload.state.as_deref().unwrap_or("-"),
            payload
                .elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            payload
                .detail
                .as_ref()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "{}".into())
        ),
    );
}

trait SettingsWriter {
    fn write_settings(&self, prefs: UserPreferences) -> Result<(), String>;
    fn refresh_dictation_hotkey(&self);
    fn refresh_qa_hotkey(&self);
    fn refresh_combo_hotkey(&self);
    fn refresh_translation_hotkey(&self);
    fn refresh_switch_style_hotkey(&self);
    fn refresh_open_app_hotkey(&self);
    fn refresh_device_custom_key_hotkeys(&self);
}

impl SettingsWriter for Coordinator {
    fn write_settings(&self, prefs: UserPreferences) -> Result<(), String> {
        self.prefs().set(prefs).map_err(|e| e.to_string())
    }

    fn refresh_dictation_hotkey(&self) {
        self.update_hotkey_binding();
    }

    fn refresh_qa_hotkey(&self) {
        self.update_qa_hotkey_binding();
    }

    fn refresh_combo_hotkey(&self) {
        self.update_combo_hotkey_binding();
    }

    fn refresh_translation_hotkey(&self) {
        self.update_translation_hotkey_binding();
    }

    fn refresh_switch_style_hotkey(&self) {
        self.update_switch_style_hotkey_binding();
    }

    fn refresh_open_app_hotkey(&self) {
        self.update_open_app_hotkey_binding();
    }

    fn refresh_device_custom_key_hotkeys(&self) {
        self.update_device_custom_key_hotkey_bindings();
    }
}

impl<T: SettingsWriter + ?Sized> SettingsWriter for Arc<T> {
    fn write_settings(&self, prefs: UserPreferences) -> Result<(), String> {
        (**self).write_settings(prefs)
    }

    fn refresh_dictation_hotkey(&self) {
        (**self).refresh_dictation_hotkey();
    }

    fn refresh_qa_hotkey(&self) {
        (**self).refresh_qa_hotkey();
    }

    fn refresh_combo_hotkey(&self) {
        (**self).refresh_combo_hotkey();
    }

    fn refresh_translation_hotkey(&self) {
        (**self).refresh_translation_hotkey();
    }

    fn refresh_switch_style_hotkey(&self) {
        (**self).refresh_switch_style_hotkey();
    }

    fn refresh_open_app_hotkey(&self) {
        (**self).refresh_open_app_hotkey();
    }

    fn refresh_device_custom_key_hotkeys(&self) {
        (**self).refresh_device_custom_key_hotkeys();
    }
}

fn persist_settings<T: SettingsWriter>(
    coord: &T,
    mut prefs: UserPreferences,
) -> Result<(), String> {
    if !prefs.dictation_input_source_user_overridden {
        prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    }
    sync_dictation_hotkey_legacy_fields(&mut prefs);
    prefs.device_custom_keys_default_migrated = true;
    reject_hotkey_collisions(&prefs)?;
    validate_device_custom_keys(&prefs.device_custom_keys)?;
    validate_device_custom_keys(&prefs.device_custom_key_double_clicks)?;
    validate_device_custom_keys(&prefs.device_custom_key_long_presses)?;
    coord.write_settings(prefs)?;
    coord.refresh_dictation_hotkey();
    coord.refresh_qa_hotkey();
    coord.refresh_combo_hotkey();
    coord.refresh_translation_hotkey();
    coord.refresh_switch_style_hotkey();
    coord.refresh_open_app_hotkey();
    coord.refresh_device_custom_key_hotkeys();
    Ok(())
}

fn device_firmware_settings_changed(previous: &UserPreferences, next: &UserPreferences) -> bool {
    previous.device_knob_rotation_action != next.device_knob_rotation_action
        || previous.device_low_power_idle_minutes != next.device_low_power_idle_minutes
        || previous.device_plugged_low_power_enabled != next.device_plugged_low_power_enabled
        || previous.device_battery_auto_shutdown_minutes
            != next.device_battery_auto_shutdown_minutes
        || previous.device_ble_name != next.device_ble_name
}

fn validate_device_firmware_preferences(prefs: &UserPreferences) -> Result<(), String> {
    if prefs.device_low_power_idle_minutes > MAX_DEVICE_LOW_POWER_IDLE_MINUTES {
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

fn firmware_mode_for_device_knob_rotation_action(action: DeviceKnobRotationAction) -> &'static str {
    match action {
        DeviceKnobRotationAction::SystemVolume => "system_volume",
        DeviceKnobRotationAction::ScreenBrightness => "screen_brightness",
        DeviceKnobRotationAction::Disabled => "disabled",
    }
}

struct DeviceSettingPacket {
    id: &'static str,
    command: String,
}

fn sync_device_setting_packet_to_firmware(packet: DeviceSettingPacket) -> Result<(), String> {
    crate::embedded_ble::send_device_settings_command(&packet.command, Duration::from_secs(2))
        .map_err(|err| format!("设备设置写入固件失败（{}）：{err}", packet.id))
}

fn device_setting_packets_for_changes(
    previous: &UserPreferences,
    next: &UserPreferences,
) -> Vec<DeviceSettingPacket> {
    let mut packets = Vec::new();
    if previous.device_knob_rotation_action != next.device_knob_rotation_action {
        let mode = firmware_mode_for_device_knob_rotation_action(next.device_knob_rotation_action);
        packets.push(DeviceSettingPacket {
            id: "knob_rotation",
            command: format!("DEVICE:SET knob_rotation={mode}"),
        });
    }
    if previous.device_low_power_idle_minutes != next.device_low_power_idle_minutes {
        packets.push(DeviceSettingPacket {
            id: "low_power_idle_minutes",
            command: format!(
                "DEVICE:SET low_power_idle_minutes={}",
                next.device_low_power_idle_minutes
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

fn sync_device_firmware_preferences(
    previous: &UserPreferences,
    next: &UserPreferences,
) -> Result<(), String> {
    for packet in device_setting_packets_for_changes(previous, next) {
        sync_device_setting_packet_to_firmware(packet)?;
    }
    Ok(())
}

#[tauri::command]
pub fn set_settings(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    tray_microphones: State<'_, TrayMicrophoneMenuState>,
    mut prefs: UserPreferences,
) -> Result<(), String> {
    let packs = coord.style_packs().list().map_err(|e| e.to_string())?;
    sync_style_pack_preferences(&mut prefs, &packs);
    let _settings_guard = settings_update_lock().lock();
    let previous_prefs = coord.prefs().get();
    if prefs.dictation_input_source != previous_prefs.dictation_input_source {
        prefs.dictation_input_source_user_overridden = true;
    }
    if !prefs.dictation_input_source_user_overridden {
        prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    }
    let next_input_source = prefs.dictation_input_source;
    let should_sync_device_firmware = device_firmware_settings_changed(&previous_prefs, &prefs);
    if should_sync_device_firmware {
        validate_device_firmware_preferences(&prefs)?;
        sync_device_firmware_preferences(&previous_prefs, &prefs)?;
    }
    // 广播给所有 webview。issue #205：QaPanel 跑在独立 webview，
    // 没有 HotkeySettingsContext，必须靠事件感知录音键变化，否则面板可见时
    // 用户改键会让浮窗里的 "{recordHotkey}" 文案一直停留在旧值。
    persist_settings(&*coord, prefs.clone())?;
    coord.refresh_embedded_ble_listener();
    if next_input_source == DictationInputSource::EmbeddedBle && !should_sync_device_firmware {
        coord.sync_device_knob_rotation_action_to_firmware("settings_save");
    }
    // refresh_tray_microphone_menu 内部会调用 NSStatusItem.set_menu，必须在主线程上跑。
    // set_settings 本身是同步 Tauri command，在 IPC handler 线程上执行；从这里直接调
    // 会触发 macOS 主线程断言或在 dispatch 队列上死锁，导致整个 UI 无响应（用户改
    // 偏好后所有按键都没反应即此根因）。dispatch 到主线程后立即返回，IPC 线程不阻塞。
    let app_for_main = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(err) = crate::refresh_tray_microphone_menu(&app_for_main) {
            log::warn!("[tray] refresh microphone menu after settings save failed: {err}");
            let tray_state = app_for_main.state::<TrayMicrophoneMenuState>();
            let coord = app_for_main.state::<Arc<Coordinator>>();
            let current_prefs = coord.prefs().get();
            sync_tray_microphone_selection(
                &tray_state.lock(),
                &current_prefs.microphone_device_name,
            );
        }
    });
    // 抑制 unused 警告：tray_microphones 现在改在闭包里通过 app.state 取，
    // 但函数签名保留 State 入参，以便 Tauri 在调用前注入。
    let _ = tray_microphones;
    let _ = app.emit("prefs:changed", &prefs);
    Ok(())
}

#[tauri::command]
pub async fn refresh_device_settings_status(
) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::embedded_ble::read_device_settings_status(Duration::from_secs(4))
    })
    .await
    .map_err(|err| format!("device settings refresh task failed: {err}"))?
}

fn refresh_tray_menu_async(app: &AppHandle) {
    let app_for_main = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(err) = crate::refresh_tray_microphone_menu(&app_for_main) {
            log::warn!("[tray] refresh after style change failed: {err}");
        }
    });
}

fn emit_prefs_changed(app: &AppHandle, prefs: &UserPreferences) {
    let _ = app.emit("prefs:changed", prefs);
    let _ = app.emit_to("main", "prefs:changed", prefs);
}

pub(crate) fn sync_style_pack_prefs_and_persist(
    coord: &Coordinator,
    app: &AppHandle,
    mut prefs: UserPreferences,
) -> Result<UserPreferences, String> {
    let packs = coord.style_packs().list().map_err(|e| e.to_string())?;
    sync_style_pack_preferences(&mut prefs, &packs);
    coord
        .prefs()
        .set(prefs.clone())
        .map_err(|e| e.to_string())?;
    emit_prefs_changed(app, &prefs);
    refresh_tray_menu_async(app);
    Ok(prefs)
}

pub(crate) fn activate_style_pack_by_id(
    coord: &Coordinator,
    app: &AppHandle,
    id: &str,
) -> Result<StylePack, String> {
    let mut prefs = coord.prefs().get();
    let pack = coord.style_packs().get(id).map_err(|e| e.to_string())?;
    log::info!(
        "[style-pack] activate helper requested id={} kind={:?} base_mode={:?} enabled={}",
        pack.id,
        pack.kind,
        pack.base_mode,
        pack.enabled
    );
    if !pack.enabled {
        coord
            .style_packs()
            .set_enabled(id, true)
            .map_err(|e| e.to_string())?;
    }
    prefs.active_style_pack_id = id.to_string();
    sync_style_pack_prefs_and_persist(coord, app, prefs)?;
    log::info!("[style-pack] activate helper applied id={id}");
    coord
        .style_packs()
        .get(id)
        .map(|mut pack| {
            pack.active = true;
            pack
        })
        .map_err(|e| e.to_string())
}

pub(crate) fn activate_builtin_style_mode(
    coord: &Coordinator,
    app: &AppHandle,
    mode: PolishMode,
) -> Result<(), String> {
    let pack_id = builtin_style_pack_id(mode).to_string();
    log::info!(
        "[style-pack] activate builtin mode helper mode={:?} pack_id={}",
        mode,
        pack_id
    );
    let _ = activate_style_pack_by_id(coord, app, &pack_id)?;
    Ok(())
}

#[tauri::command]
pub fn get_hotkey_status(coord: CoordinatorState<'_>) -> HotkeyStatus {
    coord.hotkey_status()
}

#[tauri::command]
pub fn get_hotkey_capability(coord: CoordinatorState<'_>) -> HotkeyCapability {
    coord.hotkey_capability()
}

/// Pull-style 查询：当前是否处于 Linux/Wayland session（rdev 不可用、需要走 CLI 路径）。
/// 前端 RecordingSection mount 时调一次拿状态，直接渲染 callout。
///
/// 用 pull 而不是单纯依赖 ready-time 的 `wayland_cli_mode` event：Settings 模态是
/// 条件渲染（用户首次打开 Settings 才 mount RecordingSection），但 emit 发生在 setup
/// 末尾——一次性 event 不缓冲也不 replay，listener 99% 情况下错过事件 → callout
/// 永远不显示。XDG_SESSION_TYPE 本身在进程生命周期内不会变，多次调用结果一致。
#[tauri::command]
pub fn is_wayland_cli_mode() -> bool {
    crate::hotkey::is_wayland_session()
}

#[tauri::command]
pub fn set_shortcut_recording_active(coord: CoordinatorState<'_>, active: bool) {
    coord.set_shortcut_recording_active(active);
}

#[tauri::command]
pub fn get_windows_ime_status() -> WindowsImeStatus {
    crate::windows_ime_profile::get_windows_ime_status()
}

#[tauri::command]
pub fn list_microphone_devices() -> Result<Vec<crate::recorder::MicrophoneDevice>, String> {
    crate::recorder::list_input_devices().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_microphone_level_monitor(
    app: AppHandle,
    device_name: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<MicrophoneMonitorState>();
        if let Some(existing) = state.lock().take() {
            existing.stop();
        }

        let selected = device_name.trim().to_string();
        let microphone_device_name = if selected.is_empty() {
            None
        } else {
            Some(selected)
        };
        let consumer: Arc<dyn AudioConsumer> = Arc::new(LevelProbeConsumer);
        let level_app = app.clone();
        let level_handler: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |level| {
            let _ = level_app.emit("microphone:level", serde_json::json!({ "level": level }));
        });
        let (recorder, _runtime_errors, _archive_active) =
            Recorder::start(microphone_device_name, consumer, level_handler, None)
                .map_err(|e| e.to_string())?;
        *state.lock() = Some(recorder);
        Ok(())
    })
    .await
    .map_err(|e| format!("start microphone monitor task failed: {e}"))?
}

#[tauri::command]
pub async fn stop_microphone_level_monitor(app: AppHandle) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<MicrophoneMonitorState>();
        let recorder = state.lock().take();
        if let Some(recorder) = recorder {
            recorder.stop();
        }
    })
    .await;
}

#[tauri::command]
pub fn get_credentials() -> CredentialsStatus {
    let snap = CredentialsVault::snapshot();
    let active_asr_provider = CredentialsVault::get_active_asr();
    let active_llm_provider = CredentialsVault::get_active_llm();
    let volcengine_configured = volcengine_configured(&snap);
    let asr_configured = asr_configured_for_provider(&active_asr_provider, &snap);
    let llm_configured = llm_configured_for_provider(&active_llm_provider, &snap);
    CredentialsStatus {
        active_asr_provider,
        active_llm_provider,
        asr_configured,
        llm_configured,
        volcengine_configured,
        ark_configured: llm_configured,
    }
}

fn volcengine_configured(snap: &CredentialsSnapshot) -> bool {
    configured(&snap.volcengine_app_key)
        && configured(&snap.volcengine_access_key)
        && configured(&snap.volcengine_resource_id)
}

fn asr_configured_for_provider(provider: &str, snap: &CredentialsSnapshot) -> bool {
    if provider == "volcengine" {
        return volcengine_configured(snap);
    }
    if provider == crate::asr::local::PROVIDER_ID || active_foundry_asr_is_supported(provider) {
        // 本地 ASR 不依赖云端凭据。
        return true;
    }
    if provider == crate::asr::bailian::PROVIDER_ID {
        return configured(&snap.asr_api_key);
    }
    configured(&snap.asr_endpoint) && configured(&snap.asr_model)
}

fn llm_configured_for_provider(provider: &str, snap: &CredentialsSnapshot) -> bool {
    if provider == CODEX_OAUTH_PROVIDER_ID {
        return CodexOAuthCredentials::load_default().is_ok();
    }
    let endpoint = snap.ark_endpoint.as_deref().unwrap_or_default();
    let endpoint_and_model = configured(&snap.ark_endpoint) && configured(&snap.ark_model_id);
    if endpoint_and_model
        && llm_provider_default_endpoint(provider)
            .map(|default| same_llm_endpoint(endpoint, default))
            .unwrap_or(false)
    {
        return configured(&snap.ark_api_key);
    }
    endpoint_and_model
}

fn llm_provider_default_endpoint(provider: &str) -> Option<&'static str> {
    match provider {
        "ark" => Some("https://ark.cn-beijing.volces.com/api/v3"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        "siliconflow" => Some("https://api.siliconflow.cn/v1"),
        "openai" => Some("https://api.openai.com/v1"),
        // 谷歌 Gemini 原生 API（v1beta）。后端 llm_gemini.rs 会拼成
        // `{baseUrl}/models/{model}:generateContent`，认证用 x-goog-api-key 头。
        "gemini" => Some("https://generativelanguage.googleapis.com/v1beta"),
        "mimo" => Some("https://api.xiaomimimo.com/v1"),
        "cometapi" => Some("https://api.cometapi.com/v1"),
        "openrouterFree" => Some("https://openrouter.ai/api/v1"),
        "alibabaCoding" => Some("https://coding-intl.dashscope.aliyuncs.com/v1"),
        "codingPlanX" => Some("https://api.codingplanx.ai/v1"),
        _ => None,
    }
}

fn same_llm_endpoint(a: &str, b: &str) -> bool {
    fn normalize(value: &str) -> &str {
        value
            .trim()
            .trim_end_matches('/')
            .trim_end_matches("/chat/completions")
            .trim_end_matches('/')
    }
    normalize(a).eq_ignore_ascii_case(normalize(b))
}

fn configured(field: &Option<String>) -> bool {
    field
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalAsrReleasePlan {
    qwen: bool,
    foundry: bool,
}

fn local_asr_release_plan_for_provider(provider: &str) -> LocalAsrReleasePlan {
    LocalAsrReleasePlan {
        qwen: provider != crate::asr::local::PROVIDER_ID,
        foundry: provider != FOUNDRY_LOCAL_PROVIDER_ID,
    }
}

async fn release_foundry_runtime_if_inactive(
    runtime: &Arc<FoundryLocalRuntime>,
    release_foundry: bool,
) {
    if release_foundry {
        runtime.request_cancel_prepare();
        if let Err(error) = runtime.release_now().await {
            log::warn!("[foundry-asr] release inactive runtime failed: {error:#}");
        }
    }
}

#[tauri::command]
pub fn set_credential(window: Window, account: String, value: String) -> Result<(), String> {
    ensure_main_window(&window)?;
    let acc = parse_account(&account)?;
    if value.is_empty() {
        CredentialsVault::remove(acc).map_err(|e| e.to_string())
    } else {
        CredentialsVault::set(acc, &value).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn set_active_asr_provider(
    coord: CoordinatorState<'_>,
    runtime: State<'_, Arc<FoundryLocalRuntime>>,
    provider: String,
) -> Result<(), String> {
    if provider == FOUNDRY_LOCAL_PROVIDER_ID && !active_foundry_asr_is_supported(&provider) {
        return Err("Foundry Local Whisper is only available on Windows".to_string());
    }
    CredentialsVault::set_active_asr_provider(&provider).map_err(|e| e.to_string())?;
    let release_plan = local_asr_release_plan_for_provider(&provider);
    if provider == crate::asr::local::PROVIDER_ID {
        // 切到本地 ASR → 后台预加载模型，下次按 hotkey 时不必等数秒。
        coord.preload_local_asr_in_background();
    }
    if release_plan.qwen {
        // 切回云端 → 用户已不需要本地引擎，立刻释放 1.2GB+ RAM；不释放的话只会等到
        // schedule_local_asr_release 的下一次 dictation 才触发，而切回云端后根本不会
        // 再走 local 路径，引擎会驻留到进程退出。
        coord.release_local_asr_engine();
    }
    release_foundry_runtime_if_inactive(runtime.inner(), release_plan.foundry).await;
    Ok(())
}

#[tauri::command]
pub fn set_active_llm_provider(provider: String) -> Result<(), String> {
    CredentialsVault::set_active_llm_provider(&provider).map_err(|e| e.to_string())
}

/// 读出某个账号的实际值（用于设置页预填表单）。
/// 凭据来自系统凭据库；只允许主设置窗口读取 raw secret，避免胶囊 / QA 等辅助窗口默认暴露。
#[tauri::command]
pub fn read_credential(window: Window, account: String) -> Result<Option<String>, String> {
    ensure_main_window(&window)?;
    let acc = parse_account(&account)?;
    CredentialsVault::get(acc).map_err(|e| e.to_string())
}

fn ensure_main_window(window: &Window) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("credential access is only allowed from the main window".to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCheckResult {
    ok: bool,
}

#[derive(Serialize)]
pub struct ProviderModelsResult {
    models: Vec<String>,
}

#[tauri::command]
pub async fn validate_provider_credentials(kind: String) -> Result<ProviderCheckResult, String> {
    match kind.as_str() {
        "llm" => validate_llm_provider()
            .await
            .map(|()| ProviderCheckResult { ok: true }),
        "asr" => validate_asr_provider()
            .await
            .map(|()| ProviderCheckResult { ok: true }),
        _ => Err(format!("unknown provider kind: {kind}")),
    }
}

#[tauri::command]
pub async fn list_provider_models(kind: String) -> Result<ProviderModelsResult, String> {
    if kind == "asr" && CredentialsVault::get_active_asr() == crate::asr::bailian::PROVIDER_ID {
        return Ok(ProviderModelsResult {
            models: vec![crate::asr::bailian::DEFAULT_MODEL.to_string()],
        });
    }
    if kind == "llm" && CredentialsVault::get_active_llm() == CODEX_OAUTH_PROVIDER_ID {
        return Ok(ProviderModelsResult {
            models: vec![
                CODEX_DEFAULT_MODEL.to_string(),
                "gpt-5.3-codex".to_string(),
                "gpt-5.4".to_string(),
                "gpt-5.5".to_string(),
            ],
        });
    }
    let config = read_openai_provider_config(&kind)?;
    fetch_provider_models_cached(&kind, &config)
        .await
        .map(|models| ProviderModelsResult { models })
}

struct ProviderConfig {
    provider_id: String,
    base_url: String,
    api_key: String,
    proxy_config: ProviderProxyConfig,
}

fn provider_models_cache() -> &'static Mutex<Vec<ProviderModelsCacheEntry>> {
    PROVIDER_MODELS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn provider_models_fetch_lock() -> &'static tokio::sync::Mutex<()> {
    PROVIDER_MODELS_FETCH_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn provider_models_cache_key(kind: &str, config: &ProviderConfig) -> ProviderModelsCacheKey {
    ProviderModelsCacheKey {
        kind: kind.to_string(),
        provider_id: config.provider_id.clone(),
        base_url: config.base_url.trim().trim_end_matches('/').to_string(),
        api_key_hash: hash_provider_api_key(&config.api_key),
        proxy_config: config.proxy_config.clone(),
    }
}

fn hash_provider_api_key(api_key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    api_key.hash(&mut hasher);
    hasher.finish()
}

async fn fetch_provider_models_cached(
    kind: &str,
    config: &ProviderConfig,
) -> Result<Vec<String>, String> {
    let key = provider_models_cache_key(kind, config);
    let mut now = Instant::now();
    {
        let mut cache = provider_models_cache().lock();
        cache.retain(|entry| now.duration_since(entry.fetched_at) < PROVIDER_MODELS_CACHE_TTL);
        if let Some(entry) = cache.iter().find(|entry| entry.key == key) {
            log::info!(
                "[provider-check] models cache hit kind={} provider={}",
                kind,
                config.provider_id
            );
            return Ok(entry.models.clone());
        }
    }

    let _fetch_guard = provider_models_fetch_lock().lock().await;
    now = Instant::now();
    {
        let mut cache = provider_models_cache().lock();
        cache.retain(|entry| now.duration_since(entry.fetched_at) < PROVIDER_MODELS_CACHE_TTL);
        if let Some(entry) = cache.iter().find(|entry| entry.key == key) {
            log::info!(
                "[provider-check] models cache hit after wait kind={} provider={}",
                kind,
                config.provider_id
            );
            return Ok(entry.models.clone());
        }
    }

    let models = fetch_provider_models(config).await?;
    let fetched_at = Instant::now();
    let mut cache = provider_models_cache().lock();
    cache.retain(|entry| {
        fetched_at.duration_since(entry.fetched_at) < PROVIDER_MODELS_CACHE_TTL && entry.key != key
    });
    cache.push(ProviderModelsCacheEntry {
        key,
        models: models.clone(),
        fetched_at,
    });
    if cache.len() > 16 {
        cache.sort_by_key(|entry| entry.fetched_at);
        let overflow = cache.len() - 16;
        cache.drain(0..overflow);
    }
    Ok(models)
}

fn read_openai_provider_config(kind: &str) -> Result<ProviderConfig, String> {
    let (provider_id, api_key_account, endpoint_account, api_key_required) = match kind {
        "llm" => (
            CredentialsVault::get_active_llm(),
            CredentialAccount::ArkApiKey,
            CredentialAccount::ArkEndpoint,
            false,
        ),
        "asr" => (
            CredentialsVault::get_active_asr(),
            CredentialAccount::AsrApiKey,
            CredentialAccount::AsrEndpoint,
            true,
        ),
        _ => return Err(format!("unknown provider kind: {kind}")),
    };
    let api_key = CredentialsVault::get(api_key_account)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let base_url = CredentialsVault::get(endpoint_account)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if api_key_required && api_key.trim().is_empty() {
        return Err("apiKeyMissing".to_string());
    }
    if base_url.trim().is_empty() {
        return Err("endpointMissing".to_string());
    }
    if kind == "llm"
        && api_key.trim().is_empty()
        && llm_model_list_requires_api_key(&provider_id, &base_url)
    {
        return Err("apiKeyMissing".to_string());
    }
    let proxy_config = read_provider_proxy_config(kind, &provider_id)?;
    Ok(ProviderConfig {
        provider_id,
        base_url,
        api_key,
        proxy_config,
    })
}

fn llm_model_list_requires_api_key(provider_id: &str, base_url: &str) -> bool {
    llm_provider_default_endpoint(provider_id)
        .map(|default| same_llm_endpoint(base_url, default))
        .unwrap_or(false)
}

fn read_provider_proxy_config(
    kind: &str,
    provider_id: &str,
) -> Result<ProviderProxyConfig, String> {
    let (mode_account, url_account) = match kind {
        "llm" => (
            CredentialAccount::LlmProxyMode,
            CredentialAccount::LlmProxyUrl,
        ),
        "asr" => (
            CredentialAccount::AsrProxyMode,
            CredentialAccount::AsrProxyUrl,
        ),
        _ => return Err(format!("unknown provider kind: {kind}")),
    };
    let mode = CredentialsVault::get(mode_account).map_err(|e| e.to_string())?;
    let proxy_url = CredentialsVault::get(url_account).map_err(|e| e.to_string())?;
    ProviderProxyConfig::from_stored(provider_id, mode.as_deref(), proxy_url.as_deref())
}

async fn validate_llm_provider() -> Result<(), String> {
    let llm_thinking_enabled = PreferencesStore::new()
        .map_err(|e| e.to_string())?
        .get()
        .llm_thinking_enabled;
    if CredentialsVault::get_active_llm() == CODEX_OAUTH_PROVIDER_ID {
        let model = CredentialsVault::get(CredentialAccount::ArkModelId)
            .map_err(|e| e.to_string())?
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| CODEX_DEFAULT_MODEL.to_string());
        let provider = CodexOAuthLLMProvider::new(
            CodexOAuthConfig::new(model)
                .with_thinking_enabled(llm_thinking_enabled)
                .with_proxy_config(read_provider_proxy_config("llm", CODEX_OAUTH_PROVIDER_ID)?),
        );
        return provider
            .polish(
                "验证连接",
                PolishMode::Raw,
                &[],
                "",
                &[],
                ChineseScriptPreference::Auto,
                OutputLanguagePreference::Auto,
                None,
                &[],
            )
            .await
            .map(|_| ())
            .map_err(|e| match e {
                LLMError::InvalidResponse { status, .. } => {
                    format!("providerHttpStatus:{status}")
                }
                other => other.to_string(),
            });
    }

    let config = read_openai_provider_config("llm")?;
    let active_llm = CredentialsVault::get_active_llm();
    let model = CredentialsVault::get(CredentialAccount::ArkModelId)
        .map_err(|e| e.to_string())?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "llmModelMissing".to_string())?;
    let provider = OpenAICompatibleLLMProvider::new(
        OpenAICompatibleConfig::new(
            active_llm.clone(),
            active_llm,
            config.base_url,
            config.api_key,
            model,
        )
        .with_thinking_enabled(llm_thinking_enabled)
        .with_proxy_config(config.proxy_config),
    );
    provider
        .polish(
            "验证连接",
            PolishMode::Raw,
            &[],
            "",
            &[],
            ChineseScriptPreference::Auto,
            OutputLanguagePreference::Auto,
            None,
            &[],
        )
        .await
        .map(|_| ())
        .map_err(|e| match e {
            LLMError::InvalidResponse { status, .. } => {
                format!("providerHttpStatus:{status}")
            }
            other => other.to_string(),
        })
}

async fn validate_asr_provider() -> Result<(), String> {
    let active_asr = CredentialsVault::get_active_asr();
    if active_asr_is_keyless_for_validation(&active_asr) {
        return Ok(());
    }

    if active_asr == crate::asr::bailian::PROVIDER_ID {
        return validate_bailian_asr_provider().await;
    }

    let config = read_openai_provider_config("asr")?;
    let model = CredentialsVault::get(CredentialAccount::AsrModel)
        .map_err(|e| e.to_string())?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "asrModelMissing".to_string())?;
    validate_asr_transcription(&config, model.trim()).await
}

async fn validate_bailian_asr_provider() -> Result<(), String> {
    let api_key = CredentialsVault::get(CredentialAccount::AsrApiKey)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if api_key.trim().is_empty() {
        return Err("apiKeyMissing".to_string());
    }
    let endpoint = CredentialsVault::get(CredentialAccount::AsrEndpoint)
        .map_err(|e| e.to_string())?
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| crate::asr::bailian::DEFAULT_ENDPOINT.to_string());
    let model = CredentialsVault::get(CredentialAccount::AsrModel)
        .map_err(|e| e.to_string())?
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| crate::asr::bailian::DEFAULT_MODEL.to_string());
    let vocabulary_id = CredentialsVault::get(CredentialAccount::AsrVocabularyId)
        .map_err(|e| e.to_string())?
        .filter(|s| !s.trim().is_empty());
    let asr = std::sync::Arc::new(crate::asr::BailianRealtimeASR::new(
        crate::asr::BailianCredentials {
            api_key,
            endpoint,
            model,
            vocabulary_id,
        },
    ));
    asr.open_session().await.map_err(|e| e.to_string())?;
    crate::asr::AudioConsumer::consume_pcm_chunk(
        &*asr,
        &vec![0u8; crate::asr::bailian::TARGET_AUDIO_CHUNK_BYTES],
    );
    asr.send_last_frame().await.map_err(|e| e.to_string())?;
    asr.await_final_result()
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn active_asr_is_keyless_for_validation(provider: &str) -> bool {
    provider == crate::asr::local::PROVIDER_ID || active_foundry_asr_is_supported(provider)
}

fn active_foundry_asr_is_supported(provider: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        provider == FOUNDRY_LOCAL_PROVIDER_ID
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = provider;
        false
    }
}

async fn validate_asr_transcription(config: &ProviderConfig, model: &str) -> Result<(), String> {
    const MAX_ASR_VALIDATE_BODY_BYTES: usize = 1024 * 1024;
    let url = asr_transcriptions_url(&config.base_url)?;
    let wav = encode_wav_16k_mono_silence(250);
    let wav_part = reqwest::multipart::Part::bytes(wav)
        .file_name("listener-type-asr-check.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("请求体构建失败: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .part("file", wav_part)
        .text("model", model.to_string());
    let client = http_client_builder_with_proxy(&url, 20, &config.proxy_config)
        .build()
        .map_err(|_| "providerClientInitFailed".to_string())?;
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "providerRequestTimeout".to_string()
            } else {
                "providerNetworkError".to_string()
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("providerHttpStatus:{}", status.as_u16()));
    }
    if let Some(len) = response.content_length() {
        if len as usize > MAX_ASR_VALIDATE_BODY_BYTES {
            return Err("providerResponseTooLarge".to_string());
        }
    }
    use futures_util::StreamExt;
    let mut body = Vec::<u8>::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "providerReadResponseFailed".to_string())?;
        if body.len().saturating_add(chunk.len()) > MAX_ASR_VALIDATE_BODY_BYTES {
            return Err("providerResponseTooLarge".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    let json: Value = serde_json::from_slice(&body).map_err(|_| "asrInvalidJson".to_string())?;
    if !json.is_object() || json.get("text").is_none() {
        return Err("asrMissingTextField".to_string());
    }
    Ok(())
}

fn asr_transcriptions_url(base_url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(base_url.trim()).map_err(|_| "endpointInvalid".to_string())?;
    let host = parsed.host_str().unwrap_or_default();
    let localhost = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1";
    if parsed.scheme() != "https" && !localhost {
        return Err("endpointMustUseHttps".to_string());
    }

    // Work on the URL path only so we don't corrupt query parameters.
    let mut url = parsed.clone();
    let path = parsed.path().trim_end_matches('/');
    let next_path = if path.ends_with("/audio/transcriptions") {
        path.to_string()
    } else if path.ends_with("/audio") {
        format!("{path}/transcriptions")
    } else if let Some(prefix) = path.strip_suffix("/chat/completions") {
        format!("{prefix}/audio/transcriptions")
    } else {
        format!("{path}/audio/transcriptions")
    };
    url.set_path(&next_path);
    Ok(url.to_string())
}

fn encode_wav_16k_mono_silence(duration_ms: u32) -> Vec<u8> {
    let sample_rate: u32 = 16_000;
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let bytes_per_sample = (bits_per_sample / 8) as usize;
    let samples = (sample_rate as usize * duration_ms as usize) / 1000;
    let pcm_len = samples * bytes_per_sample;
    let data_size = pcm_len as u32;
    let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
    let block_align = num_channels * bits_per_sample / 8;
    let chunk_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + pcm_len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&chunk_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&num_channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.resize(44 + pcm_len, 0);
    wav
}

async fn fetch_provider_models(config: &ProviderConfig) -> Result<Vec<String>, String> {
    let url = models_url(&config.base_url);
    let is_gemini = is_gemini_base_url(&config.base_url);
    log::info!(
        "[provider-check] GET {url} provider={} (gemini={is_gemini})",
        config.provider_id
    );
    let client = http_client_builder_with_proxy(&config.base_url, 15, &config.proxy_config)
        .build()
        .map_err(|_| "providerClientInitFailed".to_string())?;
    let mut request = client.get(&url);
    if !config.api_key.trim().is_empty() {
        // 谷歌原生 generativelanguage.googleapis.com 不识别 Bearer Authorization,
        // 必须用 x-goog-api-key 头。其它 OpenAI 兼容 provider 仍走 Bearer。
        if is_gemini {
            request = request.header("x-goog-api-key", config.api_key.as_str());
        } else {
            request = request.header("Authorization", format!("Bearer {}", config.api_key));
        }
    }
    let response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            "providerRequestTimeout".to_string()
        } else {
            "providerNetworkError".to_string()
        }
    })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|_| "providerReadResponseFailed".to_string())?;
    if !status.is_success() {
        return Err(format!("providerHttpStatus:{}", status.as_u16()));
    }
    if is_gemini {
        parse_gemini_model_ids(&body)
    } else {
        parse_model_ids(&body)
    }
}

fn is_gemini_base_url(base_url: &str) -> bool {
    base_url.contains("generativelanguage.googleapis.com")
}

fn models_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/models") {
        return trimmed.to_string();
    }
    if let Some(prefix) = trimmed.strip_suffix("/chat/completions") {
        return format!("{prefix}/models");
    }
    format!("{trimmed}/models")
}

fn parse_model_ids(body: &str) -> Result<Vec<String>, String> {
    let json: Value =
        serde_json::from_str(body).map_err(|_| "providerInvalidModelList".to_string())?;
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "providerInvalidModelList".to_string())?;
    let mut models = data
        .iter()
        .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

/// 谷歌 v1beta/models 响应形状：`{models: [{name: "models/gemini-2.5-flash",
/// supportedGenerationMethods: ["generateContent", ...], ...}, ...]}`。
/// 与 OpenAI `{data: [{id: "..."}]}` 不兼容，所以单独解析；name 字段去掉
/// "models/" 前缀后即是 ProviderTools「拉取模型」按钮可直接写入 ark.model_id
/// 的字符串。
///
/// 过滤：只保留声明支持 `generateContent` 的模型——Google 的 model list 同时
/// 暴露 embedding (`gemini-embedding-2`)、TTS、image 等不支持
/// generateContent 的家族；用户选中那种 ID 后 polish 必失败（PR #398 pr_agent
/// 漏洞反馈）。`supportedGenerationMethods` 字段缺失时保守保留——某些 preview
/// 模型可能未暴露这个字段，宁误显示也不要把新模型挡在外面。
fn parse_gemini_model_ids(body: &str) -> Result<Vec<String>, String> {
    let json: Value =
        serde_json::from_str(body).map_err(|_| "providerInvalidModelList".to_string())?;
    let models = json
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "providerInvalidModelList".to_string())?;
    let mut ids = models
        .iter()
        .filter(|item| {
            match item
                .get("supportedGenerationMethods")
                .and_then(|v| v.as_array())
            {
                Some(methods) => methods
                    .iter()
                    .any(|m| m.as_str() == Some("generateContent")),
                None => true, // 字段缺失：保守包含
            }
        })
        .filter_map(|item| item.get("name").and_then(|n| n.as_str()))
        .map(|name| {
            name.strip_prefix("models/")
                .unwrap_or(name)
                .trim()
                .to_string()
        })
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn parse_account(s: &str) -> Result<CredentialAccount, String> {
    match s {
        "volcengine.app_key" => Ok(CredentialAccount::VolcengineAppKey),
        "volcengine.access_key" => Ok(CredentialAccount::VolcengineAccessKey),
        "volcengine.resource_id" => Ok(CredentialAccount::VolcengineResourceId),
        "ark.api_key" => Ok(CredentialAccount::ArkApiKey),
        "ark.model_id" => Ok(CredentialAccount::ArkModelId),
        "ark.endpoint" => Ok(CredentialAccount::ArkEndpoint),
        "llm.proxy_mode" => Ok(CredentialAccount::LlmProxyMode),
        "llm.proxy_url" => Ok(CredentialAccount::LlmProxyUrl),
        "asr.api_key" => Ok(CredentialAccount::AsrApiKey),
        "asr.endpoint" => Ok(CredentialAccount::AsrEndpoint),
        "asr.model" => Ok(CredentialAccount::AsrModel),
        "asr.vocabulary_id" => Ok(CredentialAccount::AsrVocabularyId),
        "asr.proxy_mode" => Ok(CredentialAccount::AsrProxyMode),
        "asr.proxy_url" => Ok(CredentialAccount::AsrProxyUrl),
        _ => Err(format!("unknown account: {s}")),
    }
}

// ─────────────────────────── history ───────────────────────────

#[tauri::command]
pub fn list_history(coord: CoordinatorState<'_>) -> Result<Vec<DictationSession>, String> {
    coord.history().list().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_history_entry(coord: CoordinatorState<'_>, id: String) -> Result<(), String> {
    coord.history().delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_history(coord: CoordinatorState<'_>) -> Result<(), String> {
    coord.history().clear().map_err(|e| e.to_string())
}

/// 读取某次会话的原始麦克风 wav 字节流。仅当用户开过
/// `prefs.record_audio_for_debug` 并且这条 session 是开关打开后录的，才会有文件。
/// 文件名规约：`<data_dir>/recordings/<session_id>.wav`，与 DictationSession.id 同名。
///
/// 路径校验：session_id **必须**严格匹配 UUID-v4 字面（36 字符 = 8-4-4-4-12 + 4 个 `-`，
/// 内容仅 ASCII 十六进制 + `-`）。白名单胜过黑名单——绝对路径前缀、Windows ADS、
/// 百分号编码、NUL 字节都不在合法字符集里，挡掉所有 Path::join 越界的可能。
/// session_id 在仓库内由 `Uuid::new_v4()` 生成 (`dictation.rs:1531`)，前端只会回传
/// 自己列出的合法 id，但 IPC = boundary，按 boundary 规则严格校验。
///
/// async fs：单条 5 分钟 wav 约 9.6MB，同步 `std::fs::read` 会阻塞 Tauri IPC 主循环。
/// 改 `tokio::fs::read` 后让出线程给其它 IPC。
#[tauri::command]
pub async fn read_audio_recording(session_id: String) -> Result<Vec<u8>, String> {
    if !is_valid_session_id(&session_id) {
        return Err("invalid session id".into());
    }
    let path =
        crate::persistence::recording_path_for_session(&session_id).map_err(|e| e.to_string())?;
    if !path.exists() {
        return Err("recording not found".into());
    }
    // TOCTOU 兜底：exists() 通过到 read 之间文件可能被 prune（条数 cap / retention
    // 清理 / 用户手动删）。把 NotFound 标准化成跟 exists() 失败同样的错误字符串，
    // 前端单条 'recording not found' catch 就能稳定隐藏按钮，不依赖本地化 OS 错误。
    tokio::fs::read(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "recording not found".into()
        } else {
            format!("read wav failed: {e}")
        }
    })
}

/// UUID-v4 字面校验：36 字符 + 5 段 `-` 分隔（8-4-4-4-12）+ 仅 ASCII 十六进制。
/// 用于 install/detail/like —— pack_id 来自远端服务器，必须是它发的 UUID。
fn is_valid_session_id(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        let is_dash_position = matches!(i, 8 | 13 | 18 | 23);
        if is_dash_position {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// 本地 style pack id 白名单：`[A-Za-z0-9._-]`、长度 1..=128。
/// 上传走本地 id（`builtin.light` / 用户自取 slug / UUID 都可），不是远端 UUID。
/// 仍阻断 `..` / `/` / `\` / 控制字符，避免 path traversal 进临时 zip 文件名。
fn is_valid_local_pack_id(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

// ─────────────────────────── vocab ───────────────────────────

#[tauri::command]
pub fn list_vocab(coord: CoordinatorState<'_>) -> Result<Vec<DictionaryEntry>, String> {
    coord.vocab().list().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_vocab(
    coord: CoordinatorState<'_>,
    phrase: String,
    note: Option<String>,
) -> Result<DictionaryEntry, String> {
    coord.vocab().add(phrase, note).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_vocab(coord: CoordinatorState<'_>, id: String) -> Result<(), String> {
    coord.vocab().remove(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_vocab_enabled(
    coord: CoordinatorState<'_>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    coord
        .vocab()
        .set_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_correction_rules(coord: CoordinatorState<'_>) -> Result<Vec<CorrectionRule>, String> {
    coord.correction_rules().list().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_correction_rule(
    coord: CoordinatorState<'_>,
    pattern: String,
    replacement: String,
) -> Result<CorrectionRule, String> {
    coord
        .correction_rules()
        .add(pattern, replacement)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_correction_rule(coord: CoordinatorState<'_>, id: String) -> Result<(), String> {
    coord
        .correction_rules()
        .remove(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_correction_rule_enabled(
    coord: CoordinatorState<'_>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    coord
        .correction_rules()
        .set_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_vocab_presets() -> Result<VocabPresetStore, String> {
    crate::persistence::list_vocab_presets().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_vocab_presets(store: VocabPresetStore) -> Result<(), String> {
    crate::persistence::save_vocab_presets(&store).map_err(|e| e.to_string())
}

// ─────────────────────────── dictation lifecycle ───────────────────────────

#[tauri::command]
pub async fn start_dictation(coord: CoordinatorState<'_>) -> Result<(), String> {
    coord.start_dictation().await
}

#[tauri::command]
pub async fn stop_dictation(coord: CoordinatorState<'_>) -> Result<(), String> {
    coord.stop_dictation().await
}

#[tauri::command]
pub async fn submit_embedded_audio_notifications(
    coord: CoordinatorState<'_>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_notifications(notifications)
        .await
}

#[tauri::command]
pub async fn submit_embedded_audio_streaming_notifications(
    coord: CoordinatorState<'_>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_streaming_notifications(notifications)
        .await
}

#[tauri::command]
pub async fn submit_embedded_audio_file(
    coord: CoordinatorState<'_>,
    path: String,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_file(PathBuf::from(path), format)
        .await
}

#[tauri::command]
pub async fn submit_embedded_audio_streaming_file(
    coord: CoordinatorState<'_>,
    path: String,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_streaming_file(PathBuf::from(path), format)
        .await
}

#[tauri::command]
pub async fn submit_embedded_audio_ble_once(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord.submit_embedded_audio_ble_once(timeout_ms).await
}

#[tauri::command]
pub async fn probe_embedded_audio_ble_subscription(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    coord
        .probe_embedded_audio_ble_subscription(timeout_ms)
        .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedBleRepairResult {
    pub recovered: bool,
    pub user_action_required: bool,
    pub open_bluetooth_settings: bool,
    pub recovery_action: EmbeddedBleRecoveryAction,
    pub message: String,
    pub failure: Option<crate::embedded_ble::BleFailureClassification>,
    pub unpair_result: Option<crate::embedded_ble::BleDeviceUnpairResult>,
    pub runtime: EmbeddedBleRuntimeStatus,
    pub firmware: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EmbeddedBleRecoveryAction {
    None,
    Reconnected,
    WaitForAutomaticRecovery,
    RePairRequired,
    BluetoothSettingsRequired,
    DiagnosticsRequired,
}

fn embedded_ble_repair_failure_action(
    failure: &crate::embedded_ble::BleFailureClassification,
) -> (bool, bool) {
    let open_bluetooth_settings = matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::DeviceMissing
            | crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::StaleGattService
            | crate::embedded_ble::BleFailureKind::CccdProtocolError
            | crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded
            | crate::embedded_ble::BleFailureKind::AccessDenied
    );
    let user_action_required = open_bluetooth_settings || !failure.automatic_recovery;
    (user_action_required, open_bluetooth_settings)
}

fn embedded_ble_recovery_action_for_failure(
    failure: &crate::embedded_ble::BleFailureClassification,
) -> EmbeddedBleRecoveryAction {
    match failure.kind {
        crate::embedded_ble::BleFailureKind::MissingPairing
        | crate::embedded_ble::BleFailureKind::StaleGattService
        | crate::embedded_ble::BleFailureKind::CccdProtocolError => {
            EmbeddedBleRecoveryAction::RePairRequired
        }
        crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded
        | crate::embedded_ble::BleFailureKind::AccessDenied
        | crate::embedded_ble::BleFailureKind::DeviceMissing => {
            EmbeddedBleRecoveryAction::BluetoothSettingsRequired
        }
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
        | crate::embedded_ble::BleFailureKind::PairedButDisconnected
        | crate::embedded_ble::BleFailureKind::BackgroundListenerContention
        | crate::embedded_ble::BleFailureKind::OtaRebootWindow => {
            EmbeddedBleRecoveryAction::WaitForAutomaticRecovery
        }
        crate::embedded_ble::BleFailureKind::DeviceAsleep => {
            EmbeddedBleRecoveryAction::WaitForAutomaticRecovery
        }
        crate::embedded_ble::BleFailureKind::MissingDisFirmwareRevision
        | crate::embedded_ble::BleFailureKind::UnsupportedPlatform
        | crate::embedded_ble::BleFailureKind::Unknown => {
            EmbeddedBleRecoveryAction::DiagnosticsRequired
        }
    }
}

fn should_attempt_embedded_ble_auto_unpair(
    failure: &crate::embedded_ble::BleFailureClassification,
) -> bool {
    matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::StaleGattService
            | crate::embedded_ble::BleFailureKind::CccdProtocolError
    )
}

fn runtime_suggests_embedded_ble_auto_unpair(
    repair_error: &str,
    listener_last_error: Option<&str>,
    wake_recovery: &EmbeddedBleWakeRecoverySnapshot,
) -> bool {
    if wake_recovery.reconnect_attempts < 3 {
        return false;
    }

    if !matches!(
        wake_recovery.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Failed
            | EmbeddedBleNotifySubscriptionState::Opening
            | EmbeddedBleNotifySubscriptionState::Lost
            | EmbeddedBleNotifySubscriptionState::Unknown
    ) {
        return false;
    }

    let combined = [
        Some(repair_error),
        listener_last_error,
        wake_recovery.recent_disconnect_reason.as_deref(),
        Some(wake_recovery.user_guidance.as_str()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();

    let low_power_idle = wake_recovery.usb_powered == Some(false)
        && runtime_error_text_suggests_low_power_idle(&combined);
    let notify_or_gatt_failure = combined.contains("cccd")
        || combined.contains("notify write")
        || combined.contains("notify subscription")
        || combined.contains("gatt session still not active")
        || combined.contains("gattsessionstatus(0)")
        || combined.contains("bluetoothconnectionstatus(0)");
    let timeout_like = combined.contains("timed out")
        || combined.contains("timeout")
        || combined.contains("not active")
        || combined.contains("not recover")
        || combined.contains("did not recover")
        || combined.contains("disconnected");

    notify_or_gatt_failure && timeout_like && !low_power_idle
}

fn runtime_error_text_suggests_low_power_idle(combined: &str) -> bool {
    let reason_546 = combined.contains("reason=546")
        || combined.contains("reason: 546")
        || combined.contains("reason 546");
    let idle_label = combined.contains("low-power idle")
        || combined.contains("low power idle")
        || combined.contains("idle disconnect");
    let transport_not_ready =
        combined.contains("transport_not_ready") || combined.contains("transport not ready");
    let link_loss = combined.contains("connection status changed")
        || combined.contains("gatt session status changed")
        || combined.contains("disconnected");

    reason_546 || idle_label || (transport_not_ready && link_loss)
}

fn embedded_ble_recovery_message(
    failure: &crate::embedded_ble::BleFailureClassification,
    unpair_result: Option<&crate::embedded_ble::BleDeviceUnpairResult>,
) -> String {
    if let Some(unpair) = unpair_result {
        return match unpair.status {
            crate::embedded_ble::BleDeviceUnpairStatus::Removed => {
                "旧的 Listener 蓝牙配对已清理。请在打开的 Windows 蓝牙设置里重新配对 Listener，Type 会自动恢复。".to_string()
            }
            crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean
            | crate::embedded_ble::BleDeviceUnpairStatus::NotFound => {
                "Type 没找到可自动清理的旧配对。请在打开的 Windows 蓝牙设置里配对 Listener，Type 会自动恢复。".to_string()
            }
            crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction => {
                "Windows 需要你确认移除 Listener。请在打开的蓝牙设置里删除 Listener 后重新配对，Type 会自动恢复。".to_string()
            }
        };
    }
    match embedded_ble_recovery_action_for_failure(failure) {
        EmbeddedBleRecoveryAction::WaitForAutomaticRecovery => {
            "Listener Type 正在自动恢复连接。请保持设备唤醒，稍等片刻。".to_string()
        }
        EmbeddedBleRecoveryAction::BluetoothSettingsRequired => {
            "请在打开的 Windows 蓝牙设置里确认 Listener 已连接；如果仍失败，请删除后重新配对。"
                .to_string()
        }
        EmbeddedBleRecoveryAction::DiagnosticsRequired => {
            "Type 不能自动恢复这个状态。请导出诊断包给支持人员。".to_string()
        }
        EmbeddedBleRecoveryAction::RePairRequired => {
            "请重新配对 Listener；Type 会继续检测并自动恢复。".to_string()
        }
        EmbeddedBleRecoveryAction::None | EmbeddedBleRecoveryAction::Reconnected => {
            failure.user_action.to_string()
        }
    }
}

async fn embedded_ble_runtime_and_firmware(
    coord: &Coordinator,
) -> Result<
    (
        EmbeddedBleRuntimeStatus,
        crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    ),
    String,
> {
    let firmware =
        tauri::async_runtime::spawn_blocking(crate::embedded_ble::firmware_ota_device_snapshot)
            .await
            .map_err(|err| format!("Listener BLE repair snapshot task failed: {err}"))?;
    coord.record_embedded_ble_firmware_power_snapshot(&firmware, "runtime_and_firmware");
    let runtime = EmbeddedBleRuntimeStatus {
        background_listener_disabled_by_env: std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
            .ok()
            .is_some_and(|value| value == "1"),
        background_listener_active: coord.embedded_ble_listener_active(),
        background_listener_ready: coord.embedded_ble_listener_ready(),
        background_listener_generation: coord.embedded_ble_listener_generation(),
        background_listener_last_error: coord.embedded_ble_listener_last_error(),
        wake_recovery: coord.embedded_ble_wake_recovery_snapshot(),
    };
    Ok((runtime, firmware))
}

#[tauri::command]
pub async fn repair_embedded_ble_connection(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<EmbeddedBleRepairResult, String> {
    let repair = coord.repair_embedded_ble_connection(timeout_ms).await;
    let (runtime, firmware) = embedded_ble_runtime_and_firmware(&coord).await?;

    match repair {
        Ok(snapshot) => Ok(EmbeddedBleRepairResult {
            recovered: true,
            user_action_required: false,
            open_bluetooth_settings: false,
            recovery_action: EmbeddedBleRecoveryAction::Reconnected,
            message: snapshot.user_guidance,
            failure: None,
            unpair_result: None,
            runtime,
            firmware,
        }),
        Err(err) => {
            let failure = crate::embedded_ble::classify_ble_failure(&err);
            let (user_action_required, open_bluetooth_settings) =
                embedded_ble_repair_failure_action(&failure);
            let recovery_action = embedded_ble_recovery_action_for_failure(&failure);
            Ok(EmbeddedBleRepairResult {
                recovered: false,
                user_action_required,
                open_bluetooth_settings,
                recovery_action,
                message: embedded_ble_recovery_message(&failure, None),
                failure: Some(failure),
                unpair_result: None,
                runtime,
                firmware,
            })
        }
    }
}

#[tauri::command]
pub async fn recover_embedded_ble_device(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<EmbeddedBleRepairResult, String> {
    let repair = coord.repair_embedded_ble_connection(timeout_ms).await;
    match repair {
        Ok(snapshot) => {
            let (runtime, firmware) = embedded_ble_runtime_and_firmware(&coord).await?;
            Ok(EmbeddedBleRepairResult {
                recovered: true,
                user_action_required: false,
                open_bluetooth_settings: false,
                recovery_action: EmbeddedBleRecoveryAction::Reconnected,
                message: snapshot.user_guidance,
                failure: None,
                unpair_result: None,
                runtime,
                firmware,
            })
        }
        Err(err) => {
            let failure = crate::embedded_ble::classify_ble_failure(&err);
            let (mut user_action_required, mut open_bluetooth_settings) =
                embedded_ble_repair_failure_action(&failure);
            let mut recovery_action = embedded_ble_recovery_action_for_failure(&failure);
            let mut unpair_result = None;
            let listener_last_error = coord.embedded_ble_listener_last_error();
            let wake_recovery = coord.embedded_ble_wake_recovery_snapshot();
            let runtime_requests_auto_unpair = runtime_suggests_embedded_ble_auto_unpair(
                &err,
                listener_last_error.as_deref(),
                &wake_recovery,
            );

            if should_attempt_embedded_ble_auto_unpair(&failure) || runtime_requests_auto_unpair {
                let cleanup_wait_timeout = Duration::from_secs(45);
                log::info!(
                    "[embedded-ble] one-click recovery waiting for active Listener capture to stop before device cleanup timeout_ms={}",
                    cleanup_wait_timeout.as_millis()
                );
                let capture_stopped = coord
                    .pause_embedded_ble_listener_for_recovery_cleanup(cleanup_wait_timeout)
                    .await;
                log::info!(
                    "[embedded-ble] one-click recovery capture stop before device cleanup stopped={capture_stopped}"
                );
                log::info!(
                    "[embedded-ble] one-click recovery attempting automatic Listener unpair failure_kind={:?} runtime_escalated={runtime_requests_auto_unpair} reconnect_attempts={} notify_state={:?}",
                    failure.kind,
                    wake_recovery.reconnect_attempts,
                    wake_recovery.notify_subscription_state,
                );
                let unpair = tauri::async_runtime::spawn_blocking(
                    crate::embedded_ble::unpair_listener_devices,
                )
                .await
                .map_err(|err| format!("Listener BLE automatic unpair task failed: {err}"))?;
                log::info!(
                    "[embedded-ble] one-click recovery automatic Listener unpair result status={:?} matched={} removed={} already_clean={} failed={} user_action={}",
                    unpair.status,
                    unpair.matched_devices,
                    unpair.unpaired_devices,
                    unpair.already_unpaired_devices,
                    unpair.failed_devices,
                    unpair.needs_user_action,
                );
                let retry_after_cleanup =
                    unpair.status == crate::embedded_ble::BleDeviceUnpairStatus::Removed;
                user_action_required = true;
                open_bluetooth_settings = true;
                recovery_action = EmbeddedBleRecoveryAction::RePairRequired;
                unpair_result = Some(unpair.clone());
                coord.refresh_embedded_ble_listener();
                if retry_after_cleanup {
                    log::info!(
                        "[embedded-ble] one-click recovery retrying Listener connection after stale device cleanup"
                    );
                    match coord.repair_embedded_ble_connection(timeout_ms).await {
                        Ok(snapshot) => {
                            let (runtime, firmware) =
                                embedded_ble_runtime_and_firmware(&coord).await?;
                            return Ok(EmbeddedBleRepairResult {
                                recovered: true,
                                user_action_required: false,
                                open_bluetooth_settings: false,
                                recovery_action: EmbeddedBleRecoveryAction::Reconnected,
                                message: snapshot.user_guidance,
                                failure: None,
                                unpair_result,
                                runtime,
                                firmware,
                            });
                        }
                        Err(retry_err) => {
                            log::warn!(
                                "[embedded-ble] one-click recovery reconnect after stale device cleanup failed: {retry_err}"
                            );
                        }
                    }
                }
            }

            let (runtime, firmware) = embedded_ble_runtime_and_firmware(&coord).await?;
            Ok(EmbeddedBleRepairResult {
                recovered: false,
                user_action_required,
                open_bluetooth_settings,
                recovery_action,
                message: embedded_ble_recovery_message(&failure, unpair_result.as_ref()),
                failure: Some(failure),
                unpair_result,
                runtime,
                firmware,
            })
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedBleRuntimeStatus {
    pub background_listener_disabled_by_env: bool,
    pub background_listener_active: bool,
    pub background_listener_ready: bool,
    pub background_listener_generation: u64,
    pub background_listener_last_error: Option<String>,
    pub wake_recovery: EmbeddedBleWakeRecoverySnapshot,
}

#[tauri::command]
pub fn get_embedded_ble_runtime_status(coord: CoordinatorState<'_>) -> EmbeddedBleRuntimeStatus {
    EmbeddedBleRuntimeStatus {
        background_listener_disabled_by_env: std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
            .ok()
            .is_some_and(|value| value == "1"),
        background_listener_active: coord.embedded_ble_listener_active(),
        background_listener_ready: coord.embedded_ble_listener_ready(),
        background_listener_generation: coord.embedded_ble_listener_generation(),
        background_listener_last_error: coord.embedded_ble_listener_last_error(),
        wake_recovery: coord.embedded_ble_wake_recovery_snapshot(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettingsSnapshot {
    schema: &'static str,
    connected: bool,
    write_supported: bool,
    source: &'static str,
    status_led_brightness_percent: u8,
    key_led_brightness_percent: u8,
    knob_led_brightness_percent: u8,
    edge_led_brightness_percent: u8,
    led_zone_brightness_supported: bool,
    low_power_idle_minutes: u32,
    plugged_low_power_idle_minutes: u32,
    battery_low_power_idle_minutes: u32,
    plugged_low_power_enabled: bool,
    plugged_auto_shutdown_ms: u32,
    battery_auto_shutdown_ms: u32,
    knob_rotation_action: String,
    ble_name: String,
    ble_name_pending_restart: bool,
    active_power_source: &'static str,
    battery_percent: Option<u8>,
    detail: Option<String>,
    last_updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettingsUpdateRequest {
    status_led_brightness_percent: u8,
    key_led_brightness_percent: u8,
    knob_led_brightness_percent: u8,
    edge_led_brightness_percent: u8,
    plugged_low_power_idle_minutes: u32,
    battery_low_power_idle_minutes: u32,
    plugged_low_power_enabled: bool,
    plugged_auto_shutdown_minutes: u32,
    battery_auto_shutdown_minutes: u32,
    ble_name: String,
}

const DEVICE_SETTINGS_SCHEMA: &str = "listener.device_settings.v1";
const DEVICE_SETTINGS_DEFAULT_PLUGGED_AUTO_SHUTDOWN_MS: u32 = 0;
const DEVICE_SETTINGS_DEFAULT_BATTERY_AUTO_SHUTDOWN_MS: u32 =
    DEFAULT_DEVICE_BATTERY_AUTO_SHUTDOWN_MINUTES * 60 * 1000;
const DEVICE_SETTINGS_MIN_AUTO_SHUTDOWN_MINUTES: u32 = 0;
const DEVICE_SETTINGS_MAX_AUTO_SHUTDOWN_MINUTES: u32 = 1440;
const DEVICE_SETTINGS_DEFAULT_BLE_NAME: &str = "listener";
const DEVICE_SETTINGS_BLE_WRITE_TIMEOUT: Duration = Duration::from_secs(4);
const DEVICE_SETTINGS_BLE_TASK_TIMEOUT: Duration = Duration::from_secs(10);
const DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES: usize = 63;

async fn read_device_settings_snapshot_from_firmware() -> Result<DeviceSettingsSnapshot, String> {
    let status = run_device_settings_blocking("readback", || {
        crate::embedded_ble::read_device_settings_status(DEVICE_SETTINGS_BLE_WRITE_TIMEOUT)
    })
    .await?;
    Ok(device_settings_snapshot_from_status(status))
}

async fn run_device_settings_blocking<T, F>(label: &'static str, task: F) -> Result<T, String>
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

#[tauri::command]
pub async fn get_device_settings() -> Result<DeviceSettingsSnapshot, String> {
    match read_device_settings_snapshot_from_firmware().await {
        Ok(snapshot) => return Ok(snapshot),
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

fn device_settings_readback_unavailable_detail(error: &str) -> String {
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

fn device_settings_sent_but_readback_unavailable_detail(error: &str) -> String {
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

#[tauri::command]
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
    let commands = device_settings_update_commands(
        &request,
        led_zone_brightness_supported,
        plugged_low_power_enabled,
    )?;
    for command in commands {
        let command_for_error = command.clone();
        run_device_settings_blocking("write", move || {
            crate::embedded_ble::send_device_settings_command(
                &command,
                DEVICE_SETTINGS_BLE_WRITE_TIMEOUT,
            )
        })
        .await
        .map_err(|err| format!("{err}; command={command_for_error}"))?;
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
        prefs.device_plugged_low_power_enabled = plugged_low_power_enabled;
        prefs.device_battery_auto_shutdown_minutes = request.battery_auto_shutdown_minutes;
        prefs.device_ble_name = request.ble_name.clone();
        persist_settings(&*coord, prefs.clone())?;
        emit_prefs_changed(&app, &prefs);
    }
    match read_device_settings_snapshot_from_firmware().await {
        Ok(snapshot) => Ok(snapshot),
        Err(readback_error) => {
            let mut snapshot = device_settings_snapshot_from_request(
                &request,
                previous_snapshot,
                plugged_low_power_enabled,
            );
            snapshot.detail = Some(device_settings_sent_but_readback_unavailable_detail(
                &readback_error,
            ));
            Ok(snapshot)
        }
    }
}

fn device_settings_snapshot_from_status(
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
        low_power_idle_minutes: status.low_power_idle_minutes,
        plugged_low_power_idle_minutes: status.plugged_low_power_idle_minutes,
        battery_low_power_idle_minutes: status.battery_low_power_idle_minutes,
        plugged_low_power_enabled: status.plugged_low_power_enabled,
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

fn ui_knob_rotation_action_from_firmware(action: &str) -> String {
    match action {
        "system_volume" | "systemVolume" => "systemVolume".to_string(),
        "screen_brightness" | "screenBrightness" => "screenBrightness".to_string(),
        "disabled" => "disabled".to_string(),
        other => other.to_string(),
    }
}

fn device_settings_snapshot_from_device(
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
        status_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        key_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        knob_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        edge_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        led_zone_brightness_supported: false,
        low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        battery_low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_enabled: true,
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

fn device_settings_snapshot_from_request(
    request: &DeviceSettingsUpdateRequest,
    previous: Option<DeviceSettingsSnapshot>,
    plugged_low_power_enabled: bool,
) -> DeviceSettingsSnapshot {
    let mut snapshot = previous.unwrap_or(DeviceSettingsSnapshot {
        schema: DEVICE_SETTINGS_SCHEMA,
        connected: true,
        write_supported: true,
        source: "lastKnown",
        status_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        key_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        knob_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        edge_led_brightness_percent: crate::types::DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
        led_zone_brightness_supported: false,
        low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        battery_low_power_idle_minutes: DEFAULT_DEVICE_LOW_POWER_IDLE_MINUTES,
        plugged_low_power_enabled: true,
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

fn device_settings_update_commands(
    request: &DeviceSettingsUpdateRequest,
    led_zone_brightness_supported: bool,
    plugged_low_power_enabled: bool,
) -> Result<Vec<String>, String> {
    let mut commands = Vec::new();
    if led_zone_brightness_supported {
        commands.push(format!(
            "DEVICE:SET led_status={} led_key={}",
            request.status_led_brightness_percent, request.key_led_brightness_percent
        ));
        commands.push(format!(
            "DEVICE:SET led_ec11={} led_edge={}",
            request.knob_led_brightness_percent, request.edge_led_brightness_percent
        ));
    }
    commands.extend([
        format!(
            "DEVICE:SET plugged_low_power_idle_minutes={}",
            request.plugged_low_power_idle_minutes
        ),
        format!(
            "DEVICE:SET battery_low_power_idle_minutes={}",
            request.battery_low_power_idle_minutes
        ),
        format!(
            "DEVICE:SET plugged_low_power_enabled={}",
            if plugged_low_power_enabled { 1 } else { 0 }
        ),
        "DEVICE:SET plugged_auto_shutdown_minutes=off".to_string(),
        format!(
            "DEVICE:SET battery_auto_shutdown_minutes={}",
            request.battery_auto_shutdown_minutes
        ),
        format!("DEVICE:SET ble_name={}", request.ble_name),
    ]);
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

fn validate_device_settings_request(request: &DeviceSettingsUpdateRequest) -> Result<(), String> {
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

fn validate_device_settings_ble_name(name: &str) -> Result<(), String> {
    if !device_ble_name_is_valid(name) {
        return Err("BLE name must be 1-29 printable ASCII characters without spaces, quotes, semicolon, equals sign, or backslash.".to_string());
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaPreflightSnapshot {
    recording_active: bool,
    dictation_phase: String,
    device: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
}

#[tauri::command]
pub async fn get_firmware_ota_preflight_snapshot(
    coord: CoordinatorState<'_>,
) -> Result<FirmwareOtaPreflightSnapshot, String> {
    let phase = coord.dictation_phase_for_cli();
    let device =
        tauri::async_runtime::spawn_blocking(crate::embedded_ble::firmware_ota_device_snapshot)
            .await
            .map_err(|err| format!("Listener BLE OTA preflight task failed: {err}"))?;
    Ok(FirmwareOtaPreflightSnapshot {
        recording_active: phase != SessionPhase::Idle,
        dictation_phase: format!("{phase:?}"),
        device,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaBleTransferResult {
    bytes_transferred: usize,
    confirmed_version: Option<String>,
    transport: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaPackagePayload {
    manifest_text: String,
    firmware_bytes: Vec<u8>,
    source_label: String,
}

const FIRMWARE_OTA_CONFIRM_TIMEOUT: Duration = Duration::from_secs(45);
const FIRMWARE_OTA_CONFIRM_INTERVAL: Duration = Duration::from_secs(2);
const FIRMWARE_OTA_PACKAGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

fn normalize_firmware_ota_version(value: &str) -> String {
    value.trim().trim_start_matches('v').to_ascii_lowercase()
}

fn firmware_ota_versions_match(confirmed: &str, expected: &str) -> bool {
    let confirmed = normalize_firmware_ota_version(confirmed);
    let expected = normalize_firmware_ota_version(expected);
    !confirmed.is_empty() && !expected.is_empty() && confirmed == expected
}

fn firmware_ota_snapshot_version(
    snapshot: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
) -> Option<String> {
    snapshot
        .firmware_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn confirm_firmware_ota_version(expected_version: &str) -> Option<String> {
    if normalize_firmware_ota_version(expected_version).is_empty() || expected_version == "unknown"
    {
        return None;
    }

    let deadline = Instant::now() + FIRMWARE_OTA_CONFIRM_TIMEOUT;
    let mut last_seen_version = None;

    loop {
        let snapshot =
            tauri::async_runtime::spawn_blocking(crate::embedded_ble::firmware_ota_device_snapshot)
                .await
                .ok();
        if let Some(snapshot) = snapshot {
            if let Some(version) = firmware_ota_snapshot_version(&snapshot) {
                if firmware_ota_versions_match(&version, expected_version) {
                    return Some(version);
                }
                last_seen_version = Some(version);
            }
        }

        if Instant::now() >= deadline {
            return last_seen_version;
        }
        tokio::time::sleep(FIRMWARE_OTA_CONFIRM_INTERVAL).await;
    }
}

#[tauri::command]
pub fn load_firmware_ota_package(path: String) -> Result<FirmwareOtaPackagePayload, String> {
    let path = PathBuf::from(path);
    if path.is_dir() {
        return load_firmware_ota_package_dir(&path);
    }
    let is_zip = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if is_zip {
        return load_firmware_ota_package_zip(&path);
    }
    Err("OTA package must be a .zip file or a package directory.".to_string())
}

fn load_firmware_ota_package_dir(path: &Path) -> Result<FirmwareOtaPackagePayload, String> {
    let manifest_path = path.join("ota_manifest.json");
    let firmware_path = path.join("firmware_ota.bin");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| format!("Failed to read {}: {err}", manifest_path.display()))?;
    let firmware_bytes = read_limited_file(&firmware_path)?;
    Ok(FirmwareOtaPackagePayload {
        manifest_text,
        firmware_bytes,
        source_label: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("OTA package directory")
            .to_string(),
    })
}

fn load_firmware_ota_package_zip(path: &Path) -> Result<FirmwareOtaPackagePayload, String> {
    let file =
        File::open(path).map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|err| format!("Invalid OTA zip package: {err}"))?;
    let manifest_text =
        read_zip_entry_by_basename(&mut archive, "ota_manifest.json").and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|err| format!("ota_manifest.json is not valid UTF-8: {err}"))
        })?;
    let firmware_bytes = read_zip_entry_by_basename(&mut archive, "firmware_ota.bin")?;
    Ok(FirmwareOtaPackagePayload {
        manifest_text,
        firmware_bytes,
        source_label: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("OTA zip package")
            .to_string(),
    })
}

fn read_limited_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|err| format!("Failed to stat {}: {err}", path.display()))?;
    if metadata.len() > FIRMWARE_OTA_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{} is too large for an OTA package ({} bytes > {} bytes).",
            path.display(),
            metadata.len(),
            FIRMWARE_OTA_PACKAGE_MAX_BYTES
        ));
    }
    std::fs::read(path).map_err(|err| format!("Failed to read {}: {err}", path.display()))
}

fn read_zip_entry_by_basename<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    basename: &str,
) -> Result<Vec<u8>, String> {
    let mut match_index = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|err| format!("Failed to read OTA zip entry #{index}: {err}"))?;
        if entry.is_dir() {
            continue;
        }
        let normalized = entry.name().replace('\\', "/");
        if normalized
            .rsplit('/')
            .next()
            .map(|name| name == basename)
            .unwrap_or(false)
        {
            match_index = Some(index);
            break;
        }
    }

    let index = match_index.ok_or_else(|| format!("OTA zip is missing {basename}."))?;
    let mut entry = archive
        .by_index(index)
        .map_err(|err| format!("Failed to open OTA zip entry {basename}: {err}"))?;
    if entry.size() > FIRMWARE_OTA_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{basename} is too large for an OTA package ({} bytes > {} bytes).",
            entry.size(),
            FIRMWARE_OTA_PACKAGE_MAX_BYTES
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size().min(usize::MAX as u64) as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Failed to read OTA zip entry {basename}: {err}"))?;
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WiredFirmwarePackageKind {
    Factory,
}

impl WiredFirmwarePackageKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Factory => "factory",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WiredFirmwareTargetKind {
    Esp32S3,
}

impl WiredFirmwareTargetKind {
    fn supports_boot_repair(self) -> bool {
        true
    }
}

#[derive(Clone, Debug, Deserialize)]
struct FactoryFirmwareManifest {
    schema_version: u64,
    project: String,
    version: String,
    target: String,
    #[serde(default)]
    git_commit: String,
    #[serde(default)]
    flash: Option<FactoryFirmwareFlash>,
    artifacts: Vec<FactoryFirmwareArtifact>,
}

#[derive(Clone, Debug, Deserialize)]
struct FactoryFirmwareFlash {
    #[serde(default)]
    baud: Option<Value>,
    #[serde(default)]
    partition_table: Vec<FactoryFirmwarePartition>,
}

#[derive(Clone, Debug, Deserialize)]
struct FactoryFirmwarePartition {
    name: String,
    offset: String,
    size: String,
}

#[derive(Clone, Debug, Deserialize)]
struct FactoryFirmwareArtifact {
    role: String,
    file: String,
    offset: String,
    #[serde(default)]
    format: Option<String>,
    size_bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareArtifactInfo {
    role: String,
    file: String,
    offset: String,
    size_bytes: u64,
    sha256: String,
}

impl From<&FactoryFirmwareArtifact> for WiredFirmwareArtifactInfo {
    fn from(value: &FactoryFirmwareArtifact) -> Self {
        Self {
            role: value.role.clone(),
            file: value.file.clone(),
            offset: value.offset.clone(),
            size_bytes: value.size_bytes,
            sha256: value.sha256.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwarePackagePayload {
    kind: String,
    project: String,
    version: String,
    target: String,
    git_commit: Option<String>,
    source_label: String,
    artifacts: Vec<WiredFirmwareArtifactInfo>,
    supports_full_flash: bool,
    supports_boot_repair: bool,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareSerialPort {
    port: String,
    label: String,
    is_likely_esp32: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareFlashResult {
    action: String,
    kind: String,
    port: String,
    version: String,
    log: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareProgressPayload {
    action: String,
    stage: String,
    port: Option<String>,
    version: Option<String>,
    current_role: Option<String>,
    current_file: Option<String>,
    bytes_written: u64,
    bytes_total: u64,
    current_bytes: u64,
    current_total: u64,
    percent: u8,
    message: String,
}

#[derive(Clone, Debug)]
struct LoadedWiredFirmwarePackage {
    kind: WiredFirmwarePackageKind,
    project: String,
    version: String,
    target: String,
    git_commit: Option<String>,
    source_label: String,
    artifacts: Vec<FactoryFirmwareArtifact>,
    files: BTreeMap<String, Vec<u8>>,
    manifest_file_name: &'static str,
    manifest_text: String,
    otadata_region: Option<(String, String)>,
    notes: Vec<String>,
}

impl LoadedWiredFirmwarePackage {
    fn to_payload(&self) -> WiredFirmwarePackagePayload {
        let target_kind = wired_firmware_target_kind(&self.target).ok();
        WiredFirmwarePackagePayload {
            kind: self.kind.as_str().to_string(),
            project: self.project.clone(),
            version: self.version.clone(),
            target: self.target.clone(),
            git_commit: self.git_commit.clone(),
            source_label: self.source_label.clone(),
            artifacts: self
                .artifacts
                .iter()
                .map(WiredFirmwareArtifactInfo::from)
                .collect(),
            supports_full_flash: true,
            supports_boot_repair: target_kind
                .map(WiredFirmwareTargetKind::supports_boot_repair)
                .unwrap_or(false),
            notes: self.notes.clone(),
        }
    }
}

const WIRED_FIRMWARE_PACKAGE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const WIRED_OTADATA_OFFSET: &str = "0xf000";
const WIRED_OTADATA_SIZE: &str = "0x2000";
const WIRED_DEFAULT_BAUD: u32 = 460_800;
const WIRED_BOOT_REPAIR_BAUDS: &[u32] = &[115_200, 57_600, 9_600];
const WIRED_FLASH_MODE: FlashMode = FlashMode::Dio;
const WIRED_FLASH_FREQUENCY: FlashFrequency = FlashFrequency::_80Mhz;
const WIRED_FLASH_SIZE: FlashSize = FlashSize::_16Mb;
const WIRED_FULL_FLASH_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const WIRED_BOOT_REPAIR_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const WIRED_FLASH_CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(120);
const WIRED_FLASH_SERIAL_IO_TIMEOUT: Duration = Duration::from_secs(5);

struct PreparedWiredFlashArtifact {
    role: String,
    file: String,
    offset: u32,
    bytes: Vec<u8>,
    patch_report: Option<EspImagePatchReport>,
}

#[derive(Clone, Copy, Debug, Default)]
struct EspImagePatchReport {
    changed: bool,
    digest_recalculated: bool,
}

#[derive(Clone, Debug)]
struct WiredFlashProgressArtifact {
    role: String,
    file: String,
    offset: u32,
    size_bytes: u64,
}

#[derive(Clone, Debug)]
struct WiredFlashProgressCurrent {
    artifact: WiredFlashProgressArtifact,
    total_units: usize,
    current_units: usize,
}

struct WiredFlashProgressCallbacks {
    app: Option<AppHandle>,
    action: String,
    version: String,
    port: String,
    artifacts: Vec<WiredFlashProgressArtifact>,
    total_bytes: u64,
    completed_bytes: u64,
    current: Option<WiredFlashProgressCurrent>,
    stage_start_percent: u8,
    stage_end_percent: u8,
    last_emitted_percent: Option<u8>,
    last_emitted_bytes: u64,
}

impl WiredFlashProgressCallbacks {
    fn new(
        app: Option<&AppHandle>,
        action: &str,
        version: &str,
        port: &str,
        artifacts: Vec<WiredFlashProgressArtifact>,
        stage_start_percent: u8,
        stage_end_percent: u8,
    ) -> Self {
        let total_bytes = artifacts
            .iter()
            .map(|artifact| artifact.size_bytes)
            .sum::<u64>()
            .max(1);
        Self {
            app: app.cloned(),
            action: action.to_string(),
            version: version.to_string(),
            port: port.to_string(),
            artifacts,
            total_bytes,
            completed_bytes: 0,
            current: None,
            stage_start_percent,
            stage_end_percent: stage_end_percent.max(stage_start_percent),
            last_emitted_percent: None,
            last_emitted_bytes: 0,
        }
    }

    fn emit_current(&mut self, force: bool) {
        let Some(current) = self.current.as_ref() else {
            return;
        };
        let current_bytes = scaled_progress_bytes(
            current.artifact.size_bytes,
            current.current_units,
            current.total_units,
        );
        let bytes_written = self
            .completed_bytes
            .saturating_add(current_bytes)
            .min(self.total_bytes);
        let percent = wired_progress_percent_for_range(
            bytes_written,
            self.total_bytes,
            self.stage_start_percent,
            self.stage_end_percent,
        );
        if !force
            && self.last_emitted_percent == Some(percent)
            && bytes_written.saturating_sub(self.last_emitted_bytes) < 32 * 1024
        {
            return;
        }
        self.last_emitted_percent = Some(percent);
        self.last_emitted_bytes = bytes_written;
        emit_wired_firmware_progress(
            self.app.as_ref(),
            WiredFirmwareProgressPayload {
                action: self.action.clone(),
                stage: "writing".to_string(),
                port: Some(self.port.clone()),
                version: Some(self.version.clone()),
                current_role: Some(current.artifact.role.clone()),
                current_file: Some(current.artifact.file.clone()),
                bytes_written,
                bytes_total: self.total_bytes,
                current_bytes,
                current_total: current.artifact.size_bytes,
                percent,
                message: format!("Writing {}", current.artifact.file),
            },
        );
    }
}

impl ProgressCallbacks for WiredFlashProgressCallbacks {
    fn init(&mut self, addr: u32, total: usize) {
        let artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.offset == addr)
            .cloned()
            .unwrap_or_else(|| WiredFlashProgressArtifact {
                role: "firmware".to_string(),
                file: format!("0x{addr:x}"),
                offset: addr,
                size_bytes: 0,
            });
        self.current = Some(WiredFlashProgressCurrent {
            artifact,
            total_units: total.max(1),
            current_units: 0,
        });
        self.emit_current(true);
    }

    fn update(&mut self, current: usize) {
        if let Some(progress) = self.current.as_mut() {
            progress.current_units = current.min(progress.total_units);
        }
        self.emit_current(false);
    }

    fn finish(&mut self) {
        if let Some(progress) = self.current.as_mut() {
            progress.current_units = progress.total_units;
        }
        self.emit_current(true);
        if let Some(progress) = self.current.take() {
            self.completed_bytes = self
                .completed_bytes
                .saturating_add(progress.artifact.size_bytes)
                .min(self.total_bytes);
        }
    }
}

fn emit_wired_firmware_progress(app: Option<&AppHandle>, payload: WiredFirmwareProgressPayload) {
    if let Some(app) = app {
        let _ = app.emit("wired-firmware:progress", payload);
    }
}

fn emit_wired_firmware_stage(
    app: Option<&AppHandle>,
    action: &str,
    stage: &str,
    version: Option<&str>,
    port: Option<&str>,
    percent: u8,
    message: impl Into<String>,
) {
    emit_wired_firmware_progress(
        app,
        WiredFirmwareProgressPayload {
            action: action.to_string(),
            stage: stage.to_string(),
            port: port.map(ToOwned::to_owned),
            version: version.map(ToOwned::to_owned),
            current_role: None,
            current_file: None,
            bytes_written: 0,
            bytes_total: 0,
            current_bytes: 0,
            current_total: 0,
            percent: percent.min(100),
            message: message.into(),
        },
    );
}

fn scaled_progress_bytes(total_bytes: u64, current_units: usize, total_units: usize) -> u64 {
    if total_units == 0 {
        return total_bytes;
    }
    ((total_bytes as u128)
        .saturating_mul(current_units as u128)
        .checked_div(total_units as u128)
        .unwrap_or(0)
        .min(total_bytes as u128)) as u64
}

fn wired_progress_percent_for_range(
    bytes_written: u64,
    bytes_total: u64,
    start_percent: u8,
    end_percent: u8,
) -> u8 {
    if bytes_total == 0 {
        return end_percent.min(100);
    }
    let span = u16::from(end_percent.saturating_sub(start_percent));
    let delta = ((bytes_written as u128)
        .saturating_mul(span as u128)
        .checked_div(bytes_total as u128)
        .unwrap_or(0)
        .min(span as u128)) as u8;
    start_percent.saturating_add(delta).min(100)
}

#[tauri::command]
pub fn list_wired_firmware_ports() -> Result<Vec<WiredFirmwareSerialPort>, String> {
    Ok(list_wired_firmware_ports_internal())
}

#[tauri::command]
pub fn load_wired_firmware_package(path: String) -> Result<WiredFirmwarePackagePayload, String> {
    load_wired_firmware_package_internal(&PathBuf::from(path)).map(|loaded| loaded.to_payload())
}

#[tauri::command]
pub async fn flash_wired_firmware_package(
    app: AppHandle,
    path: String,
    port: Option<String>,
    baud: Option<u32>,
    preserve_ota_data: Option<bool>,
) -> Result<WiredFirmwareFlashResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_wired_firmware_flash_with_progress(
            &PathBuf::from(path),
            port.as_deref(),
            baud,
            preserve_ota_data.unwrap_or(false),
            Some(app),
        )
    })
    .await
    .map_err(|err| format!("Wired firmware flash task failed: {err}"))?
}

#[tauri::command]
pub async fn repair_wired_firmware_bootloader(
    app: AppHandle,
    path: String,
    port: Option<String>,
    baud: Option<u32>,
) -> Result<WiredFirmwareFlashResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_wired_bootloader_repair_with_progress(
            &PathBuf::from(path),
            port.as_deref(),
            baud,
            Some(app),
        )
    })
    .await
    .map_err(|err| format!("Wired bootloader repair task failed: {err}"))?
}

fn list_wired_firmware_ports_internal() -> Vec<WiredFirmwareSerialPort> {
    let mut ports = serialport::available_ports().unwrap_or_default();
    ports.sort_by(|a, b| a.port_name.cmp(&b.port_name));
    ports
        .into_iter()
        .map(|port| {
            let mut label = port.port_name.clone();
            let mut is_likely_esp32 = false;
            if let serialport::SerialPortType::UsbPort(info) = port.port_type {
                is_likely_esp32 = info.vid == 0x303a || info.pid == 0x1001;
                let details = [info.manufacturer, info.product]
                    .into_iter()
                    .flatten()
                    .filter(|value| !value.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !details.is_empty() {
                    label = format!("{} · {}", port.port_name, details);
                }
            }
            WiredFirmwareSerialPort {
                port: port.port_name,
                label,
                is_likely_esp32,
            }
        })
        .collect()
}

fn resolve_wired_flash_port(requested: Option<&str>) -> Result<String, String> {
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    if let Some(value) = requested {
        if !value.eq_ignore_ascii_case("COMx") {
            return Ok(value.to_string());
        }
    }

    let ports = list_wired_firmware_ports_internal();
    if ports.is_empty() {
        return Err(
            "No serial ports were detected. Connect the Listener ESP32-S3 USB port and retry."
                .to_string(),
        );
    }
    let esp32_ports = ports
        .iter()
        .filter(|port| port.is_likely_esp32)
        .collect::<Vec<_>>();
    if esp32_ports.len() == 1 {
        return Ok(esp32_ports[0].port.clone());
    }
    if ports.len() == 1 {
        return Ok(ports[0].port.clone());
    }

    let summary = ports
        .iter()
        .map(|port| port.label.clone())
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "Multiple serial ports were detected. Choose the Listener COM port explicitly. Ports: {summary}"
    ))
}

fn load_wired_firmware_package_internal(path: &Path) -> Result<LoadedWiredFirmwarePackage, String> {
    if path.is_dir() {
        if path.join("manifest.json").is_file() {
            return load_factory_firmware_package_dir(path);
        }
        if let Some(factory_dir) = find_factory_package_dir(path) {
            return load_factory_firmware_package_dir(&factory_dir);
        }
        return Err(format!(
            "Wired firmware flashing requires a factory package directory with manifest.json, or a directory containing factory/<package>/manifest.json: {}",
            path.display()
        ));
    }

    let is_zip = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if !is_zip {
        return Err("Wired firmware package must be a .zip file or package directory.".to_string());
    }

    if zip_entry_exists(path, "manifest.json")? {
        return load_factory_firmware_package_zip(path);
    }
    if zip_entry_exists(path, "ota_manifest.json")? {
        return Err("Wired firmware flashing uses the factory package format, not a Bluetooth OTA zip. Select a factory zip or package directory containing manifest.json.".to_string());
    }
    Err("Wired firmware zip is missing factory manifest.json.".to_string())
}

fn find_factory_package_dir(path: &Path) -> Option<PathBuf> {
    if path.join("manifest.json").is_file() {
        return Some(path.to_path_buf());
    }

    let factory_root = path.join("factory");
    let entries = std::fs::read_dir(factory_root).ok()?;
    let mut candidates = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| candidate.join("manifest.json").is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

fn source_label_for_path(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn load_factory_firmware_package_dir(path: &Path) -> Result<LoadedWiredFirmwarePackage, String> {
    let manifest_path = path.join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| format!("Failed to read {}: {err}", manifest_path.display()))?;
    let manifest = parse_factory_firmware_manifest(&manifest_text)?;
    let mut files = BTreeMap::new();
    for artifact in &manifest.artifacts {
        validate_factory_artifact(artifact)?;
        let bytes = read_limited_wired_file(&path.join(&artifact.file))?;
        validate_factory_artifact_bytes(artifact, &bytes)?;
        files.insert(artifact.file.clone(), bytes);
    }
    loaded_factory_package_from_manifest(
        manifest,
        manifest_text,
        source_label_for_path(path, "factory firmware package"),
        files,
    )
}

fn load_factory_firmware_package_zip(path: &Path) -> Result<LoadedWiredFirmwarePackage, String> {
    let manifest_bytes = read_zip_entry_by_basename_limited(path, "manifest.json")?
        .ok_or_else(|| "Factory firmware zip is missing manifest.json.".to_string())?;
    let manifest_text = String::from_utf8(manifest_bytes)
        .map_err(|err| format!("manifest.json is not valid UTF-8: {err}"))?;
    let manifest = parse_factory_firmware_manifest(&manifest_text)?;
    let mut files = BTreeMap::new();
    for artifact in &manifest.artifacts {
        validate_factory_artifact(artifact)?;
        let bytes = read_zip_entry_by_basename_limited(path, &artifact.file)?
            .ok_or_else(|| format!("Factory firmware zip is missing {}.", artifact.file))?;
        validate_factory_artifact_bytes(artifact, &bytes)?;
        files.insert(artifact.file.clone(), bytes);
    }
    loaded_factory_package_from_manifest(
        manifest,
        manifest_text,
        source_label_for_path(path, "factory firmware zip"),
        files,
    )
}

fn parse_factory_firmware_manifest(text: &str) -> Result<FactoryFirmwareManifest, String> {
    let manifest: FactoryFirmwareManifest =
        serde_json::from_str(text).map_err(|err| format!("manifest.json is invalid: {err}"))?;
    if manifest.schema_version != 1 {
        return Err(format!(
            "Factory manifest schema_version must be 1, got {}.",
            manifest.schema_version
        ));
    }
    let _ = wired_firmware_target_kind(&manifest.target)?;
    if manifest.project != "voice-keyboard-firmware" {
        return Err(format!(
            "Factory manifest project must be voice-keyboard-firmware for ESP32-S3, got {}.",
            manifest.project
        ));
    }
    Ok(manifest)
}

fn loaded_factory_package_from_manifest(
    manifest: FactoryFirmwareManifest,
    manifest_text: String,
    source_label: String,
    files: BTreeMap<String, Vec<u8>>,
) -> Result<LoadedWiredFirmwarePackage, String> {
    let _ = wired_firmware_target_kind(&manifest.target)?;
    for role in ["bootloader", "partition_table", "app"] {
        require_artifact(&manifest.artifacts, role)?;
    }
    let otadata_region = manifest.flash.as_ref().and_then(|flash| {
        flash
            .partition_table
            .iter()
            .find(|entry| entry.name == "otadata")
    });
    let otadata_region = match otadata_region {
        Some(entry) => Some((
            normalize_esptool_region_arg(&entry.offset, "otadata offset", true)?,
            normalize_esptool_region_arg(&entry.size, "otadata size", false)?,
        )),
        None => Some((
            WIRED_OTADATA_OFFSET.to_string(),
            WIRED_OTADATA_SIZE.to_string(),
        )),
    };
    let notes = vec![
        "Factory package: wired flash writes bootloader, partition table, and app.".to_string(),
        "Boot repair is available and writes only bootloader.bin at 0x0.".to_string(),
    ];
    Ok(LoadedWiredFirmwarePackage {
        kind: WiredFirmwarePackageKind::Factory,
        project: manifest.project,
        version: manifest.version,
        target: manifest.target,
        git_commit: (!manifest.git_commit.trim().is_empty()).then_some(manifest.git_commit),
        source_label,
        artifacts: manifest.artifacts,
        files,
        manifest_file_name: "manifest.json",
        manifest_text,
        otadata_region,
        notes,
    })
}

fn validate_factory_artifact(artifact: &FactoryFirmwareArtifact) -> Result<(), String> {
    if artifact.role.trim().is_empty() {
        return Err("Factory artifact role must not be empty.".to_string());
    }
    if artifact.offset.trim().is_empty() {
        return Err(format!(
            "Factory artifact {} offset is empty.",
            artifact.role
        ));
    }
    validate_package_file_name(&artifact.file)?;
    if artifact.size_bytes == 0 {
        return Err(format!("Factory artifact {} is empty.", artifact.file));
    }
    if !is_sha256_hex(&artifact.sha256) {
        return Err(format!(
            "Factory artifact {} has invalid SHA256.",
            artifact.file
        ));
    }
    Ok(())
}

fn validate_package_file_name(file_name: &str) -> Result<(), String> {
    if file_name.trim().is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.chars().any(char::is_control)
    {
        return Err(format!("Unsafe firmware package file name: {file_name}"));
    }
    Ok(())
}

fn validate_factory_artifact_bytes(
    artifact: &FactoryFirmwareArtifact,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.len() as u64 != artifact.size_bytes {
        return Err(format!(
            "{} size mismatch: manifest={} actual={}.",
            artifact.file,
            artifact.size_bytes,
            bytes.len()
        ));
    }
    let actual_sha256 = crate::firmware_ota::sha256_hex(bytes);
    if actual_sha256 != artifact.sha256.to_ascii_lowercase() {
        return Err(format!(
            "{} SHA256 does not match manifest.json.",
            artifact.file
        ));
    }
    Ok(())
}

fn require_artifact<'a>(
    artifacts: &'a [FactoryFirmwareArtifact],
    role: &str,
) -> Result<&'a FactoryFirmwareArtifact, String> {
    artifacts
        .iter()
        .find(|artifact| artifact.role == role)
        .ok_or_else(|| format!("Firmware package is missing {role} artifact."))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn read_limited_wired_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|err| format!("Failed to stat {}: {err}", path.display()))?;
    if metadata.len() > WIRED_FIRMWARE_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{} is too large for a firmware package ({} bytes > {} bytes).",
            path.display(),
            metadata.len(),
            WIRED_FIRMWARE_PACKAGE_MAX_BYTES
        ));
    }
    std::fs::read(path).map_err(|err| format!("Failed to read {}: {err}", path.display()))
}

fn zip_entry_exists(path: &Path, basename: &str) -> Result<bool, String> {
    read_zip_entry_by_basename_limited(path, basename).map(|entry| entry.is_some())
}

fn read_zip_entry_by_basename_limited(
    path: &Path,
    basename: &str,
) -> Result<Option<Vec<u8>>, String> {
    let file =
        File::open(path).map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|err| format!("Invalid firmware zip package: {err}"))?;
    let mut match_index = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|err| format!("Failed to read firmware zip entry #{index}: {err}"))?;
        if entry.is_dir() {
            continue;
        }
        let normalized = entry.name().replace('\\', "/");
        if normalized
            .rsplit('/')
            .next()
            .map(|name| name == basename)
            .unwrap_or(false)
        {
            match_index = Some(index);
            break;
        }
    }

    let Some(index) = match_index else {
        return Ok(None);
    };
    let mut entry = archive
        .by_index(index)
        .map_err(|err| format!("Failed to open firmware zip entry {basename}: {err}"))?;
    if entry.size() > WIRED_FIRMWARE_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{basename} is too large for a firmware package ({} bytes > {} bytes).",
            entry.size(),
            WIRED_FIRMWARE_PACKAGE_MAX_BYTES
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size().min(usize::MAX as u64) as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Failed to read firmware zip entry {basename}: {err}"))?;
    Ok(Some(bytes))
}

pub(crate) fn run_wired_firmware_flash(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
    preserve_ota_data: bool,
) -> Result<WiredFirmwareFlashResult, String> {
    run_wired_firmware_flash_with_progress(path, requested_port, baud, preserve_ota_data, None)
}

fn run_wired_firmware_flash_with_progress(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
    preserve_ota_data: bool,
    progress_app: Option<AppHandle>,
) -> Result<WiredFirmwareFlashResult, String> {
    let progress_app_ref = progress_app.as_ref();
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "loading",
        None,
        requested_port,
        1,
        "Loading wired firmware package",
    );
    let loaded = load_wired_firmware_package_internal(path)?;
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "packageLoaded",
        Some(&loaded.version),
        requested_port,
        4,
        "Wired firmware package loaded",
    );

    let chip = wired_target_chip(&loaded.target)?;
    let port_hint = resolve_wired_flash_port(requested_port)?;
    let baud = baud
        .or_else(|| package_manifest_baud(&loaded))
        .unwrap_or(WIRED_DEFAULT_BAUD);
    let mut log = String::new();
    log.push_str("> Listener Type built-in wired firmware flasher\n");
    log.push_str("No ESP-IDF, IDF_PATH, Python, or esptool.py environment is required.\n");
    log.push_str(&format!(
        "Package: {} ({})\nTarget: {}\nBaud: {}\n",
        loaded.source_label, loaded.version, loaded.target, baud
    ));

    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "connecting",
        Some(&loaded.version),
        Some(&port_hint),
        8,
        "Connecting to ESP32-S3 serial flasher",
    );
    let (mut flasher, port, connect_log) = connect_builtin_esp_flasher_with_wait(
        Some(&port_hint),
        baud,
        chip,
        true,
        true,
        ResetBeforeOperation::DefaultReset,
        ResetAfterOperation::HardReset,
        WIRED_FULL_FLASH_CONNECT_TIMEOUT,
        false,
    )?;
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "connected",
        Some(&loaded.version),
        Some(&port),
        16,
        "Connected to ESP32-S3 flasher",
    );
    log.push_str(&connect_log);
    flasher.set_flash_size(WIRED_FLASH_SIZE);
    match flasher.device_info() {
        Ok(info) => log.push_str(&format_wired_device_info(&info)),
        Err(err) => log.push_str(&format!("Device info read skipped: {err}\n")),
    }
    log.push_str(&format!(
        "Flash config: mode={:?}, frequency={:?}, size={:?}\n",
        WIRED_FLASH_MODE, WIRED_FLASH_FREQUENCY, WIRED_FLASH_SIZE
    ));

    if !preserve_ota_data {
        if let Some((offset, size)) = loaded.otadata_region.as_ref() {
            let offset = parse_flash_u32_arg(offset, "otadata offset", true)?;
            let size = parse_flash_u32_arg(size, "otadata size", false)?;
            emit_wired_firmware_stage(
                progress_app_ref,
                "flash",
                "erasing",
                Some(&loaded.version),
                Some(&port),
                22,
                "Erasing OTA state partition",
            );
            flasher
                .erase_region(offset, size)
                .map_err(|err| format!("Failed to erase otadata region: {err}"))?;
            log.push_str(&format!(
                "Erased otadata region at 0x{offset:x}, size 0x{size:x}.\n"
            ));
            emit_wired_firmware_stage(
                progress_app_ref,
                "flash",
                "erased",
                Some(&loaded.version),
                Some(&port),
                26,
                "OTA state partition erased",
            );
        }
    } else {
        log.push_str("Preserved otadata region.\n");
        emit_wired_firmware_stage(
            progress_app_ref,
            "flash",
            "erased",
            Some(&loaded.version),
            Some(&port),
            26,
            "OTA state partition preserved",
        );
    }

    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "preparing",
        Some(&loaded.version),
        Some(&port),
        28,
        "Preparing firmware images",
    );
    let prepared = prepare_wired_flash_artifacts(&loaded, chip)?;
    let segments = prepared
        .iter()
        .map(|artifact| RomSegment {
            addr: artifact.offset,
            data: Cow::Borrowed(artifact.bytes.as_slice()),
        })
        .collect::<Vec<_>>();
    if progress_app_ref.is_some() {
        let progress_artifacts = wired_progress_artifacts_from_prepared(&prepared);
        let mut progress = WiredFlashProgressCallbacks::new(
            progress_app_ref,
            "flash",
            &loaded.version,
            &port,
            progress_artifacts,
            28,
            98,
        );
        flasher
            .write_bins_to_flash(&segments, Some(&mut progress))
            .map_err(|err| format!("Failed to write wired firmware image: {err}"))?;
    } else {
        flasher
            .write_bins_to_flash(&segments, None)
            .map_err(|err| format!("Failed to write wired firmware image: {err}"))?;
    }
    for artifact in &prepared {
        log.push_str(&format!(
            "Wrote {} ({}) at 0x{:x}, {} bytes",
            artifact.role,
            artifact.file,
            artifact.offset,
            artifact.bytes.len()
        ));
        if let Some(report) = artifact.patch_report {
            if report.changed {
                log.push_str("; ESP image header patched to wired flash config");
                if report.digest_recalculated {
                    log.push_str("; SHA256 digest recalculated");
                }
            }
        }
        log.push_str(".\n");
    }
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "finalizing",
        Some(&loaded.version),
        Some(&port),
        99,
        "Finalizing wired firmware flash",
    );
    log.push_str("Wired factory flash completed and verified by espflash.\n");

    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "done",
        Some(&loaded.version),
        Some(&port),
        100,
        "Wired factory flash completed",
    );
    Ok(WiredFirmwareFlashResult {
        action: "flash".to_string(),
        kind: loaded.kind.as_str().to_string(),
        port,
        version: loaded.version,
        log: trim_command_output(&log, 24_000),
    })
}

pub(crate) fn run_wired_bootloader_repair(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
) -> Result<WiredFirmwareFlashResult, String> {
    run_wired_bootloader_repair_with_progress(path, requested_port, baud, None)
}

fn run_wired_bootloader_repair_with_progress(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
    progress_app: Option<AppHandle>,
) -> Result<WiredFirmwareFlashResult, String> {
    let progress_app_ref = progress_app.as_ref();
    emit_wired_firmware_stage(
        progress_app_ref,
        "bootloaderRepair",
        "loading",
        None,
        requested_port,
        1,
        "Loading wired firmware package",
    );
    let loaded = load_wired_firmware_package_internal(path)?;
    let chip = wired_target_chip(&loaded.target)?;
    let port_hint = normalize_requested_wired_port(requested_port);
    let bootloader = require_artifact(&loaded.artifacts, "bootloader")?;
    let bootloader_offset = parse_flash_u32_arg(&bootloader.offset, "bootloader offset", true)?;
    let bootloader_bytes = loaded
        .files
        .get(&bootloader.file)
        .ok_or_else(|| format!("Factory artifact file is missing: {}", bootloader.file))?;
    let (bootloader_bytes, patch_report) =
        prepare_esp_image_for_wired_flash("bootloader", bootloader_bytes, chip)?;
    emit_wired_firmware_stage(
        progress_app_ref,
        "bootloaderRepair",
        "packageLoaded",
        Some(&loaded.version),
        port_hint.as_deref(),
        4,
        "Bootloader image loaded",
    );
    let bauds = baud
        .map(|value| vec![value])
        .unwrap_or_else(|| WIRED_BOOT_REPAIR_BAUDS.to_vec());
    let reset_modes = [ResetBeforeOperation::DefaultReset];
    let mut log = String::new();
    log.push_str("> Listener Type built-in bootloader repair\n");
    log.push_str("No ESP-IDF, IDF_PATH, Python, or esptool.py environment is required.\n");
    log.push_str(
        "Repair strategy: poll for the COM port, use default reset to enter the ESP ROM loader, and write bootloader.bin in the same session.\n",
    );
    let mut failures = Vec::new();

    for baud in bauds {
        for before in reset_modes {
            emit_wired_firmware_stage(
                progress_app_ref,
                "bootloaderRepair",
                "connecting",
                Some(&loaded.version),
                port_hint.as_deref(),
                8,
                "Waiting for ESP32-S3 bootloader repair window",
            );
            match connect_builtin_esp_flasher_with_wait(
                port_hint.as_deref(),
                baud,
                chip,
                false,
                true,
                before,
                ResetAfterOperation::HardReset,
                WIRED_BOOT_REPAIR_CONNECT_TIMEOUT,
                true,
            ) {
                Ok((mut flasher, port, connect_log)) => {
                    log.push_str(&connect_log);
                    flasher.set_flash_size(WIRED_FLASH_SIZE);
                    emit_wired_firmware_stage(
                        progress_app_ref,
                        "bootloaderRepair",
                        "connected",
                        Some(&loaded.version),
                        Some(&port),
                        28,
                        "Connected to ESP32-S3 bootloader repair window",
                    );
                    log.push_str(&format!(
                        "Writing bootloader immediately at 0x{bootloader_offset:x}, {} bytes.\n",
                        bootloader_bytes.len()
                    ));
                    if patch_report.changed {
                        log.push_str("ESP image header patched to wired flash config");
                        if patch_report.digest_recalculated {
                            log.push_str("; SHA256 digest recalculated");
                        }
                        log.push_str(".\n");
                    }
                    let write_result = if progress_app_ref.is_some() {
                        let mut progress = WiredFlashProgressCallbacks::new(
                            progress_app_ref,
                            "bootloaderRepair",
                            &loaded.version,
                            &port,
                            vec![WiredFlashProgressArtifact {
                                role: bootloader.role.clone(),
                                file: bootloader.file.clone(),
                                offset: bootloader_offset,
                                size_bytes: bootloader_bytes.len() as u64,
                            }],
                            32,
                            96,
                        );
                        flasher.write_bin_to_flash(
                            bootloader_offset,
                            &bootloader_bytes,
                            Some(&mut progress),
                        )
                    } else {
                        flasher.write_bin_to_flash(bootloader_offset, &bootloader_bytes, None)
                    };
                    match write_result {
                        Ok(()) => {
                            emit_wired_firmware_stage(
                                progress_app_ref,
                                "bootloaderRepair",
                                "done",
                                Some(&loaded.version),
                                Some(&port),
                                100,
                                "Bootloader repair completed",
                            );
                            log.push_str("Bootloader repair completed and verified by espflash.\n");
                            return Ok(WiredFirmwareFlashResult {
                                action: "bootloaderRepair".to_string(),
                                kind: loaded.kind.as_str().to_string(),
                                port,
                                version: loaded.version,
                                log: trim_command_output(&log, 24_000),
                            });
                        }
                        Err(err) => failures.push(format!(
                            "baud {baud} before={before:?} bootloader write failed: {err}"
                        )),
                    }
                }
                Err(err) => failures.push(format!(
                    "baud {baud} before={before:?} connect failed: {err}"
                )),
            }
        }
    }

    Err(format!(
        "Bootloader repair failed after polling the COM port and trying all repair modes: {}",
        failures.join("; ")
    ))
}

fn package_manifest_baud(package: &LoadedWiredFirmwarePackage) -> Option<u32> {
    let manifest = serde_json::from_str::<FactoryFirmwareManifest>(&package.manifest_text).ok()?;
    manifest
        .flash
        .as_ref()
        .and_then(|flash| flash.baud.as_ref())
        .and_then(parse_baud_value)
}

fn parse_baud_value(value: &Value) -> Option<u32> {
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).ok();
    }
    value
        .as_str()
        .and_then(|text| text.trim().parse::<u32>().ok())
}

fn wired_target_chip(target: &str) -> Result<Chip, String> {
    match wired_firmware_target_kind(target)? {
        WiredFirmwareTargetKind::Esp32S3 => Ok(Chip::Esp32s3),
    }
}

fn wired_firmware_target_kind(target: &str) -> Result<WiredFirmwareTargetKind, String> {
    let normalized = target
        .trim()
        .to_ascii_lowercase()
        .replace('-', "")
        .replace('_', "");
    match normalized.as_str() {
        "esp32s3" => Ok(WiredFirmwareTargetKind::Esp32S3),
        _ => Err(format!(
            "Wired firmware flashing supports ESP32-S3 Listener packages; package target is {target}."
        )),
    }
}

fn prepare_wired_flash_artifacts(
    package: &LoadedWiredFirmwarePackage,
    chip: Chip,
) -> Result<Vec<PreparedWiredFlashArtifact>, String> {
    let mut prepared = Vec::new();
    for role in ["bootloader", "partition_table", "app"] {
        let artifact = require_artifact(&package.artifacts, role)?;
        let offset = parse_flash_u32_arg(&artifact.offset, &format!("{role} offset"), true)?;
        let bytes = package
            .files
            .get(&artifact.file)
            .ok_or_else(|| format!("Factory artifact file is missing: {}", artifact.file))?;
        let (bytes, patch_report) = if matches!(role, "bootloader" | "app") {
            let (bytes, report) = prepare_esp_image_for_wired_flash(role, bytes, chip)?;
            (bytes, Some(report))
        } else {
            (bytes.clone(), None)
        };
        prepared.push(PreparedWiredFlashArtifact {
            role: artifact.role.clone(),
            file: artifact.file.clone(),
            offset,
            bytes,
            patch_report,
        });
    }
    Ok(prepared)
}

fn wired_progress_artifacts_from_prepared(
    prepared: &[PreparedWiredFlashArtifact],
) -> Vec<WiredFlashProgressArtifact> {
    prepared
        .iter()
        .map(|artifact| WiredFlashProgressArtifact {
            role: artifact.role.clone(),
            file: artifact.file.clone(),
            offset: artifact.offset,
            size_bytes: artifact.bytes.len() as u64,
        })
        .collect()
}

fn prepare_esp_image_for_wired_flash(
    role: &str,
    bytes: &[u8],
    chip: Chip,
) -> Result<(Vec<u8>, EspImagePatchReport), String> {
    const ESP_IMAGE_MAGIC: u8 = 0xe9;
    const ESP_IMAGE_APPEND_DIGEST_OFFSET: usize = 23;
    const ESP_IMAGE_DIGEST_LEN: usize = 32;

    if bytes.len() <= ESP_IMAGE_APPEND_DIGEST_OFFSET {
        return Err(format!(
            "Factory artifact {role} is too small to be an ESP image."
        ));
    }
    if bytes[0] != ESP_IMAGE_MAGIC {
        return Err(format!(
            "Factory artifact {role} is not an ESP image; expected magic 0xe9."
        ));
    }

    let mode = WIRED_FLASH_MODE as u8;
    let size = WIRED_FLASH_SIZE
        .encode_flash_size()
        .map_err(|err| format!("Unsupported wired flash size: {err}"))?;
    let frequency = WIRED_FLASH_FREQUENCY
        .encode_flash_frequency(chip)
        .map_err(|err| format!("Unsupported wired flash frequency: {err}"))?;
    let flash_config = (size << 4) | frequency;

    let mut patched = bytes.to_vec();
    let changed = patched[2] != mode || patched[3] != flash_config;
    let mut digest_recalculated = false;
    if changed {
        patched[2] = mode;
        patched[3] = flash_config;
        if patched[ESP_IMAGE_APPEND_DIGEST_OFFSET] == 1 {
            if patched.len() <= ESP_IMAGE_DIGEST_LEN {
                return Err(format!(
                    "Factory artifact {role} declares a SHA256 digest but is too small to contain one."
                ));
            }
            let digest_start = patched.len() - ESP_IMAGE_DIGEST_LEN;
            let digest = sha256_digest_bytes(&patched[..digest_start]);
            patched[digest_start..].copy_from_slice(&digest);
            digest_recalculated = true;
        }
    }

    Ok((
        patched,
        EspImagePatchReport {
            changed,
            digest_recalculated,
        },
    ))
}

fn sha256_digest_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn normalize_esptool_region_arg(
    value: &str,
    field_name: &str,
    allow_zero: bool,
) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("Factory manifest {field_name} is empty."));
    }
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        let parsed = u64::from_str_radix(hex, 16).map_err(|_| {
            format!("Factory manifest {field_name} must be a number, hex value, or IDF size unit.")
        })?;
        if parsed == 0 && !allow_zero {
            return Err(format!(
                "Factory manifest {field_name} must be greater than 0."
            ));
        }
        return Ok(format!("0x{parsed:x}"));
    }
    if let Ok(parsed) = trimmed.parse::<u64>() {
        if parsed == 0 && !allow_zero {
            return Err(format!(
                "Factory manifest {field_name} must be greater than 0."
            ));
        }
        return Ok(parsed.to_string());
    }

    let upper = trimmed.to_ascii_uppercase();
    let (number, multiplier) = if let Some(number) = upper.strip_suffix("KB") {
        (number, 1024_u64)
    } else if let Some(number) = upper.strip_suffix('K') {
        (number, 1024_u64)
    } else if let Some(number) = upper.strip_suffix("MB") {
        (number, 1024_u64 * 1024)
    } else if let Some(number) = upper.strip_suffix('M') {
        (number, 1024_u64 * 1024)
    } else {
        return Err(format!(
            "Factory manifest {field_name} must be a number, hex value, or IDF size unit."
        ));
    };
    let count = number.trim().parse::<u64>().map_err(|_| {
        format!("Factory manifest {field_name} has invalid IDF size unit: {trimmed}.")
    })?;
    let parsed = count
        .checked_mul(multiplier)
        .ok_or_else(|| format!("Factory manifest {field_name} is too large: {trimmed}."))?;
    if parsed == 0 && !allow_zero {
        return Err(format!(
            "Factory manifest {field_name} must be greater than 0."
        ));
    }
    Ok(parsed.to_string())
}

fn parse_flash_u32_arg(value: &str, field_name: &str, allow_zero: bool) -> Result<u32, String> {
    let normalized = normalize_esptool_region_arg(value, field_name, allow_zero)?;
    let parsed = if let Some(hex) = normalized
        .strip_prefix("0x")
        .or_else(|| normalized.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).map_err(|_| {
            format!("Factory manifest {field_name} must be a number, hex value, or IDF size unit.")
        })?
    } else {
        normalized.parse::<u64>().map_err(|_| {
            format!("Factory manifest {field_name} must be a number, hex value, or IDF size unit.")
        })?
    };
    u32::try_from(parsed)
        .map_err(|_| format!("Factory manifest {field_name} is too large: {normalized}."))
}

fn normalize_requested_wired_port(requested: Option<&str>) -> Option<String> {
    requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !value.eq_ignore_ascii_case("COMx"))
        .map(ToOwned::to_owned)
}

fn connect_builtin_esp_flasher_with_wait(
    requested_port: Option<&str>,
    baud: u32,
    chip: Chip,
    use_stub: bool,
    verify: bool,
    before_operation: ResetBeforeOperation,
    after_operation: ResetAfterOperation,
    timeout: Duration,
    allow_auto_select: bool,
) -> Result<(Flasher, String, String), String> {
    let started = Instant::now();
    let mut attempts = 0_u32;
    let mut connection_errors = Vec::new();

    loop {
        match select_wired_flash_port_for_attempt(requested_port, allow_auto_select) {
            Ok(Some(port)) => {
                attempts = attempts.saturating_add(1);
                match connect_builtin_esp_flasher(
                    &port,
                    baud,
                    chip,
                    use_stub,
                    verify,
                    before_operation,
                    after_operation,
                ) {
                    Ok(flasher) => {
                        let mut log = String::new();
                        log.push_str(&format!(
                            "Serial port: {port}\nConnecting with built-in espflash: baud={baud}, stub={use_stub}, verify={verify}, before={before_operation:?}, after={after_operation:?}.\n"
                        ));
                        log.push_str(&format!("Connected after {attempts} attempt(s).\n"));
                        return Ok((flasher, port, log));
                    }
                    Err(err) => {
                        connection_errors.push(format!("{port}: {err}"));
                    }
                }
            }
            Ok(None) => {
                connection_errors.push("serial port not present yet".to_string());
            }
            Err(err) => return Err(err),
        }

        if started.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(WIRED_FLASH_CONNECT_RETRY_INTERVAL);
    }

    let requested = normalize_requested_wired_port(requested_port)
        .map(|port| format!(" Requested port: {port}."))
        .unwrap_or_default();
    let last_error = connection_errors
        .last()
        .map(|err| format!(" Last error: {err}."))
        .unwrap_or_default();
    Err(format!(
        "Timed out after {:.1}s waiting for a Listener ESP32-S3 serial flashing connection.{requested}{last_error}",
        timeout.as_secs_f32()
    ))
}

fn connect_builtin_esp_flasher(
    port: &str,
    baud: u32,
    chip: Chip,
    use_stub: bool,
    verify: bool,
    before_operation: ResetBeforeOperation,
    after_operation: ResetAfterOperation,
) -> Result<Flasher, String> {
    let usb_info = usb_port_info_for(port);
    let serial_port = serialport::new(port, 115_200)
        .flow_control(FlowControl::None)
        .timeout(WIRED_FLASH_SERIAL_IO_TIMEOUT)
        .open_native()
        .map_err(|err| format!("Failed to open serial port {port}: {err}"))?;

    Flasher::connect(
        serial_port,
        usb_info,
        Some(baud),
        use_stub,
        verify,
        false,
        Some(chip),
        after_operation,
        before_operation,
    )
    .map_err(|err| format!("Failed to connect to ESP ROM/flasher on {port}: {err}"))
}

fn select_wired_flash_port_for_attempt(
    requested_port: Option<&str>,
    allow_auto_select: bool,
) -> Result<Option<String>, String> {
    if let Some(port) = normalize_requested_wired_port(requested_port) {
        return Ok(Some(port));
    }
    if !allow_auto_select {
        return resolve_wired_flash_port(None).map(Some);
    }

    let ports = list_wired_firmware_ports_internal();
    if ports.is_empty() {
        return Ok(None);
    }
    let esp32_ports = ports
        .iter()
        .filter(|port| port.is_likely_esp32)
        .collect::<Vec<_>>();
    if esp32_ports.len() == 1 {
        return Ok(Some(esp32_ports[0].port.clone()));
    }
    if ports.len() == 1 {
        return Ok(Some(ports[0].port.clone()));
    }

    let summary = ports
        .iter()
        .map(|port| port.label.clone())
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "Multiple serial ports were detected. Choose the Listener COM port explicitly. Ports: {summary}"
    ))
}

fn usb_port_info_for(port_name: &str) -> UsbPortInfo {
    if let Ok(ports) = serialport::available_ports() {
        for port in ports {
            if port.port_name.eq_ignore_ascii_case(port_name) {
                if let SerialPortType::UsbPort(info) = port.port_type {
                    return info;
                }
            }
        }
    }
    UsbPortInfo {
        vid: 0,
        pid: 0,
        serial_number: None,
        manufacturer: None,
        product: None,
    }
}

fn format_wired_device_info(info: &espflash::flasher::DeviceInfo) -> String {
    let revision = info
        .revision
        .map(|(major, minor)| format!("{major}.{minor}"))
        .unwrap_or_else(|| "unknown".to_string());
    let features = if info.features.is_empty() {
        "none".to_string()
    } else {
        info.features.join(", ")
    };
    format!(
        "Connected chip: {}\nRevision: {revision}\nCrystal: {:?}\nMAC: {}\nFeatures: {features}\n",
        info.chip, info.crystal_frequency, info.mac_address
    )
}

fn trim_command_output(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let tail = chars[chars.len().saturating_sub(max_chars)..]
        .iter()
        .collect::<String>();
    format!("... output truncated ...\n{tail}")
}

#[tauri::command]
pub async fn transfer_firmware_ota_ble(
    app: AppHandle,
    coord: CoordinatorState<'_>,
    manifest: Value,
    firmware_bytes: Vec<u8>,
    expected_sha256: String,
) -> Result<FirmwareOtaBleTransferResult, String> {
    let phase = coord.dictation_phase_for_cli();
    if phase != SessionPhase::Idle {
        return Err(format!(
            "Recording or dictation is still active ({phase:?}). Stop it before updating firmware."
        ));
    }
    let manifest = serde_json::from_value::<crate::firmware_ota::FirmwareOtaManifest>(manifest)
        .map_err(|err| format!("OTA manifest payload is invalid: {err}"))
        .and_then(crate::firmware_ota::validate_normalized_manifest)?;
    if firmware_bytes.is_empty() {
        return Err("firmware_ota.bin is empty.".to_string());
    }
    if firmware_bytes.len() as u64 != manifest.file_size_bytes {
        return Err(format!(
            "firmware_ota.bin size changed before transfer: manifest={} actual={}",
            manifest.file_size_bytes,
            firmware_bytes.len()
        ));
    }
    if !expected_sha256.eq_ignore_ascii_case(&manifest.file_sha256) {
        return Err("OTA package hash changed before transfer.".to_string());
    }
    let actual_sha256 = crate::firmware_ota::sha256_hex(&firmware_bytes);
    if actual_sha256 != manifest.file_sha256 {
        return Err("firmware_ota.bin SHA256 does not match ota_manifest.json.".to_string());
    }

    coord.begin_firmware_ota_transfer();
    let is_listener_ota_v2 = manifest.is_listener_ble_ota_v2();
    let version = manifest.version;
    let manifest_chunk_bytes = manifest.gatt_chunk_bytes as usize;
    let transfer_version = version.clone();
    let transfer_sha256 = expected_sha256.clone();
    let app_for_progress = app;
    let transfer = tauri::async_runtime::spawn_blocking(move || {
        let progress = |bytes_sent, bytes_total| {
            let _ = app_for_progress.emit(
                "firmware-ota:progress",
                serde_json::json!({
                    "bytesSent": bytes_sent,
                    "bytesTotal": bytes_total,
                }),
            );
        };
        if is_listener_ota_v2 {
            crate::embedded_ble::transfer_listener_ota_v2(
                &firmware_bytes,
                manifest_chunk_bytes,
                Some(&progress),
            )
        } else {
            crate::embedded_ble::transfer_firmware_ota(
                &transfer_version,
                &transfer_sha256,
                &firmware_bytes,
                manifest_chunk_bytes,
                Some(&progress),
            )
        }
    })
    .await
    .map_err(|err| format!("Listener BLE OTA transfer task failed: {err}"))
    .and_then(|result| result);
    let confirmed_version = if transfer.is_ok() {
        confirm_firmware_ota_version(&version).await
    } else {
        None
    };
    coord.end_firmware_ota_transfer();
    coord.refresh_embedded_ble_listener();

    let stats = transfer?;
    Ok(FirmwareOtaBleTransferResult {
        bytes_transferred: stats.bytes_transferred,
        confirmed_version,
        transport: stats.transport,
    })
}

#[tauri::command]
pub async fn submit_embedded_audio_ble_stream(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord.submit_embedded_audio_ble_stream(timeout_ms).await
}

#[tauri::command]
pub fn cancel_dictation(coord: CoordinatorState<'_>) {
    coord.cancel_dictation();
}

#[tauri::command]
pub async fn handle_window_hotkey_event(
    coord: CoordinatorState<'_>,
    event_type: String,
    key: String,
    code: String,
    repeat: bool,
) -> Result<(), String> {
    coord
        .handle_window_hotkey_event(event_type, key, code, repeat)
        .await
}

#[cfg(debug_assertions)]
#[tauri::command]
pub async fn inject_hotkey_click_for_dev(coord: CoordinatorState<'_>) -> Result<(), String> {
    coord.inject_hotkey_click_for_dev().await
}

#[tauri::command]
pub async fn repolish(
    coord: CoordinatorState<'_>,
    raw_text: String,
    mode: PolishMode,
) -> Result<String, String> {
    log::info!(
        "[style-pack] command repolish requested legacy_mode={:?} raw_chars={}",
        mode,
        raw_text.chars().count()
    );
    coord.repolish(raw_text, mode).await
}

// ─────────────────────────── style packs ───────────────────────────

#[tauri::command]
pub fn list_style_packs(coord: CoordinatorState<'_>) -> Result<Vec<StylePack>, String> {
    let prefs = coord.prefs().get();
    coord
        .style_packs()
        .list_with_active(&prefs.active_style_pack_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_style_pack_from_template(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    template: StylePack,
) -> Result<StylePack, String> {
    log::info!(
        "[style-pack] command create_from_template name={} base_mode={:?}",
        template.name,
        template.base_mode
    );
    let created = coord
        .style_packs()
        .create_from_template(template)
        .map_err(|e| e.to_string())?;
    let prefs = coord.prefs().get();
    let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    Ok(created)
}

#[tauri::command]
pub fn save_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    style_pack: StylePack,
) -> Result<StylePack, String> {
    log::info!(
        "[style-pack] command save id={} kind={:?} base_mode={:?}",
        style_pack.id,
        style_pack.kind,
        style_pack.base_mode
    );
    let saved = coord
        .style_packs()
        .upsert(style_pack)
        .map_err(|e| e.to_string())?;
    if saved.kind == StylePackKind::Builtin {
        let prefs = coord.prefs().get();
        let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    }
    Ok(saved)
}

#[tauri::command]
pub fn preview_style_pack_runtime(
    coord: CoordinatorState<'_>,
    style_pack: StylePack,
) -> Result<StylePackRuntimeDiagnostics, String> {
    log::info!(
        "[style-pack] command preview_runtime id={} base_mode={:?} prompt_chars={}",
        style_pack.id,
        style_pack.base_mode,
        style_pack.prompt.chars().count()
    );
    Ok(coord.preview_style_pack_runtime(&style_pack))
}

#[tauri::command]
pub fn set_active_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
) -> Result<StylePack, String> {
    activate_style_pack_by_id(&coord, &app, &id)
}

#[tauri::command]
pub fn set_style_pack_enabled(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<Vec<StylePack>, String> {
    log::info!(
        "[style-pack] command set_enabled requested id={} enabled={}",
        id,
        enabled
    );
    coord
        .style_packs()
        .set_enabled(&id, enabled)
        .map_err(|e| e.to_string())?;
    let mut prefs = coord.prefs().get();
    if !enabled && prefs.active_style_pack_id == id {
        prefs.active_style_pack_id = default_active_style_pack_id();
    }
    let prefs = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    coord
        .style_packs()
        .list_with_active(&prefs.active_style_pack_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reset_builtin_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
) -> Result<StylePack, String> {
    log::info!("[style-pack] command reset_builtin requested id={id}");
    let saved = coord
        .style_packs()
        .reset_builtin(&id)
        .map_err(|e| e.to_string())?;
    let prefs = coord.prefs().get();
    let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    Ok(saved)
}

#[tauri::command]
pub fn delete_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    let mut prefs = coord.prefs().get();
    log::info!("[style-pack] command delete requested id={id}");
    coord
        .style_packs()
        .remove_imported(&id)
        .map_err(|e| e.to_string())?;
    if prefs.active_style_pack_id == id {
        prefs.active_style_pack_id = default_active_style_pack_id();
        let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    } else {
        refresh_tray_menu_async(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn import_style_pack_from_zip(
    coord: CoordinatorState<'_>,
    zip_path: String,
) -> Result<StylePack, String> {
    log::info!("[style-pack] command import requested zip_path={zip_path}");
    coord
        .style_packs()
        .import_from_zip(std::path::Path::new(&zip_path))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_style_pack_to_zip(
    coord: CoordinatorState<'_>,
    id: String,
    target_path: String,
) -> Result<String, String> {
    log::info!(
        "[style-pack] command export requested id={} target_path={}",
        id,
        target_path
    );
    coord
        .style_packs()
        .export_to_zip(&id, std::path::Path::new(&target_path))
        .map_err(|e| e.to_string())?;
    Ok(target_path)
}

// ─────────────────────────── style toggles (compat) ───────────────────────────

#[tauri::command]
pub fn set_default_polish_mode(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    mode: PolishMode,
) -> Result<(), String> {
    activate_builtin_style_mode(&coord, &app, mode)
}

#[tauri::command]
pub fn set_style_enabled(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    mode: PolishMode,
    enabled: bool,
) -> Result<(), String> {
    let pack_id = builtin_style_pack_id(mode).to_string();
    log::info!(
        "[style-pack] compat set_style_enabled mode={:?} pack_id={} enabled={}",
        mode,
        pack_id,
        enabled
    );
    coord
        .style_packs()
        .set_enabled(&pack_id, enabled)
        .map_err(|e| e.to_string())?;
    let mut prefs = coord.prefs().get();
    if !enabled && prefs.active_style_pack_id == pack_id {
        prefs.active_style_pack_id = default_active_style_pack_id();
    }
    let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    Ok(())
}

// ─────────────────────────── 系统权限 ───────────────────────────

#[tauri::command]
pub fn check_accessibility_permission() -> PermissionStatus {
    permissions::check_accessibility()
}

#[tauri::command]
pub fn request_accessibility_permission() -> PermissionStatus {
    permissions::request_accessibility()
}

#[tauri::command]
pub fn check_microphone_permission() -> PermissionStatus {
    permissions::check_microphone()
}

#[tauri::command]
pub fn request_microphone_permission(app: AppHandle) -> PermissionStatus {
    crate::request_microphone_from_foreground(&app)
}

/// 跳到 macOS 系统设置的指定隐私面板。pane: "accessibility" | "microphone".
#[tauri::command]
pub fn open_system_settings(pane: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let url = match pane.as_str() {
            "accessibility" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            "microphone" => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            _ => "x-apple.systempreferences:com.apple.preference.security?Privacy",
        };
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        fn wide_null(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(std::iter::once(0)).collect()
        }

        let uri = match pane.as_str() {
            "microphone" => "ms-settings:privacy-microphone",
            "bluetooth" => "ms-settings:bluetooth",
            "sound" => "ms-settings:sound",
            "accessibility" => "ms-settings:easeofaccess",
            _ => "ms-settings:",
        };

        let operation = wide_null("open");
        let target = wide_null(uri);
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
            Err(format!("ShellExecuteW failed: {}", result.0 as isize))
        } else {
            Ok(())
        }
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let _ = pane;
        Err("open_system_settings is only supported on macOS and Windows".to_string())
    }
}

/// 触发 macOS 系统弹"是否允许 Listener Type 访问麦克风"对话框。
/// 与 Swift `MicrophonePermission.request()` 同语义：只信系统权限回调，
/// 不用 cpal stream 成功与否伪造授权状态。
#[tauri::command]
pub fn trigger_microphone_prompt(app: AppHandle) -> Result<(), String> {
    let status = crate::request_microphone_from_foreground(&app);
    if matches!(
        status,
        PermissionStatus::Granted | PermissionStatus::NotApplicable
    ) {
        Ok(())
    } else {
        Err(format!("microphone permission is {status:?}"))
    }
}

// ─────────────────────────── QA (划词语音问答, issue #118) ───────────────────────────

/// 给前端 Settings 页渲染当前 QA 快捷键 label（如 `"Cmd+Shift+;"`）。
/// 未启用时返回空串。
#[tauri::command]
pub fn get_qa_hotkey_label(coord: CoordinatorState<'_>) -> String {
    coord.qa_hotkey_label()
}

/// 设置 QA 快捷键并热更新 monitor。
/// 传入 `None` 形式的字段不在这里支持——前端用 `binding == null` 时调下面的
/// "disable" 写法（写 prefs.qa_hotkey = None）即可。
#[tauri::command]
pub fn set_qa_hotkey(
    coord: CoordinatorState<'_>,
    binding: Option<ShortcutBinding>,
) -> Result<(), String> {
    if let Some(binding) = binding.as_ref() {
        crate::shortcut_binding::validate_binding(binding).map_err(|e| e.to_string())?;
        reject_device_fallback_reserved_hotkey(binding)?;
        if binding.modifiers.is_empty() && binding.primary.eq_ignore_ascii_case("shift") {
            return Err("Shift 单键目前只能用于翻译快捷键".into());
        }
    }
    let mut prefs = coord.prefs().get();
    if let Some(binding) = binding.as_ref() {
        reject_dictation_qa_hotkey_overlap(&prefs.dictation_hotkey, binding)?;
        reject_qa_translation_hotkey_overlap(binding, &prefs.translation_hotkey)?;
        reject_qa_switch_style_hotkey_overlap(binding, &prefs.switch_style_hotkey)?;
        reject_qa_open_app_hotkey_overlap(binding, &prefs.open_app_hotkey)?;
    }
    prefs.qa_hotkey = binding;
    coord.prefs().set(prefs).map_err(|e| e.to_string())?;
    coord.update_qa_hotkey_binding();
    Ok(())
}

/// 用户点 ✕ / 按 Esc 关 QA 浮窗。
#[tauri::command]
pub fn qa_window_dismiss(coord: CoordinatorState<'_>) {
    coord.qa_window_dismiss();
}

/// 用户点 📌 / 取消 📌。pinned=true 时浮窗不会自动隐藏。
#[tauri::command]
pub fn qa_window_pin(coord: CoordinatorState<'_>, pinned: bool) {
    coord.qa_window_pin(pinned);
}

// ─────────────────────────── 自定义组合键 ───────────────────────────

/// 测试一个组合键是否可以注册（验证格式，不实际注册）。
#[tauri::command]
pub fn validate_shortcut_binding(binding: ShortcutBinding) -> Result<(), String> {
    crate::shortcut_binding::validate_binding(&binding).map_err(|e| e.to_string())?;
    reject_device_fallback_reserved_hotkey(&binding)
}

#[tauri::command]
pub fn set_dictation_hotkey(
    coord: CoordinatorState<'_>,
    binding: ShortcutBinding,
) -> Result<(), String> {
    crate::shortcut_binding::validate_binding(&binding).map_err(|e| e.to_string())?;
    reject_device_fallback_reserved_hotkey(&binding)?;
    reject_bare_shift_dictation_shortcut(&binding)?;
    let mut prefs = coord.prefs().get();
    if let Some(qa_hotkey) = prefs.qa_hotkey.as_ref() {
        reject_dictation_qa_hotkey_overlap(&binding, qa_hotkey)?;
    }
    reject_dictation_translation_hotkey_overlap(&binding, &prefs.translation_hotkey)?;
    reject_dictation_switch_style_hotkey_overlap(&binding, &prefs.switch_style_hotkey)?;
    reject_dictation_open_app_hotkey_overlap(&binding, &prefs.open_app_hotkey)?;
    prefs.dictation_hotkey = binding;
    sync_dictation_hotkey_legacy_fields(&mut prefs);
    coord.prefs().set(prefs).map_err(|e| e.to_string())?;
    coord.update_hotkey_binding();
    coord.update_combo_hotkey_binding();
    Ok(())
}

#[tauri::command]
pub fn set_translation_hotkey(
    coord: CoordinatorState<'_>,
    binding: ShortcutBinding,
) -> Result<(), String> {
    crate::shortcut_binding::validate_binding(&binding).map_err(|e| e.to_string())?;
    reject_device_fallback_reserved_hotkey(&binding)?;
    let previous = coord.prefs().get();
    reject_dictation_translation_hotkey_overlap(&previous.dictation_hotkey, &binding)?;
    if let Some(qa_hotkey) = previous.qa_hotkey.as_ref() {
        reject_qa_translation_hotkey_overlap(qa_hotkey, &binding)?;
    }
    reject_translation_switch_style_hotkey_overlap(&binding, &previous.switch_style_hotkey)?;
    reject_translation_open_app_hotkey_overlap(&binding, &previous.open_app_hotkey)?;
    let mut prefs = previous.clone();
    prefs.translation_hotkey = binding;
    coord.prefs().set(prefs).map_err(|e| e.to_string())?;
    if let Err(e) = coord.try_update_translation_hotkey_binding() {
        if let Err(rollback_err) = coord.prefs().set(previous) {
            log::warn!("[commands] 回滚翻译快捷键失败: {rollback_err}");
        }
        coord.update_translation_hotkey_binding();
        return Err(e);
    }
    Ok(())
}

#[tauri::command]
pub fn set_switch_style_hotkey(
    coord: CoordinatorState<'_>,
    binding: ShortcutBinding,
) -> Result<(), String> {
    crate::shortcut_binding::validate_binding(&binding).map_err(|e| e.to_string())?;
    reject_device_fallback_reserved_hotkey(&binding)?;
    reject_modifier_only_action_shortcut(&binding)?;
    let mut prefs = coord.prefs().get();
    reject_dictation_switch_style_hotkey_overlap(&prefs.dictation_hotkey, &binding)?;
    reject_translation_switch_style_hotkey_overlap(&prefs.translation_hotkey, &binding)?;
    if let Some(qa_hotkey) = prefs.qa_hotkey.as_ref() {
        reject_qa_switch_style_hotkey_overlap(qa_hotkey, &binding)?;
    }
    reject_switch_style_open_app_hotkey_overlap(&binding, &prefs.open_app_hotkey)?;
    prefs.switch_style_hotkey = binding;
    coord.prefs().set(prefs).map_err(|e| e.to_string())?;
    coord.update_switch_style_hotkey_binding();
    Ok(())
}

#[tauri::command]
pub fn set_open_app_hotkey(
    coord: CoordinatorState<'_>,
    binding: ShortcutBinding,
) -> Result<(), String> {
    crate::shortcut_binding::validate_binding(&binding).map_err(|e| e.to_string())?;
    reject_device_fallback_reserved_hotkey(&binding)?;
    reject_modifier_only_action_shortcut(&binding)?;
    let mut prefs = coord.prefs().get();
    reject_dictation_open_app_hotkey_overlap(&prefs.dictation_hotkey, &binding)?;
    reject_translation_open_app_hotkey_overlap(&prefs.translation_hotkey, &binding)?;
    if let Some(qa_hotkey) = prefs.qa_hotkey.as_ref() {
        reject_qa_open_app_hotkey_overlap(qa_hotkey, &binding)?;
    }
    reject_switch_style_open_app_hotkey_overlap(&prefs.switch_style_hotkey, &binding)?;
    prefs.open_app_hotkey = binding;
    coord.prefs().set(prefs).map_err(|e| e.to_string())?;
    coord.update_open_app_hotkey_binding();
    Ok(())
}

fn reject_modifier_only_action_shortcut(binding: &ShortcutBinding) -> Result<(), String> {
    if binding.modifiers.is_empty()
        && (binding.primary.eq_ignore_ascii_case("shift")
            || crate::shortcut_binding::legacy_modifier_trigger(binding).is_some())
    {
        return Err("该快捷键需要使用组合键或非修饰主键".into());
    }
    Ok(())
}

#[tauri::command]
pub fn validate_combo_hotkey(binding: ComboBinding) -> Result<(), String> {
    let shortcut = ShortcutBinding {
        primary: binding.primary,
        modifiers: binding.modifiers,
    };
    reject_bare_shift_dictation_shortcut(&shortcut)?;
    crate::combo_hotkey::validate_binding(&shortcut).map_err(|e| e.to_string())?;
    reject_device_fallback_reserved_hotkey(&shortcut)
}

/// 设置自定义录音组合键并热更新 monitor。
#[tauri::command]
pub fn set_combo_hotkey(coord: CoordinatorState<'_>, binding: ComboBinding) -> Result<(), String> {
    let mut prefs = coord.prefs().get();
    let shortcut = ShortcutBinding {
        primary: binding.primary.clone(),
        modifiers: binding.modifiers.clone(),
    };
    reject_bare_shift_dictation_shortcut(&shortcut)?;
    crate::combo_hotkey::validate_binding(&shortcut).map_err(|e| e.to_string())?;
    reject_device_fallback_reserved_hotkey(&shortcut)?;
    if let Some(qa_hotkey) = prefs.qa_hotkey.as_ref() {
        reject_dictation_qa_hotkey_overlap(&shortcut, qa_hotkey)?;
    }
    reject_dictation_translation_hotkey_overlap(&shortcut, &prefs.translation_hotkey)?;
    reject_dictation_switch_style_hotkey_overlap(&shortcut, &prefs.switch_style_hotkey)?;
    reject_dictation_open_app_hotkey_overlap(&shortcut, &prefs.open_app_hotkey)?;
    prefs.custom_combo_hotkey = Some(binding);
    prefs.dictation_hotkey = shortcut;
    sync_dictation_hotkey_legacy_fields(&mut prefs);
    coord.prefs().set(prefs).map_err(|e| e.to_string())?;
    coord.update_hotkey_binding();
    coord.update_combo_hotkey_binding();
    Ok(())
}

fn reject_bare_shift_dictation_shortcut(binding: &ShortcutBinding) -> Result<(), String> {
    if binding.modifiers.is_empty() && binding.primary.eq_ignore_ascii_case("shift") {
        return Err("Shift 单键目前只能用于翻译快捷键".into());
    }
    Ok(())
}

fn is_device_fallback_reserved_hotkey(binding: &ShortcutBinding) -> bool {
    DeviceCustomKeyGesture::ALL.into_iter().any(|gesture| {
        DeviceCustomKeyId::ALL.into_iter().any(|key| {
            key.supports_gesture(gesture)
                && shortcut_bindings_overlap(binding, &device_fallback_shortcut(key, gesture))
        })
    })
}

fn reject_device_fallback_reserved_hotkey(binding: &ShortcutBinding) -> Result<(), String> {
    if is_device_fallback_reserved_hotkey(binding) {
        return Err("设备 fallback 快捷键已保留给 KEY1-KEY4 和 EC11 单击入口".into());
    }
    Ok(())
}

fn device_fallback_shortcut(
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) -> ShortcutBinding {
    ShortcutBinding {
        primary: key.fallback_primary_for(gesture).into(),
        modifiers: if key == DeviceCustomKeyId::Knob {
            vec!["shift".into()]
        } else {
            Vec::new()
        },
    }
}

fn sync_dictation_hotkey_legacy_fields(prefs: &mut UserPreferences) {
    prefs.hotkey.mode = crate::types::HotkeyMode::Toggle;
    if let Some(trigger) = crate::shortcut_binding::legacy_modifier_trigger(&prefs.dictation_hotkey)
    {
        prefs.hotkey.trigger = trigger;
        prefs.custom_combo_hotkey = None;
        return;
    }
    prefs.hotkey.trigger = crate::types::HotkeyTrigger::Custom;
    prefs.custom_combo_hotkey = if prefs.dictation_hotkey.primary.trim().is_empty() {
        None
    } else {
        Some(ComboBinding {
            primary: prefs.dictation_hotkey.primary.clone(),
            modifiers: prefs.dictation_hotkey.modifiers.clone(),
        })
    };
}

fn reject_dictation_qa_hotkey_overlap(
    dictation: &ShortcutBinding,
    qa: &ShortcutBinding,
) -> Result<(), String> {
    if shortcut_bindings_overlap(dictation, qa) {
        return Err("QA 快捷键不能和听写快捷键相同".into());
    }
    Ok(())
}

fn reject_hotkey_overlap(
    left: &ShortcutBinding,
    right: &ShortcutBinding,
    message: &'static str,
) -> Result<(), String> {
    if shortcut_bindings_overlap(left, right) {
        return Err(message.into());
    }
    Ok(())
}

fn reject_hotkey_collisions(prefs: &UserPreferences) -> Result<(), String> {
    reject_device_fallback_reserved_hotkey(&prefs.dictation_hotkey)?;
    reject_device_fallback_reserved_hotkey(&prefs.translation_hotkey)?;
    reject_device_fallback_reserved_hotkey(&prefs.switch_style_hotkey)?;
    reject_device_fallback_reserved_hotkey(&prefs.open_app_hotkey)?;
    if let Some(qa_hotkey) = prefs.qa_hotkey.as_ref() {
        reject_device_fallback_reserved_hotkey(qa_hotkey)?;
        reject_dictation_qa_hotkey_overlap(&prefs.dictation_hotkey, qa_hotkey)?;
        reject_qa_translation_hotkey_overlap(qa_hotkey, &prefs.translation_hotkey)?;
        reject_qa_switch_style_hotkey_overlap(qa_hotkey, &prefs.switch_style_hotkey)?;
        reject_qa_open_app_hotkey_overlap(qa_hotkey, &prefs.open_app_hotkey)?;
    }
    reject_dictation_translation_hotkey_overlap(
        &prefs.dictation_hotkey,
        &prefs.translation_hotkey,
    )?;
    reject_dictation_switch_style_hotkey_overlap(
        &prefs.dictation_hotkey,
        &prefs.switch_style_hotkey,
    )?;
    reject_dictation_open_app_hotkey_overlap(&prefs.dictation_hotkey, &prefs.open_app_hotkey)?;
    reject_translation_switch_style_hotkey_overlap(
        &prefs.translation_hotkey,
        &prefs.switch_style_hotkey,
    )?;
    reject_translation_open_app_hotkey_overlap(&prefs.translation_hotkey, &prefs.open_app_hotkey)?;
    reject_switch_style_open_app_hotkey_overlap(
        &prefs.switch_style_hotkey,
        &prefs.open_app_hotkey,
    )?;
    Ok(())
}

fn validate_device_custom_keys(keys: &DeviceCustomKeys) -> Result<(), String> {
    for mapping in [&keys.key1, &keys.key2, &keys.key3, &keys.key4, &keys.knob] {
        validate_device_custom_key_mapping(mapping)?;
    }
    Ok(())
}

fn validate_device_custom_key_mapping(mapping: &DeviceCustomKeyMapping) -> Result<(), String> {
    if mapping.action != DeviceCustomKeyAction::SendShortcut {
        return Ok(());
    }
    let shortcut = mapping
        .shortcut
        .as_ref()
        .ok_or_else(|| "设备自定义键的快捷键动作缺少按键绑定".to_string())?;
    crate::shortcut_binding::validate_binding(shortcut).map_err(|e| e.to_string())?;
    reject_modifier_only_action_shortcut(shortcut)?;
    if is_device_fallback_reserved_hotkey(shortcut) {
        return Err("设备自定义键不能转发为设备 fallback 快捷键，避免重复触发自身".into());
    }
    Ok(())
}

fn reject_dictation_translation_hotkey_overlap(
    dictation: &ShortcutBinding,
    translation: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(dictation, translation, "翻译快捷键不能和听写快捷键相同")
}

fn reject_dictation_switch_style_hotkey_overlap(
    dictation: &ShortcutBinding,
    switch_style: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(
        dictation,
        switch_style,
        "切换风格快捷键不能和听写快捷键相同",
    )
}

fn reject_dictation_open_app_hotkey_overlap(
    dictation: &ShortcutBinding,
    open_app: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(dictation, open_app, "打开应用快捷键不能和听写快捷键相同")
}

fn reject_qa_translation_hotkey_overlap(
    qa: &ShortcutBinding,
    translation: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(qa, translation, "翻译快捷键不能和 QA 快捷键相同")
}

fn reject_qa_switch_style_hotkey_overlap(
    qa: &ShortcutBinding,
    switch_style: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(qa, switch_style, "切换风格快捷键不能和 QA 快捷键相同")
}

fn reject_qa_open_app_hotkey_overlap(
    qa: &ShortcutBinding,
    open_app: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(qa, open_app, "打开应用快捷键不能和 QA 快捷键相同")
}

fn reject_translation_switch_style_hotkey_overlap(
    translation: &ShortcutBinding,
    switch_style: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(
        translation,
        switch_style,
        "切换风格快捷键不能和翻译快捷键相同",
    )
}

fn reject_translation_open_app_hotkey_overlap(
    translation: &ShortcutBinding,
    open_app: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(translation, open_app, "打开应用快捷键不能和翻译快捷键相同")
}

fn reject_switch_style_open_app_hotkey_overlap(
    switch_style: &ShortcutBinding,
    open_app: &ShortcutBinding,
) -> Result<(), String> {
    reject_hotkey_overlap(
        switch_style,
        open_app,
        "打开应用快捷键不能和切换风格快捷键相同",
    )
}

fn shortcut_bindings_overlap(left: &ShortcutBinding, right: &ShortcutBinding) -> bool {
    let left_legacy = crate::shortcut_binding::legacy_modifier_trigger(left);
    let right_legacy = crate::shortcut_binding::legacy_modifier_trigger(right);
    match (left_legacy, right_legacy) {
        (Some(left), Some(right)) => left == right,
        (Some(_), None) | (None, Some(_)) => false,
        (None, None) => {
            let Ok(left) = crate::shortcut_binding::parse_global_hotkey(left) else {
                return false;
            };
            let Ok(right) = crate::shortcut_binding::parse_global_hotkey(right) else {
                return false;
            };
            left == right
        }
    }
}

// ─────────────────────────── local ASR (Qwen3-ASR) ───────────────────────────

use crate::asr::local::{
    download::{fetch_remote_info, RemoteInfo},
    DownloadManager, Mirror, ModelId, ModelStatus, PROVIDER_ID as LOCAL_PROVIDER_ID,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAsrSettings {
    pub provider_id: String,
    pub active_model: String,
    pub mirror: String,
    /// macOS 才编入引擎；Windows 端 UI 需要据此把"开始下载"按钮灰掉。
    pub engine_available: bool,
}

#[tauri::command]
pub fn local_asr_get_settings(coord: CoordinatorState<'_>) -> LocalAsrSettings {
    let prefs = coord.prefs().get();
    LocalAsrSettings {
        provider_id: LOCAL_PROVIDER_ID.into(),
        active_model: prefs.local_asr_active_model,
        mirror: prefs.local_asr_mirror,
        engine_available: cfg!(target_os = "macos"),
    }
}

#[tauri::command]
pub fn local_asr_set_active_model(
    coord: CoordinatorState<'_>,
    model_id: String,
) -> Result<(), String> {
    if ModelId::from_str(&model_id).is_none() {
        return Err(format!("unknown model id: {model_id}"));
    }
    let mut prefs = coord.prefs().get();
    prefs.local_asr_active_model = model_id;
    coord.prefs().set(prefs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn local_asr_set_mirror(coord: CoordinatorState<'_>, mirror: String) -> Result<(), String> {
    let _normalized = Mirror::from_str(&mirror);
    let mut prefs = coord.prefs().get();
    prefs.local_asr_mirror = mirror;
    coord.prefs().set(prefs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn local_asr_list_models() -> Vec<ModelStatus> {
    crate::asr::local::models::list_status()
}

/// 实时去 HuggingFace API 拉某个模型的真实文件清单 + 总尺寸；
/// 前端在显示模型卡时调一次，避免硬编码尺寸过期。
#[tauri::command]
pub async fn local_asr_fetch_remote_info(
    model_id: String,
    mirror: Option<String>,
) -> Result<RemoteInfo, String> {
    let id = ModelId::from_str(&model_id).ok_or_else(|| format!("unknown model id: {model_id}"))?;
    let m = mirror.as_deref().map(Mirror::from_str).unwrap_or_default();
    fetch_remote_info(id, m).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn local_asr_download_model(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
    model_id: String,
    mirror: Option<String>,
) -> Result<(), String> {
    let id = ModelId::from_str(&model_id).ok_or_else(|| format!("unknown model id: {model_id}"))?;
    let m = mirror.as_deref().map(Mirror::from_str).unwrap_or_default();
    manager.start(app, id, m);
    Ok(())
}

#[tauri::command]
pub fn local_asr_cancel_download(
    manager: State<'_, Arc<DownloadManager>>,
    model_id: String,
) -> Result<(), String> {
    let id = ModelId::from_str(&model_id).ok_or_else(|| format!("unknown model id: {model_id}"))?;
    manager.cancel(id);
    Ok(())
}

#[tauri::command]
pub fn local_asr_delete_model(coord: CoordinatorState<'_>, model_id: String) -> Result<(), String> {
    let id = ModelId::from_str(&model_id).ok_or_else(|| format!("unknown model id: {model_id}"))?;
    // 如果内存里加载的就是要删的这个模型，先释放：否则 mmap 残留指向已 unlink 的文件，
    // 且 RAM 直到下次切模型 / 用户手动按"释放"才回收。
    if coord.local_asr_loaded_model().as_deref() == Some(id.as_str()) {
        coord.release_local_asr_engine();
    }
    crate::asr::local::models::delete_model(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn local_asr_test_model(
    model_id: String,
) -> Result<crate::asr::local::test_run::TestResult, String> {
    let id = ModelId::from_str(&model_id).ok_or_else(|| format!("unknown model id: {model_id}"))?;
    crate::asr::local::test_run::run_test(id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAsrEngineStatus {
    pub loaded: bool,
    pub model_id: Option<String>,
    pub keep_loaded_secs: u32,
}

#[tauri::command]
pub fn local_asr_engine_status(coord: CoordinatorState<'_>) -> LocalAsrEngineStatus {
    let prefs = coord.prefs().get();
    LocalAsrEngineStatus {
        loaded: coord.local_asr_loaded_model().is_some(),
        model_id: coord.local_asr_loaded_model(),
        keep_loaded_secs: prefs.local_asr_keep_loaded_secs,
    }
}

#[tauri::command]
pub fn local_asr_release_engine(coord: CoordinatorState<'_>) {
    coord.release_local_asr_engine();
}

#[tauri::command]
pub fn local_asr_preload(coord: tauri::State<'_, std::sync::Arc<crate::coordinator::Coordinator>>) {
    coord.preload_local_asr_in_background();
}

#[tauri::command]
pub fn local_asr_set_keep_loaded_secs(
    coord: CoordinatorState<'_>,
    seconds: u32,
) -> Result<(), String> {
    let mut prefs = coord.prefs().get();
    prefs.local_asr_keep_loaded_secs = seconds;
    coord.prefs().set(prefs).map_err(|e| e.to_string())
}

// ───────────────────── Windows local ASR (Foundry Local Whisper) ─────────────────────

fn active_foundry_model_from_prefs(prefs: &UserPreferences) -> String {
    if model_alias_is_known(&prefs.foundry_local_asr_model) {
        prefs.foundry_local_asr_model.clone()
    } else {
        DEFAULT_MODEL_ALIAS.to_string()
    }
}

fn validate_foundry_model_alias(model_alias: &str) -> Result<(), String> {
    if model_alias_is_known(model_alias) {
        Ok(())
    } else {
        Err(format!(
            "unknown Foundry Whisper model alias: {model_alias}"
        ))
    }
}

fn normalize_foundry_language_hint(language_hint: &str) -> Result<String, String> {
    let normalized = language_hint.trim().to_string();
    if normalized.is_empty()
        || (normalized.len() == 2 && normalized.bytes().all(|b| b.is_ascii_lowercase()))
    {
        Ok(normalized)
    } else {
        Err("language hint must be empty or ISO 639-1 lowercase code".to_string())
    }
}

fn normalize_foundry_runtime_source(source: &str) -> String {
    crate::asr::local::foundry_native::normalize_runtime_source_str(source)
}

#[tauri::command]
pub async fn foundry_local_asr_status(
    coord: CoordinatorState<'_>,
    runtime: State<'_, Arc<FoundryLocalRuntime>>,
) -> Result<FoundryRuntimeStatus, String> {
    let prefs = coord.prefs().get();
    let active_model = active_foundry_model_from_prefs(&prefs);
    Ok(runtime
        .status_snapshot(&active_model, &prefs.foundry_local_runtime_source)
        .await)
}

#[tauri::command]
pub async fn foundry_local_asr_catalog(
    runtime: State<'_, Arc<FoundryLocalRuntime>>,
) -> Result<Vec<FoundryCatalogModel>, String> {
    runtime
        .catalog_snapshot()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn foundry_local_asr_set_model(
    coord: CoordinatorState<'_>,
    model_alias: String,
) -> Result<(), String> {
    validate_foundry_model_alias(&model_alias)?;
    let mut prefs = coord.prefs().get();
    prefs.foundry_local_asr_model = model_alias;
    coord.prefs().set(prefs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn foundry_local_asr_set_language_hint(
    coord: CoordinatorState<'_>,
    language_hint: String,
) -> Result<(), String> {
    let normalized = normalize_foundry_language_hint(&language_hint)?;
    let mut prefs = coord.prefs().get();
    prefs.foundry_local_asr_language_hint = normalized;
    coord.prefs().set(prefs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn foundry_local_asr_set_runtime_source(
    coord: CoordinatorState<'_>,
    source: String,
) -> Result<(), String> {
    let mut prefs = coord.prefs().get();
    prefs.foundry_local_runtime_source = normalize_foundry_runtime_source(&source);
    coord.prefs().set(prefs).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn foundry_local_asr_prepare(
    app: AppHandle,
    coord: CoordinatorState<'_>,
    runtime: State<'_, Arc<FoundryLocalRuntime>>,
    model_alias: String,
) -> Result<String, String> {
    validate_foundry_model_alias(&model_alias)?;
    let prefs = coord.prefs().get();
    let runtime_source = prefs.foundry_local_runtime_source.clone();
    let progress_app = app.clone();
    let result = runtime
        .ensure_loaded_with_progress(&model_alias, &runtime_source, move |payload| {
            emit_foundry_prepare_progress(&progress_app, payload);
        })
        .await;
    match result {
        Ok(model_id) => Ok(model_id),
        Err(error) => {
            let message = format!("{error:#}");
            emit_foundry_prepare_progress(
                &app,
                FoundryPrepareProgressPayload::failed(
                    model_alias,
                    "Foundry Local Whisper prepare failed",
                    message.clone(),
                ),
            );
            Err(message)
        }
    }
}

#[tauri::command]
pub fn foundry_local_asr_cancel_prepare(
    runtime: State<'_, Arc<FoundryLocalRuntime>>,
) -> Result<(), String> {
    runtime.request_cancel_prepare();
    Ok(())
}

#[tauri::command]
pub async fn foundry_local_asr_release(
    runtime: State<'_, Arc<FoundryLocalRuntime>>,
) -> Result<(), String> {
    runtime.release_now().await.map_err(|e| format!("{e:#}"))
}

fn emit_foundry_prepare_progress(app: &AppHandle, payload: FoundryPrepareProgressPayload) {
    if let Err(error) = app.emit("foundry-local-asr-prepare-progress", payload) {
        log::warn!("[foundry-asr] emit prepare progress failed: {error}");
    }
}

/// 把当前会话的 listener-type.log 复制到用户选择的位置（前端用 plugin-dialog 拿 target_path）。
/// 路径来自 lib::log_dir_path() —— mac: ~/Library/Logs/Listener Type/listener-type.log，
/// windows: %LOCALAPPDATA%\Listener Type\Logs\listener-type.log。
#[tauri::command]
pub fn export_error_log(target_path: String) -> Result<(), String> {
    let src = crate::log_dir_path().join("listener-type.log");
    if !src.exists() {
        return Err(format!("日志文件不存在：{}", src.display()));
    }
    std::fs::copy(&src, std::path::Path::new(&target_path))
        .map(|_| ())
        .map_err(|e| format!("复制日志失败：{}", e))
}

/// Export the minimal first-start/OOBE diagnostic package used by the Windows installer path.
///
/// The package is intentionally metadata-only: it excludes audio bytes, raw transcripts,
/// final inserted text and credential values. Credentials are represented only as
/// configured/unconfigured booleans.
#[tauri::command]
pub fn export_diagnostic_package(
    coord: CoordinatorState<'_>,
    target_path: String,
) -> Result<String, String> {
    let package = build_diagnostic_package(coord.inner())?;
    let firmware_log = crate::embedded_ble::pull_firmware_diagnostic_log(Duration::from_secs(45));
    let target = diagnostic_export_target_path(&target_path, &package)?;
    write_diagnostic_package_zip(&target, &package, &firmware_log)?;
    Ok(target.display().to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticPackage {
    schema_version: u32,
    generated_at: String,
    app: DiagnosticApp,
    platform: DiagnosticPlatform,
    firmware: DiagnosticFirmware,
    ble: DiagnosticBle,
    config: DiagnosticConfig,
    credentials: DiagnosticCredentials,
    recent_errors: Vec<String>,
    timeline: Vec<String>,
    recent_sessions: Vec<DiagnosticRecentSession>,
    privacy: DiagnosticPrivacy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticApp {
    product_name: &'static str,
    identifier: &'static str,
    version: &'static str,
    log_path: String,
    executable_path: Option<String>,
    windows_ime_status: WindowsImeStatus,
    hotkey_status: HotkeyStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticPlatform {
    os: &'static str,
    family: &'static str,
    arch: &'static str,
    debug_build: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticFirmware {
    device_model: &'static str,
    version: Option<String>,
    protocol: &'static str,
    protocol_version: Option<u32>,
    readiness: Option<Value>,
    wake_policy: FirmwareWakePolicySnapshot,
    source: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticBle {
    input_source: Value,
    enabled_by_input_source: bool,
    background_listener_disabled_by_env: bool,
    background_listener_active: bool,
    background_listener_ready: bool,
    background_listener_generation: u64,
    service_uuid: &'static str,
    diagnostic_snapshot: crate::embedded_ble::BleDiagnosticSnapshot,
    failure_taxonomy: Vec<crate::embedded_ble::BleFailureClassification>,
    device_address: Option<String>,
    firmware_version: Option<String>,
    battery_percent: Option<u8>,
    capabilities: Vec<String>,
    recent_disconnect_reason: Option<String>,
    reconnect_attempts: u32,
    notify_subscription_state: String,
    wake_recovery: EmbeddedBleWakeRecoverySnapshot,
    session_actor_history: Vec<EmbeddedBleSessionActorDiagnosticRecord>,
    recent_embedded_session_count: usize,
    latest_session_id: Option<String>,
    last_error_code: Option<String>,
    last_embedded_audio_end_reason: Option<Value>,
    last_embedded_audio_stats: Option<crate::embedded_audio::SessionStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticConfig {
    dictation_input_source: Value,
    active_asr_provider: String,
    active_llm_provider: String,
    asr_configured: bool,
    llm_configured: bool,
    default_mode: Value,
    active_style_pack_id: String,
    enabled_modes: Value,
    show_capsule: bool,
    start_minimized: bool,
    launch_at_login: bool,
    auto_update_check: bool,
    update_channel: Value,
    history_retention_days: u32,
    history_max_entries: Option<u32>,
    record_audio_for_debug: bool,
    audio_recording_max_entries: Option<u32>,
    local_asr_active_model: String,
    local_asr_keep_loaded_secs: u32,
    foundry_local_asr_model: String,
    foundry_local_runtime_source: String,
    foundry_local_asr_language_hint_configured: bool,
    foundry_local_asr_keep_loaded_secs: u32,
    microphone_device_configured: bool,
    marketplace_backend_configured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticCredentials {
    active_asr_provider: String,
    active_llm_provider: String,
    asr_configured: bool,
    llm_configured: bool,
    volcengine_configured: bool,
    asr_api_key_configured: bool,
    asr_endpoint_configured: bool,
    asr_model_configured: bool,
    llm_api_key_configured: bool,
    llm_endpoint_configured: bool,
    llm_model_configured: bool,
    codex_oauth_configured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRecentSession {
    id: String,
    created_at: String,
    mode: Value,
    insert_status: Value,
    error_code: Option<String>,
    duration_ms: Option<u64>,
    dictionary_entry_count: Option<u32>,
    has_audio_recording: Option<bool>,
    embedded_audio_stats: Option<crate::embedded_audio::SessionStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticPrivacy {
    excludes_audio_contents: bool,
    excludes_audio_recordings: bool,
    excludes_raw_transcripts: bool,
    excludes_final_text: bool,
    excludes_api_key_values: bool,
    credential_values_exported: bool,
    notes: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExportManifest {
    schema_version: u32,
    generated_at: String,
    package_file_name: String,
    app_version: &'static str,
    device_descriptor: String,
    desktop_package_schema_version: u32,
    firmware_diagnostic_log: crate::embedded_ble::FirmwareDiagnosticLogPull,
    files: Vec<DiagnosticZipEntry>,
    privacy: DiagnosticExportPrivacy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticZipEntry {
    path: String,
    kind: &'static str,
    bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExportPrivacy {
    audio_contents_included: bool,
    audio_recordings_included: bool,
    raw_transcripts_included: bool,
    final_text_included: bool,
    credential_values_included: bool,
    notes: Vec<&'static str>,
}

struct DiagnosticAudioSampleFile {
    session_id: String,
    zip_path: String,
    bytes: Vec<u8>,
}

const DIAGNOSTIC_AUDIO_SAMPLE_LIMIT: usize = 3;
const DIAGNOSTIC_AUDIO_SAMPLE_MAX_BYTES: u64 = 20 * 1024 * 1024;

fn diagnostic_export_target_path(
    requested_path: &str,
    package: &DiagnosticPackage,
) -> Result<PathBuf, String> {
    let requested = Path::new(requested_path);
    let parent = requested
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let requested_name = requested
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let file_name = if diagnostic_requested_file_name_is_generic(requested_name) {
        diagnostic_package_file_name(package)
    } else if requested
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        requested_name.to_string()
    } else {
        format!("{requested_name}.zip")
    };
    Ok(parent.join(file_name))
}

fn diagnostic_requested_file_name_is_generic(name: &str) -> bool {
    if name.trim().is_empty() {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".json")
        || lower.starts_with("listener-type-diagnostic")
        || lower.starts_with("listener-type-ble-wake-diagnostics")
        || lower.starts_with("listener-type-ota-diagnostics")
}

fn diagnostic_package_file_name(package: &DiagnosticPackage) -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    format!(
        "listener-type-diagnostic-{}-{timestamp}.zip",
        diagnostic_device_descriptor(package)
    )
}

fn diagnostic_device_descriptor(package: &DiagnosticPackage) -> String {
    let firmware = package
        .ble
        .firmware_version
        .as_deref()
        .or(package.firmware.version.as_deref())
        .unwrap_or("fw-unknown");
    let address = package
        .ble
        .device_address
        .as_deref()
        .unwrap_or("device-offline");
    sanitize_diagnostic_file_segment(&format!("{firmware}-{address}"))
}

fn sanitize_diagnostic_file_segment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut previous_dash = false;
    for ch in value.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '-'
        };
        if normalized == '-' {
            if previous_dash {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        output.push(normalized);
    }
    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "device-unknown".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn write_diagnostic_package_zip(
    target: &Path,
    package: &DiagnosticPackage,
    firmware_log: &crate::embedded_ble::FirmwareDiagnosticLogPull,
) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败：{e}"))?;
        }
    }

    let log_lines = read_diagnostic_log_tail(1000);
    let desktop_package_bytes =
        serde_json::to_vec_pretty(package).map_err(|e| format!("生成桌面诊断 JSON 失败：{e}"))?;
    let log_tail = sanitized_diagnostic_log_tail(&log_lines);
    let log_tail_bytes = log_tail.as_bytes().to_vec();
    let ble_history_bytes = serde_json::to_vec_pretty(&diagnostic_ble_history(package, &log_lines))
        .map_err(|e| format!("生成 BLE 连接历史失败：{e}"))?;
    let audio_samples = diagnostic_audio_sample_files(package);
    let audio_manifest_bytes =
        serde_json::to_vec_pretty(&diagnostic_audio_samples_manifest(package, &audio_samples))
            .map_err(|e| format!("生成音频样本清单失败：{e}"))?;
    let firmware_summary_bytes = serde_json::to_vec_pretty(firmware_log)
        .map_err(|e| format!("生成固件诊断摘要失败：{e}"))?;

    let mut files = vec![
        DiagnosticZipEntry {
            path: "desktop/diagnostic_package.json".to_string(),
            kind: "desktop_diagnostic_json",
            bytes: desktop_package_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "desktop/listener-type-log-tail.txt".to_string(),
            kind: "desktop_log_tail",
            bytes: log_tail_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "desktop/ble_connection_history.json".to_string(),
            kind: "ble_connection_history",
            bytes: ble_history_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "desktop/audio_samples_manifest.json".to_string(),
            kind: "audio_sample_metadata",
            bytes: audio_manifest_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "firmware/diag_log_summary.json".to_string(),
            kind: "firmware_diagnostic_summary",
            bytes: firmware_summary_bytes.len(),
        },
    ];
    if firmware_log.status == "ok" {
        files.push(DiagnosticZipEntry {
            path: "firmware/diag_log.bin".to_string(),
            kind: "firmware_diagnostic_events",
            bytes: firmware_log.raw_event_bytes.len(),
        });
    }
    for sample in &audio_samples {
        files.push(DiagnosticZipEntry {
            path: sample.zip_path.clone(),
            kind: "audio_sample_wav",
            bytes: sample.bytes.len(),
        });
    }

    let package_file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("listener-type-diagnostic.zip")
        .to_string();
    let manifest = DiagnosticExportManifest {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        package_file_name,
        app_version: env!("CARGO_PKG_VERSION"),
        device_descriptor: diagnostic_device_descriptor(package),
        desktop_package_schema_version: package.schema_version,
        firmware_diagnostic_log: firmware_log.clone(),
        files,
        privacy: DiagnosticExportPrivacy {
            audio_contents_included: !audio_samples.is_empty(),
            audio_recordings_included: !audio_samples.is_empty(),
            raw_transcripts_included: false,
            final_text_included: false,
            credential_values_included: false,
            notes: vec![
                "Desktop logs are exported as a sanitized tail only.",
                "Only previously retained debug WAV recordings are included as audio samples; no new audio is captured during export.",
                "Firmware diag_log events are raw firmware diagnostic records without desktop transcript or credential values.",
            ],
        },
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| format!("生成诊断包 manifest 失败：{e}"))?;

    let file = File::create(target).map_err(|e| format!("创建诊断 zip 失败：{e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    write_zip_entry(&mut zip, "manifest.json", &manifest_bytes, options)?;
    write_zip_entry(
        &mut zip,
        "desktop/diagnostic_package.json",
        &desktop_package_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "desktop/listener-type-log-tail.txt",
        &log_tail_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "desktop/ble_connection_history.json",
        &ble_history_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "desktop/audio_samples_manifest.json",
        &audio_manifest_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "firmware/diag_log_summary.json",
        &firmware_summary_bytes,
        options,
    )?;
    if firmware_log.status == "ok" {
        write_zip_entry(
            &mut zip,
            "firmware/diag_log.bin",
            &firmware_log.raw_event_bytes,
            options,
        )?;
    }
    for sample in &audio_samples {
        write_zip_entry(&mut zip, &sample.zip_path, &sample.bytes, options)?;
    }
    zip.finish()
        .map(|_| ())
        .map_err(|e| format!("完成诊断 zip 失败：{e}"))
}

fn write_zip_entry<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    path: &str,
    bytes: &[u8],
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    zip.start_file(path, options)
        .map_err(|e| format!("创建诊断 zip 条目 {path} 失败：{e}"))?;
    zip.write_all(bytes)
        .map_err(|e| format!("写入诊断 zip 条目 {path} 失败：{e}"))
}

fn sanitized_diagnostic_log_tail(lines: &[String]) -> String {
    lines
        .iter()
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn diagnostic_ble_history(package: &DiagnosticPackage, lines: &[String]) -> Value {
    let ble_lines: Vec<String> = lines
        .iter()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("ble") || lower.contains("bluetooth") || lower.contains("gatt")
        })
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .rev()
        .take(160)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    serde_json::json!({
        "capturedAt": &package.generated_at,
        "deviceAddress": &package.ble.device_address,
        "firmwareVersion": &package.ble.firmware_version,
        "batteryPercent": package.ble.battery_percent,
        "capabilities": &package.ble.capabilities,
        "backgroundListenerActive": package.ble.background_listener_active,
        "backgroundListenerReady": package.ble.background_listener_ready,
        "notifySubscriptionState": &package.ble.notify_subscription_state,
        "recentDisconnectReason": &package.ble.recent_disconnect_reason,
        "reconnectAttempts": package.ble.reconnect_attempts,
        "failureTaxonomy": &package.ble.failure_taxonomy,
        "diagnosticSnapshot": &package.ble.diagnostic_snapshot,
        "wakeRecovery": &package.ble.wake_recovery,
        "sessionActorHistory": &package.ble.session_actor_history,
        "logLines": ble_lines,
    })
}

fn diagnostic_audio_sample_files(package: &DiagnosticPackage) -> Vec<DiagnosticAudioSampleFile> {
    let mut samples = Vec::new();
    for session in package.recent_sessions.iter() {
        if samples.len() >= DIAGNOSTIC_AUDIO_SAMPLE_LIMIT {
            break;
        }
        if session.has_audio_recording != Some(true) || !is_valid_session_id(&session.id) {
            continue;
        }
        let Ok(path) = crate::persistence::recording_path_for_session(&session.id) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() == 0 || metadata.len() > DIAGNOSTIC_AUDIO_SAMPLE_MAX_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        samples.push(DiagnosticAudioSampleFile {
            session_id: session.id.clone(),
            zip_path: format!("desktop/audio_samples/{}.wav", session.id),
            bytes,
        });
    }
    samples
}

fn diagnostic_audio_samples_manifest(
    package: &DiagnosticPackage,
    audio_samples: &[DiagnosticAudioSampleFile],
) -> Value {
    let included_recordings: Vec<Value> = audio_samples
        .iter()
        .map(|sample| {
            serde_json::json!({
                "sessionId": &sample.session_id,
                "path": &sample.zip_path,
                "bytes": sample.bytes.len(),
            })
        })
        .collect();
    serde_json::json!({
        "capturedAt": &package.generated_at,
        "audioContentsIncluded": !audio_samples.is_empty(),
        "audioRecordingsIncluded": !audio_samples.is_empty(),
        "audioSampleLimit": DIAGNOSTIC_AUDIO_SAMPLE_LIMIT,
        "audioSampleMaxBytes": DIAGNOSTIC_AUDIO_SAMPLE_MAX_BYTES,
        "recordAudioForDebug": package.config.record_audio_for_debug,
        "audioRecordingMaxEntries": package.config.audio_recording_max_entries,
        "includedRecordings": included_recordings,
        "recentSessions": &package.recent_sessions,
        "latestEmbeddedSessionId": &package.ble.latest_session_id,
        "lastEmbeddedAudioEndReason": &package.ble.last_embedded_audio_end_reason,
        "lastEmbeddedAudioStats": &package.ble.last_embedded_audio_stats,
    })
}

fn build_diagnostic_package(coord: &Arc<Coordinator>) -> Result<DiagnosticPackage, String> {
    build_diagnostic_package_with_ble_snapshot(
        coord,
        crate::embedded_ble::ble_diagnostic_snapshot(),
    )
}

fn build_diagnostic_package_with_ble_snapshot(
    coord: &Arc<Coordinator>,
    ble_snapshot: crate::embedded_ble::BleDiagnosticSnapshot,
) -> Result<DiagnosticPackage, String> {
    let prefs = coord.prefs().get();
    let snap = CredentialsVault::snapshot();
    let active_asr_provider = CredentialsVault::get_active_asr();
    let active_llm_provider = CredentialsVault::get_active_llm();
    let asr_configured = asr_configured_for_provider(&active_asr_provider, &snap);
    let llm_configured = llm_configured_for_provider(&active_llm_provider, &snap);
    let history = coord.history().list().map_err(|e| e.to_string())?;
    let recent_sessions: Vec<DiagnosticRecentSession> = history
        .iter()
        .take(8)
        .map(diagnostic_recent_session)
        .collect();
    let embedded_sessions: Vec<&DiagnosticRecentSession> = recent_sessions
        .iter()
        .filter(|session| session.embedded_audio_stats.is_some())
        .collect();
    let latest_embedded = embedded_sessions.first().copied();
    let log_lines = read_diagnostic_log_tail(600);
    let timeline = diagnostic_timeline(&log_lines, 80);
    let recent_errors = diagnostic_recent_errors(&log_lines, 40);
    let wake_recovery = coord.embedded_ble_wake_recovery_snapshot();
    let failure_taxonomy = diagnostic_ble_failure_taxonomy(
        coord.embedded_ble_listener_last_error(),
        wake_recovery.recent_disconnect_reason.clone(),
        &recent_errors,
        &ble_snapshot,
    );

    Ok(DiagnosticPackage {
        schema_version: 3,
        generated_at: chrono::Utc::now().to_rfc3339(),
        app: DiagnosticApp {
            product_name: "Listener Type",
            identifier: "com.listener.type",
            version: env!("CARGO_PKG_VERSION"),
            log_path: crate::log_dir_path()
                .join("listener-type.log")
                .display()
                .to_string(),
            executable_path: std::env::current_exe()
                .ok()
                .map(|path| path.display().to_string()),
            windows_ime_status: crate::windows_ime_profile::get_windows_ime_status(),
            hotkey_status: coord.hotkey_status(),
        },
        platform: DiagnosticPlatform {
            os: std::env::consts::OS,
            family: std::env::consts::FAMILY,
            arch: std::env::consts::ARCH,
            debug_build: cfg!(debug_assertions),
        },
        firmware: DiagnosticFirmware {
            device_model: "VKA1",
            version: None,
            protocol: "VKA1 BLE audio",
            protocol_version: Some(1),
            readiness: Some(diagnostic_value(&wake_recovery.firmware_wake_policy)),
            wake_policy: wake_recovery.firmware_wake_policy.clone(),
            source: "Desktop readiness includes the accepted firmware 1.1 wake-policy contract; live DIS/readiness is populated by OTA preflight when available.",
        },
        ble: DiagnosticBle {
            input_source: diagnostic_value(&prefs.dictation_input_source),
            enabled_by_input_source: matches!(
                prefs.dictation_input_source,
                crate::types::DictationInputSource::EmbeddedBle
            ),
            background_listener_disabled_by_env: std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
                .ok()
                .is_some_and(|value| value == "1"),
            background_listener_active: coord.embedded_ble_listener_active(),
            background_listener_ready: coord.embedded_ble_listener_ready(),
            background_listener_generation: coord.embedded_ble_listener_generation(),
            service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3091a",
            device_address: ble_snapshot.configured_device_address.clone().or_else(|| {
                ble_snapshot
                    .audio_services
                    .iter()
                    .chain(ble_snapshot.ota_services.iter())
                    .find_map(|entry| entry.bluetooth_address.clone())
            }),
            firmware_version: ble_snapshot.firmware_snapshot.firmware_version.clone(),
            battery_percent: ble_snapshot.firmware_snapshot.battery_percent,
            capabilities: ble_snapshot.firmware_snapshot.capabilities.clone(),
            diagnostic_snapshot: ble_snapshot,
            failure_taxonomy,
            recent_disconnect_reason: wake_recovery.recent_disconnect_reason.clone(),
            reconnect_attempts: wake_recovery.reconnect_attempts,
            notify_subscription_state: format!("{:?}", wake_recovery.notify_subscription_state),
            wake_recovery,
            session_actor_history: coord.embedded_ble_session_actor_diagnostics(),
            recent_embedded_session_count: embedded_sessions.len(),
            latest_session_id: latest_embedded.map(|session| session.id.clone()),
            last_error_code: recent_sessions
                .iter()
                .find_map(|session| session.error_code.clone()),
            last_embedded_audio_end_reason: latest_embedded
                .and_then(|session| session.embedded_audio_stats.as_ref())
                .and_then(|stats| stats.end_reason.as_ref())
                .map(diagnostic_value),
            last_embedded_audio_stats: latest_embedded
                .and_then(|session| session.embedded_audio_stats.clone()),
        },
        config: DiagnosticConfig {
            dictation_input_source: diagnostic_value(&prefs.dictation_input_source),
            active_asr_provider: prefs.active_asr_provider.clone(),
            active_llm_provider: prefs.active_llm_provider.clone(),
            asr_configured,
            llm_configured,
            default_mode: diagnostic_value(&prefs.default_mode),
            active_style_pack_id: prefs.active_style_pack_id.clone(),
            enabled_modes: diagnostic_value(&prefs.enabled_modes),
            show_capsule: prefs.show_capsule,
            start_minimized: prefs.start_minimized,
            launch_at_login: prefs.launch_at_login,
            auto_update_check: prefs.auto_update_check,
            update_channel: diagnostic_value(&prefs.update_channel),
            history_retention_days: prefs.history_retention_days,
            history_max_entries: prefs.history_max_entries,
            record_audio_for_debug: prefs.record_audio_for_debug,
            audio_recording_max_entries: prefs.audio_recording_max_entries,
            local_asr_active_model: prefs.local_asr_active_model.clone(),
            local_asr_keep_loaded_secs: prefs.local_asr_keep_loaded_secs,
            foundry_local_asr_model: prefs.foundry_local_asr_model.clone(),
            foundry_local_runtime_source: prefs.foundry_local_runtime_source.clone(),
            foundry_local_asr_language_hint_configured: !prefs
                .foundry_local_asr_language_hint
                .trim()
                .is_empty(),
            foundry_local_asr_keep_loaded_secs: prefs.foundry_local_asr_keep_loaded_secs,
            microphone_device_configured: !prefs.microphone_device_name.trim().is_empty(),
            marketplace_backend_configured: !prefs.marketplace_base_url.trim().is_empty(),
        },
        credentials: DiagnosticCredentials {
            active_asr_provider: active_asr_provider.clone(),
            active_llm_provider: active_llm_provider.clone(),
            asr_configured,
            llm_configured,
            volcengine_configured: volcengine_configured(&snap),
            asr_api_key_configured: configured(&snap.asr_api_key),
            asr_endpoint_configured: configured(&snap.asr_endpoint),
            asr_model_configured: configured(&snap.asr_model),
            llm_api_key_configured: configured(&snap.ark_api_key),
            llm_endpoint_configured: configured(&snap.ark_endpoint),
            llm_model_configured: configured(&snap.ark_model_id),
            codex_oauth_configured: CodexOAuthCredentials::load_default().is_ok(),
        },
        recent_errors,
        timeline,
        recent_sessions,
        privacy: DiagnosticPrivacy {
            excludes_audio_contents: true,
            excludes_audio_recordings: true,
            excludes_raw_transcripts: true,
            excludes_final_text: true,
            excludes_api_key_values: true,
            credential_values_exported: false,
            notes: vec![
                "History rawTranscript/finalText fields are not exported.",
                "This JSON excludes WAV/audio bytes; the surrounding ZIP may include previously retained debug WAV samples when available.",
                "Credential values, API keys, access tokens and OAuth tokens are not exported.",
            ],
        },
    })
}

fn diagnostic_recent_session(session: &DictationSession) -> DiagnosticRecentSession {
    DiagnosticRecentSession {
        id: session.id.clone(),
        created_at: session.created_at.clone(),
        mode: diagnostic_value(&session.mode),
        insert_status: diagnostic_value(&session.insert_status),
        error_code: session.error_code.clone(),
        duration_ms: session.duration_ms,
        dictionary_entry_count: session.dictionary_entry_count,
        has_audio_recording: session.has_audio_recording,
        embedded_audio_stats: session.embedded_audio_stats.clone(),
    }
}

fn diagnostic_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn diagnostic_ble_failure_taxonomy(
    listener_last_error: Option<String>,
    recent_disconnect_reason: Option<String>,
    recent_errors: &[String],
    snapshot: &crate::embedded_ble::BleDiagnosticSnapshot,
) -> Vec<crate::embedded_ble::BleFailureClassification> {
    let mut messages = Vec::new();
    if let Some(value) = listener_last_error {
        messages.push(value);
    }
    if let Some(value) = recent_disconnect_reason {
        messages.push(value);
    }
    messages.extend(recent_errors.iter().cloned());
    messages.extend(snapshot.errors.iter().cloned());
    if snapshot.firmware_snapshot.connected && snapshot.firmware_snapshot.firmware_version.is_none()
    {
        messages.push("DIS firmware revision missing from live firmware snapshot".to_string());
    }
    if snapshot.audio_services.is_empty() && snapshot.ota_services.is_empty() {
        messages.push("Listener BLE service selectors returned no devices".to_string());
    }

    let mut classifications = Vec::new();
    for message in messages {
        let classification = crate::embedded_ble::classify_ble_failure(&message);
        if !classifications.iter().any(
            |existing: &crate::embedded_ble::BleFailureClassification| {
                existing.kind == classification.kind && existing.evidence == classification.evidence
            },
        ) {
            classifications.push(classification);
        }
    }

    classifications
}

fn read_diagnostic_log_tail(max_lines: usize) -> Vec<String> {
    let path = crate::log_dir_path().join("listener-type.log");
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = content
        .lines()
        .rev()
        .take(max_lines)
        .map(|line| line.to_string())
        .collect();
    lines.reverse();
    lines
}

fn diagnostic_timeline(lines: &[String], max_lines: usize) -> Vec<String> {
    let mut selected: Vec<String> = lines
        .iter()
        .filter(|line| is_diagnostic_timeline_line(line))
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .collect();
    if selected.len() > max_lines {
        selected = selected.split_off(selected.len() - max_lines);
    }
    selected
}

fn diagnostic_recent_errors(lines: &[String], max_lines: usize) -> Vec<String> {
    let mut selected: Vec<String> = lines
        .iter()
        .filter(|line| is_diagnostic_error_line(line))
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .collect();
    if selected.len() > max_lines {
        selected = selected.split_off(selected.len() - max_lines);
    }
    selected
}

fn is_diagnostic_timeline_line(line: &str) -> bool {
    const MARKERS: &[&str] = &[
        "[startup]",
        "[embedded-ble]",
        "[coord]",
        "[asr]",
        "[foundry-asr]",
        "[local-asr]",
        "[windows-ime]",
        "[capsule]",
        "[qa]",
        "ERROR",
        "WARN",
    ];
    MARKERS.iter().any(|marker| line.contains(marker))
}

fn is_diagnostic_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("warn")
        || lower.contains("failed")
        || lower.contains("timeout")
        || lower.contains("panic")
}

fn sanitize_diagnostic_log_line(line: &str) -> Option<String> {
    if diagnostic_line_may_contain_user_text(line) {
        return None;
    }
    let redacted = redact_diagnostic_log_line(line);
    let trimmed: String = redacted.chars().take(480).collect();
    Some(trimmed)
}

fn diagnostic_line_may_contain_user_text(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const TEXT_MARKERS: &[&str] = &[
        "rawtranscript",
        "raw_transcript",
        "raw transcript",
        "finaltext",
        "final_text",
        "final text",
        "transcript=",
        "transcript:",
    ];
    TEXT_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn redact_diagnostic_log_line(line: &str) -> String {
    let mut redact_next = false;
    line.split_whitespace()
        .map(|token| {
            if redact_next {
                redact_next = false;
                return "[redacted]".to_string();
            }
            let lower = token.to_ascii_lowercase();
            if lower == "bearer" || lower == "authorization:" || lower == "authorization" {
                redact_next = true;
                return "[redacted]".to_string();
            }
            if diagnostic_token_contains_secret(&lower) {
                if !(token.contains('=') || token.contains(':')) {
                    redact_next = true;
                }
                return "[redacted]".to_string();
            }
            if looks_like_secret_token(token) {
                return "[redacted]".to_string();
            }
            token.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn diagnostic_token_contains_secret(lower: &str) -> bool {
    const SECRET_MARKERS: &[&str] = &[
        "api_key",
        "apikey",
        "x-goog-api-key",
        "access_key",
        "accesskey",
        "secret_key",
        "secretkey",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
    ];
    SECRET_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn looks_like_secret_token(token: &str) -> bool {
    token.starts_with("sk-")
        || token.starts_with("eyJ")
        || token.starts_with("ya29.")
        || token.len() > 72
            && token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))
}

// ─────────────────────────── unused but exported (silences dead_code) ───────────────────────────

#[allow(dead_code)]
fn _ensure_snapshot_used(_: CredentialsSnapshot) {}

// ─────────────────────────── marketplace (Phase A) ───────────────────────────
//
// 客户端跟 marketplace backend 的 HTTP 客户端封装。Backend URL 走 prefs
// `marketplace_base_url`（默认 http://127.0.0.1:8090 开发；生产用户填 https://api.<domain>）。
// auth：GitHub OAuth device flow token 写入系统 credential vault；上传、点赞、
// 撤回和“我的发布”在调用 marketplace backend 前先用 token 调 GitHub /user
// 取得当前 login，再沿用 v1 backend 的 X-Dev-User 身份头。
//
// IPC -> REST contract v1:
// - marketplace_list      GET    /styles?q=&category=&sort=&limit=&offset=
// - marketplace_detail    GET    /styles/{id}
// - marketplace_install   GET    /styles/{id}, GET /styles/{id}/download
// - marketplace_upload    POST   /styles/upload (multipart zip)
// - marketplace_like      POST   /styles/{id}/like
// - marketplace_delete    DELETE /styles/{id}
// - marketplace_my_likes  GET    /me/likes
// - marketplace_my_packs  GET    /me/styles

/// Listener Type does not inherit any upstream-owned production marketplace.
///
/// Remote marketplace calls are disabled by default. A future Listener Type
/// backend can be enabled by setting `LISTENER_TYPE_MARKETPLACE_BASE_URL` or
/// by writing `prefs.marketplace_base_url` through a controlled config surface.
const MARKETPLACE_BACKEND_DISABLED: &str =
    "Listener Type marketplace backend is not configured; local style packs remain available.";

fn configured_marketplace_url(prefs: &UserPreferences) -> Result<Option<String>, String> {
    let env_url = std::env::var("LISTENER_TYPE_MARKETPLACE_BASE_URL").unwrap_or_default();
    let configured = if env_url.trim().is_empty() {
        prefs.marketplace_base_url.trim()
    } else {
        env_url.trim()
    };
    if configured.is_empty() {
        return Ok(None);
    }
    let parsed =
        reqwest::Url::parse(configured).map_err(|e| format!("invalid marketplace url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("marketplace url must use http or https".into());
    }
    Ok(Some(configured.trim_end_matches('/').to_string()))
}

fn marketplace_url_from_prefs(prefs: &UserPreferences) -> Result<String, String> {
    configured_marketplace_url(prefs)?.ok_or_else(|| MARKETPLACE_BACKEND_DISABLED.to_string())
}

fn marketplace_client_from_prefs(prefs: &UserPreferences) -> Result<MarketplaceClient, String> {
    let base = marketplace_url_from_prefs(prefs)?;
    MarketplaceClient::new(&base).map_err(|error| error.to_string())
}

fn optional_marketplace_client_from_prefs(
    prefs: &UserPreferences,
) -> Result<Option<MarketplaceClient>, String> {
    configured_marketplace_url(prefs)?
        .map(|base| MarketplaceClient::new(&base).map_err(|error| error.to_string()))
        .transpose()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MarketplaceTransferProgressPayload {
    operation: String,
    pack_id: String,
    phase: String,
    progress: u8,
    message: String,
}

fn emit_marketplace_transfer_progress(
    app: &AppHandle,
    operation: &str,
    pack_id: &str,
    phase: &str,
    progress: u8,
    message: &str,
) {
    let payload = MarketplaceTransferProgressPayload {
        operation: operation.to_string(),
        pack_id: pack_id.to_string(),
        phase: phase.to_string(),
        progress,
        message: message.to_string(),
    };
    let _ = app.emit("marketplace-transfer-progress", payload);
}

fn marketplace_api_error_message(action: &str, error: MarketplaceApiError) -> String {
    let guidance = match error.kind() {
        MarketplaceApiErrorKind::InvalidUrl => "backend URL is invalid",
        MarketplaceApiErrorKind::Network => "network interrupted; check the connection and retry",
        MarketplaceApiErrorKind::Unauthorized => "GitHub or marketplace authorization failed",
        MarketplaceApiErrorKind::NotFound => "style pack was not found on the marketplace backend",
        MarketplaceApiErrorKind::HttpStatus => "marketplace backend rejected the request",
        MarketplaceApiErrorKind::Decode => "marketplace backend returned an invalid response",
    };
    format!("{action} failed: {guidance}: {error}")
}

async fn marketplace_authenticated_github_login() -> Result<String, String> {
    let client = GithubOAuthClient::production().map_err(|error| error.to_string())?;
    marketplace_authenticated_github_login_with_client(&client).await
}

async fn marketplace_authenticated_github_login_with_client(
    client: &GithubOAuthClient,
) -> Result<String, String> {
    let mut credentials = CredentialsVault::marketplace_github_credentials()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "未登录：请先用 GitHub OAuth 登录风格市场".to_string())?;

    let now = current_epoch_secs();
    if token_needs_refresh(&credentials, now) {
        let client_id = get_github_oauth_client_id()?;
        credentials = refresh_marketplace_github_credentials(client, &client_id, credentials, now)
            .await
            .map_err(|error| error.to_string())?;
    }

    let user = client
        .authenticated_user(&credentials.access_token)
        .await
        .map_err(|error| format!("GitHub 用户信息验证失败：{error}"))?;

    if credentials.login != user.login {
        credentials.login = user.login.clone();
        CredentialsVault::set_marketplace_github_credentials(credentials)
            .map_err(|error| error.to_string())?;
    }

    Ok(user.login)
}

async fn refresh_marketplace_github_credentials(
    client: &GithubOAuthClient,
    client_id: &str,
    credentials: MarketplaceGithubCredentials,
    now_epoch_secs: i64,
) -> Result<MarketplaceGithubCredentials, GithubOAuthError> {
    let Some(refresh_token) = credentials
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    else {
        return Err(GithubOAuthError::RefreshUnavailable);
    };
    if refresh_token_is_expired(&credentials, now_epoch_secs) {
        return Err(GithubOAuthError::RefreshExpired);
    }

    let token = client
        .refresh_access_token(client_id, refresh_token)
        .await?;
    let login = credentials.login;
    let mut refreshed = token.into_credentials(login, current_epoch_secs());
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = Some(refresh_token.to_string());
    }
    if refreshed.refresh_token_expires_at_epoch_secs.is_none() {
        refreshed.refresh_token_expires_at_epoch_secs =
            credentials.refresh_token_expires_at_epoch_secs;
    }
    CredentialsVault::set_marketplace_github_credentials(refreshed.clone())
        .map_err(|error| GithubOAuthError::OAuth(error.to_string()))?;
    Ok(refreshed)
}

#[tauri::command]
pub async fn marketplace_list(
    coord: CoordinatorState<'_>,
    query: Option<String>,
    category: Option<String>,
    sort: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<MarketplaceListPage, String> {
    let prefs = coord.prefs().get();
    let Some(client) = optional_marketplace_client_from_prefs(&prefs)? else {
        return Ok(MarketplaceListPage::empty());
    };
    client
        .list_styles(
            query.as_deref(),
            category.as_deref(),
            sort.as_deref(),
            limit,
            offset,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn marketplace_detail(
    coord: CoordinatorState<'_>,
    pack_id: String,
) -> Result<MarketplaceDetail, String> {
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    marketplace_client_from_prefs(&prefs)?
        .style_detail(&pack_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn marketplace_install(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    pack_id: String,
) -> Result<StylePack, String> {
    // 安全校验：pack_id 来自远端 backend，可能含路径遍历 segment。
    // 用跟 read_audio_recording 同样的 UUID-v4 白名单挡住 ../ / 绝对路径等。
    // backend 当前用 Uuid::new_v4 生成所有 id，合法 id 必然匹配。
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;

    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "metadata",
        15,
        "Reading marketplace style pack details",
    );
    // 先拉 detail 拿 authorLogin —— 装好后本地写 originAuthorLogin，
    // 后续编辑+发布时 backend 据此判 supersede（原作者）vs derivative（他人 fork）。
    let detail = client
        .style_detail(&pack_id)
        .await
        .map_err(|error| marketplace_api_error_message("marketplace detail", error))?;
    let origin_author_login = if detail.summary.author_login.trim().is_empty() {
        None
    } else {
        Some(detail.summary.author_login)
    };

    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "downloading",
        35,
        "Downloading style pack archive",
    );
    let bytes = client
        .download_style_archive(&pack_id)
        .await
        .map_err(|error| marketplace_api_error_message("marketplace download", error))?;

    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "installing",
        70,
        "Validating and installing style pack archive",
    );
    // pack_id 已经过 UUID 白名单，拼临时文件路径安全。
    let tmp = std::env::temp_dir().join(format!("listener-type-marketplace-{pack_id}.zip"));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("write temporary style pack zip: {e}"))?;
    let imported_result = coord
        .style_packs()
        .import_from_zip(&tmp)
        .map_err(|e| format!("style pack format validation failed: {e}"));
    let _ = std::fs::remove_file(&tmp);
    let imported = imported_result?;

    // 绑定 origin —— 后续编辑+发布走 derivative / supersede 分支。
    let imported = coord
        .style_packs()
        .set_origin(&imported.id, Some(pack_id.clone()), origin_author_login)
        .map_err(|e| format!("set origin failed: {e}"))?;
    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "finished",
        100,
        "Installed locally",
    );
    Ok(imported)
}

#[tauri::command]
pub async fn marketplace_upload(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    pack_id: String,
    origin_pack_id: Option<String>,
) -> Result<serde_json::Value, String> {
    // 本地 pack id 形态：`builtin.light` / 用户 slug / Uuid。用 local 白名单挡 `..` / `/` / `\`。
    if !is_valid_local_pack_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "auth",
        10,
        "Verifying GitHub marketplace login",
    );
    let github_login = marketplace_authenticated_github_login().await?;

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "validating",
        25,
        "Validating local style pack",
    );
    // 拉本地 pack 拿 origin_pack_id —— 装过的 pack 这里有值，
    // backend 据此判同作者就 supersede 原行（新版本），他人就 derivative（独立新 row）。
    let local_pack = coord
        .style_packs()
        .get(&pack_id)
        .map_err(|e| format!("local pack not found: {e}"))?;
    if local_pack.kind == StylePackKind::Builtin {
        return Err(
            "builtin style packs cannot be uploaded; duplicate it as an editable pack first".into(),
        );
    }
    let origin_pack_id = origin_pack_id
        .filter(|id| is_valid_session_id(id))
        .or_else(|| local_pack.origin_pack_id.clone());

    // 先 export 本地 pack → 临时 ZIP
    let tmp = std::env::temp_dir().join(format!("listener-type-marketplace-upload-{pack_id}.zip"));
    coord
        .style_packs()
        .export_to_zip(&pack_id, &tmp)
        .map_err(|e| format!("style pack format validation failed before upload: {e}"))?;
    let bytes = std::fs::read(&tmp).map_err(|e| format!("read validated style pack zip: {e}"))?;
    let _ = std::fs::remove_file(&tmp);

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "uploading",
        60,
        "Uploading style pack archive to marketplace backend",
    );
    let parsed = client
        .upload_style_archive(&pack_id, origin_pack_id.as_deref(), bytes, &github_login)
        .await
        .map_err(|error| marketplace_api_error_message("marketplace upload", error))?;

    // 本地从未绑定 origin（首次上传一个本地原创 pack）→ 把 backend 分配的 pack id 写回本地，
    // 让用户在同设备上后续编辑能继续走「同作者 supersede」分支，更新自己原创的包。
    if origin_pack_id.is_none() {
        if let Some(remote_id) = parsed.get("id").and_then(|v| v.as_str()) {
            let _ = coord.style_packs().set_origin(
                &pack_id,
                Some(remote_id.to_string()),
                Some(github_login.clone()),
            );
        }
    }

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "finished",
        100,
        "Uploaded to marketplace backend",
    );
    Ok(parsed)
}

#[tauri::command]
pub async fn marketplace_like(
    coord: CoordinatorState<'_>,
    pack_id: String,
) -> Result<serde_json::Value, String> {
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;
    let github_login = marketplace_authenticated_github_login().await?;
    client
        .like_style(&pack_id, &github_login)
        .await
        .map_err(|error| format!("like request failed: {error}"))
}

/// 撤回自己发布的 pack（后端软删 state='withdrawn'，前端列表不再可见）。
/// pack_id 来自远端，必须是 UUID-v4。
#[tauri::command]
pub async fn marketplace_delete(
    coord: CoordinatorState<'_>,
    pack_id: String,
) -> Result<(), String> {
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;
    let github_login = marketplace_authenticated_github_login().await?;
    client
        .delete_style(&pack_id, &github_login)
        .await
        .map_err(|error| format!("delete request failed: {error}"))
}

/// 拉当前用户赞过的所有 pack id，用于客户端市场页面渲染红心 + 「我赞过的」过滤。
#[tauri::command]
pub async fn marketplace_my_likes(coord: CoordinatorState<'_>) -> Result<Vec<String>, String> {
    let prefs = coord.prefs().get();
    let Some(client) = optional_marketplace_client_from_prefs(&prefs)? else {
        return Ok(Vec::new());
    };
    let github_login = match marketplace_authenticated_github_login().await {
        Ok(login) => login,
        Err(error) => {
            log::info!("[marketplace] my-likes skipped: {error}");
            return Ok(Vec::new());
        }
    };
    if github_login.is_empty() {
        return Ok(Vec::new()); // 未登录就空集合，UI 渲染无红心
    }
    client
        .my_likes(&github_login)
        .await
        .map_err(|error| format!("my-likes request failed: {error}"))
}

/// 拉当前用户发布过的 pack（含审核中/已通过/已拒绝/已撤回），用于「我的发布」页面。
#[tauri::command]
pub async fn marketplace_my_packs(
    coord: CoordinatorState<'_>,
) -> Result<Vec<MarketplaceMyPackItem>, String> {
    let prefs = coord.prefs().get();
    let Some(client) = optional_marketplace_client_from_prefs(&prefs)? else {
        return Ok(Vec::new());
    };
    let github_login = match marketplace_authenticated_github_login().await {
        Ok(login) => login,
        Err(error) => {
            log::info!("[marketplace] my-packs skipped: {error}");
            return Ok(Vec::new());
        }
    };
    if github_login.is_empty() {
        return Ok(Vec::new());
    }
    client
        .my_styles(&github_login)
        .await
        .map_err(|error| format!("my-packs request failed: {error}"))
}

// ─────────────────────── GitHub OAuth Device Flow (Phase 1) ───────────────────────
//
// Rust 后端直连 GitHub 拿 access_token + login。token 只写入系统
// credential vault；前端只拿 login 用于展示/兼容现有上传按钮状态。
// Listener Type 后端未上线前，默认不内置 OAuth App，因此该能力必须由环境变量
// 或未来配置显式开启。
//
// 配置 client_id 的两种方式（OAuth App client_id 非敏感，但必须使用 Listener Type 自有 App）：
//   1. 生产构建可在下方 GITHUB_OAUTH_CLIENT_ID 常量填 Listener Type 自有值
//   2. 启动前设置环境变量 GITHUB_OAUTH_CLIENT_ID=<your_client_id>
//
// 注册 OAuth App：
//   https://github.com/settings/applications/new
//   - Application name: Listener Type (or your fork)
//   - Homepage URL: https://github.com/Listener-ai-Macau/Listener-Type
//   - Authorization callback URL: http://localhost (Device Flow 不真用，但表单要求填)
//   - 创建后在 General 页面勾选 "Enable Device Flow"
//   - 抄 client_id 填到本常量

const GITHUB_OAUTH_CLIENT_ID: &str = "";

fn get_github_oauth_client_id() -> Result<String, String> {
    if let Ok(env_id) = std::env::var("GITHUB_OAUTH_CLIENT_ID") {
        let trimmed = env_id.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if !GITHUB_OAUTH_CLIENT_ID.is_empty() {
        return Ok(GITHUB_OAUTH_CLIENT_ID.to_string());
    }
    Err("GitHub OAuth 未配置。请为 Listener Type 注册自有 OAuth App\
        （必须勾 Enable Device Flow），把 client_id 填到 \
        src-tauri/src/commands.rs 的 GITHUB_OAUTH_CLIENT_ID 常量，\
        或在启动前设置环境变量 GITHUB_OAUTH_CLIENT_ID=<your_client_id>。"
        .to_string())
}

#[tauri::command]
pub async fn github_device_flow_start() -> Result<GithubDeviceStartResponse, String> {
    let client_id = get_github_oauth_client_id()?;
    let client = GithubOAuthClient::production().map_err(|error| error.to_string())?;
    client
        .start_device_flow(&client_id, "read:user")
        .await
        .map_err(|error| format!("调用 GitHub /login/device/code 失败：{error}"))
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GithubDevicePollResult {
    Authorized { login: String },
    Pending,
    SlowDown,
    Error { message: String },
}

#[tauri::command]
pub async fn github_device_flow_poll(
    device_code: String,
) -> Result<GithubDevicePollResult, String> {
    let client_id = get_github_oauth_client_id()?;
    let client = GithubOAuthClient::production().map_err(|error| error.to_string())?;
    let poll = client
        .poll_device_flow(&client_id, &device_code)
        .await
        .map_err(|error| format!("调用 GitHub /login/oauth/access_token 失败：{error}"))?;

    let token = match poll {
        GithubDevicePollStatus::Authorized(token) => token,
        GithubDevicePollStatus::Pending => return Ok(GithubDevicePollResult::Pending),
        GithubDevicePollStatus::SlowDown => return Ok(GithubDevicePollResult::SlowDown),
        GithubDevicePollStatus::Expired => {
            return Ok(GithubDevicePollResult::Error {
                message: "OAuth 设备码已过期，请重新发起登录".to_string(),
            })
        }
        GithubDevicePollStatus::AccessDenied => {
            return Ok(GithubDevicePollResult::Error {
                message: "你在 GitHub 上拒绝了授权".to_string(),
            })
        }
        GithubDevicePollStatus::Error(message) => {
            return Ok(GithubDevicePollResult::Error { message })
        }
    };

    let user = client
        .authenticated_user(&token.access_token)
        .await
        .map_err(|error| format!("调用 GitHub /user 失败：{error}"))?;
    let credentials = token.into_credentials(user.login.clone(), current_epoch_secs());
    CredentialsVault::set_marketplace_github_credentials(credentials)
        .map_err(|error| format!("保存 GitHub OAuth token 失败：{error}"))?;
    Ok(GithubDevicePollResult::Authorized { login: user.login })
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use super::release_foundry_runtime_if_inactive;
    use super::{
        active_asr_is_keyless_for_validation, active_foundry_model_from_prefs,
        asr_configured_for_provider, asr_transcriptions_url, device_settings_update_commands,
        diagnostic_recent_errors, fetch_provider_models, fetch_provider_models_cached,
        firmware_ota_snapshot_version, firmware_ota_versions_match, is_diagnostic_error_line,
        is_gemini_base_url, is_valid_local_pack_id, is_valid_session_id,
        llm_configured_for_provider, load_firmware_ota_package,
        local_asr_release_plan_for_provider, models_url, normalize_foundry_language_hint,
        parse_gemini_model_ids, parse_model_ids, persist_settings, provider_models_cache,
        sanitize_diagnostic_log_line, validate_device_settings_request,
        validate_foundry_model_alias, DeviceSettingsUpdateRequest, ProviderConfig, SettingsWriter,
        DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES,
    };
    use crate::coordinator::Coordinator;
    use crate::embedded_audio::{SessionEndReason, SessionErrorCode, SessionStats};
    use crate::embedded_ble::FirmwareOtaDeviceSnapshot;
    use crate::persistence::CredentialsSnapshot;
    use crate::polish::ProviderProxyConfig;
    use crate::types::{
        ComboBinding, DeviceCustomKeyAction, DeviceCustomKeyMapping, DictationSession,
        HotkeyBinding, HotkeyMode, HotkeyTrigger, InsertStatus, PolishMode, ShortcutBinding,
        UserPreferences, MAX_DEVICE_LOW_POWER_IDLE_MINUTES,
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

    fn ota_snapshot_with_version(version: Option<&str>) -> FirmwareOtaDeviceSnapshot {
        FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: Some("keyboard-v1".to_string()),
            firmware_version: version.map(str::to_string),
            capabilities: vec!["firmware_ota_v1".to_string()],
            battery_percent: Some(80),
            usb_powered: Some(true),
            detail: None,
        }
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
            plugged_auto_shutdown_minutes: 30,
            battery_auto_shutdown_minutes: 30,
            ble_name: "listener-dev".to_string(),
        };

        assert!(validate_device_settings_request(&request).is_err());
    }

    #[test]
    fn device_settings_snapshot_uses_firmware_readback_values() {
        let snapshot = super::device_settings_snapshot_from_status(
            crate::embedded_ble::DeviceSettingsStatus {
                status_led_brightness_percent: 70,
                key_led_brightness_percent: 65,
                knob_led_brightness_percent: 60,
                edge_led_brightness_percent: 55,
                led_zone_brightness_supported: true,
                low_power_idle_minutes: 3,
                plugged_low_power_idle_minutes: 2,
                battery_low_power_idle_minutes: 3,
                plugged_low_power_enabled: true,
                plugged_auto_shutdown_minutes: 0,
                battery_auto_shutdown_minutes: 30,
                knob_rotation_action: "screen_brightness".to_string(),
                ble_name: "listener-dev".to_string(),
                ble_name_pending_restart: true,
                external_power_present: true,
                usb_power_present: true,
                charging: false,
                charge_full: false,
                raw_line: "~DEVICE:SETTINGS result=OK".to_string(),
            },
        );

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
    fn device_settings_update_commands_fit_ble_audio_control() {
        let request = DeviceSettingsUpdateRequest {
            status_led_brightness_percent: 100,
            key_led_brightness_percent: 100,
            knob_led_brightness_percent: 100,
            edge_led_brightness_percent: 100,
            plugged_low_power_idle_minutes: 1440,
            battery_low_power_idle_minutes: 1440,
            plugged_low_power_enabled: false,
            plugged_auto_shutdown_minutes: 0,
            battery_auto_shutdown_minutes: 1440,
            ble_name: "listener-12345678901234567890123".to_string(),
        };
        let commands =
            device_settings_update_commands(&request, true, request.plugged_low_power_enabled)
                .expect("commands");

        assert_eq!(commands.len(), 8);
        assert!(commands
            .iter()
            .any(|command| { command == "DEVICE:SET plugged_low_power_enabled=0" }));
        assert!(commands
            .iter()
            .any(|command| { command == "DEVICE:SET plugged_auto_shutdown_minutes=off" }));
        assert!(commands.iter().all(|command| {
            command.as_bytes().len() + 1 <= DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES
        }));

        let legacy_commands =
            device_settings_update_commands(&request, false, request.plugged_low_power_enabled)
                .expect("commands");
        assert_eq!(legacy_commands.len(), 6);
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
            plugged_auto_shutdown_minutes: 0,
            battery_auto_shutdown_minutes: 0,
            ble_name: "listener-dev".to_string(),
        };

        assert!(validate_device_settings_request(&request).is_ok());
        let plugged_low_power_enabled =
            request.plugged_low_power_enabled && request.plugged_low_power_idle_minutes > 0;
        let commands = device_settings_update_commands(&request, false, plugged_low_power_enabled)
            .expect("commands");

        assert!(commands
            .iter()
            .any(|command| { command == "DEVICE:SET plugged_low_power_idle_minutes=0" }));
        assert!(commands
            .iter()
            .any(|command| { command == "DEVICE:SET battery_low_power_idle_minutes=0" }));
        assert!(commands
            .iter()
            .any(|command| { command == "DEVICE:SET plugged_low_power_enabled=0" }));
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
            super::embedded_ble_repair_failure_action(&stale);
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
            super::embedded_ble_repair_failure_action(&transient);
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

        let (user_action_required, open_bluetooth_settings) =
            super::embedded_ble_repair_failure_action(&cccd);
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

        assert!(super::should_attempt_embedded_ble_auto_unpair(&cccd));
        assert!(super::should_attempt_embedded_ble_auto_unpair(&stale));
        assert!(super::should_attempt_embedded_ble_auto_unpair(
            &missing_pairing
        ));
        assert!(!super::should_attempt_embedded_ble_auto_unpair(&transient));
        assert!(!super::should_attempt_embedded_ble_auto_unpair(&asleep));
        assert_eq!(
            super::embedded_ble_recovery_action_for_failure(&transient),
            super::EmbeddedBleRecoveryAction::WaitForAutomaticRecovery
        );
        assert_eq!(
            super::embedded_ble_recovery_action_for_failure(&cccd),
            super::EmbeddedBleRecoveryAction::RePairRequired
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
            notify_subscription_state:
                crate::coordinator::EmbeddedBleNotifySubscriptionState::Opening,
            ..Default::default()
        };

        assert!(super::runtime_suggests_embedded_ble_auto_unpair(
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
            notify_subscription_state:
                crate::coordinator::EmbeddedBleNotifySubscriptionState::Opening,
            usb_powered: Some(false),
            ..wake_recovery
        };
        assert!(!super::runtime_suggests_embedded_ble_auto_unpair(
            "Listener BLE notify subscription did not recover within 15000 ms after foreground probe",
            None,
            &idle_recovery,
        ));

        let powered_idle_recovery = crate::coordinator::EmbeddedBleWakeRecoverySnapshot {
            usb_powered: Some(true),
            ..idle_recovery
        };
        assert!(super::runtime_suggests_embedded_ble_auto_unpair(
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

        let message = super::embedded_ble_recovery_message(&failure, Some(&unpair));
        assert!(message.contains("重新配对"));
        assert!(!message.to_ascii_lowercase().contains("cccd"));
        assert!(!message.to_ascii_lowercase().contains("gatt"));
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

        let (user_action_required, open_bluetooth_settings) =
            super::embedded_ble_repair_failure_action(&idle);
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
            super::embedded_ble_repair_failure_action(&missing_pairing),
            (true, true)
        );
        assert_eq!(
            super::embedded_ble_repair_failure_action(&asleep),
            (true, false)
        );
    }

    #[test]
    fn load_firmware_ota_package_reads_directory() {
        let root =
            std::env::temp_dir().join(format!("listener-ota-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp package dir");
        std::fs::write(root.join("ota_manifest.json"), "{\"schema_version\":2}")
            .expect("write manifest");
        std::fs::write(root.join("firmware_ota.bin"), [1u8, 2, 3]).expect("write firmware");

        let payload = load_firmware_ota_package(root.to_string_lossy().to_string())
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

        let payload = load_firmware_ota_package(zip_path.to_string_lossy().to_string())
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

        let ota_payload = load_firmware_ota_package(zip_path.to_string_lossy().to_string())
            .expect("load OTA files from combined release zip");
        assert_eq!(ota_payload.manifest_text, "{\"schema_version\":2}");
        assert_eq!(ota_payload.firmware_bytes, vec![4, 5, 6]);

        let wired = super::load_wired_firmware_package_internal(&zip_path)
            .expect("load factory files from combined release zip");
        assert_eq!(wired.kind, super::WiredFirmwarePackageKind::Factory);
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

        let loaded = super::load_wired_firmware_package_internal(&root)
            .expect("load factory wired firmware package");
        assert_eq!(loaded.kind, super::WiredFirmwarePackageKind::Factory);
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

        let loaded = super::load_wired_firmware_package_internal(&root)
            .expect("load nested factory wired firmware package");
        assert_eq!(loaded.kind, super::WiredFirmwarePackageKind::Factory);
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

        let err = super::load_wired_firmware_package_internal(&zip_path)
            .expect_err("OTA zip should not be accepted for wired factory flashing");
        assert!(err.contains("factory package format"), "{err}");
        let _ = std::fs::remove_file(&zip_path);
    }

    #[test]
    fn normalize_esptool_region_arg_accepts_idf_size_units() {
        assert_eq!(
            super::normalize_esptool_region_arg("8K", "size", false).unwrap(),
            "8192"
        );
        assert_eq!(
            super::normalize_esptool_region_arg("6M", "size", false).unwrap(),
            "6291456"
        );
        assert_eq!(
            super::normalize_esptool_region_arg("0xf000", "offset", true).unwrap(),
            "0xf000"
        );
        assert!(super::normalize_esptool_region_arg("0K", "size", false).is_err());
    }

    #[test]
    fn parse_flash_u32_arg_accepts_idf_size_units() {
        assert_eq!(
            super::parse_flash_u32_arg("8K", "size", false).unwrap(),
            8192
        );
        assert_eq!(
            super::parse_flash_u32_arg("0xf000", "offset", true).unwrap(),
            0xf000
        );
        assert!(super::parse_flash_u32_arg("5G", "size", false).is_err());
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
        let digest = super::sha256_digest_bytes(&image[..digest_start]);
        image[digest_start..].copy_from_slice(&digest);

        let (patched, report) =
            super::prepare_esp_image_for_wired_flash("app", &image, super::Chip::Esp32s3)
                .expect("patch ESP image");

        assert!(report.changed);
        assert!(report.digest_recalculated);
        assert_eq!(patched[2], super::WIRED_FLASH_MODE as u8);
        assert_eq!(patched[3], 0x4f);
        let patched_digest = super::sha256_digest_bytes(&patched[..digest_start]);
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
            terminal_received: true,
            end_reason: Some(SessionEndReason::Error(SessionErrorCode::QueueFull)),
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
                    id: r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3091A}_14C19F48FE72"
                        .to_string(),
                    bluetooth_address: Some("14C19F48FE72".to_string()),
                }],
                ota_services: Vec::new(),
                diagnostic_services: Vec::new(),
                firmware_snapshot: FirmwareOtaDeviceSnapshot {
                    connected: true,
                    hardware_revision: Some("keyboard-v1".to_string()),
                    firmware_version: Some("v1.2.3".to_string()),
                    capabilities: vec!["firmware_ota_v1".to_string()],
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
                    id: r"BTHLEDEVICE\{710AF845-6D9F-6583-0C4D-9E5B3BC3093A}_14C19F48FE72"
                        .to_string(),
                    bluetooth_address: Some("14C19F48FE72".to_string()),
                }],
                firmware_snapshot: FirmwareOtaDeviceSnapshot {
                    connected: true,
                    hardware_revision: Some("keyboard-v1".to_string()),
                    firmware_version: Some("v1.2.3".to_string()),
                    capabilities: vec!["firmware_ota_v1".to_string(), "diag_export_v1".to_string()],
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
                capabilities: vec!["firmware_ota_v1".to_string()],
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
        assert!(kinds
            .contains(&crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded));
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
        assert!(
            validate_foundry_model_alias(crate::asr::local::foundry::DEFAULT_MODEL_ALIAS).is_ok()
        );
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
            parse_model_ids(r#"{ "data": [{ "id": "b" }, { "id": "a" }, { "id": "b" }] }"#)
                .unwrap();
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

        assert!(
            super::reject_dictation_translation_hotkey_overlap(&dictation, &translation).is_ok()
        );
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
}
