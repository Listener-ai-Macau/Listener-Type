//! Tauri command surface — every IPC entry the React UI invokes lives here.

use std::collections::{hash_map::DefaultHasher, BTreeMap};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::asr::local::foundry::{
    model_alias_is_known, FoundryCatalogModel, FoundryPrepareProgressPayload, FoundryRuntimeStatus,
    DEFAULT_MODEL_ALIAS, PROVIDER_ID as FOUNDRY_LOCAL_PROVIDER_ID,
};
use crate::asr::local::FoundryLocalRuntime;
use crate::coordinator::{
    Coordinator, EmbeddedBleSessionActorDiagnosticRecord, EmbeddedBleWakeRecoverySnapshot,
    FirmwareWakePolicySnapshot,
};
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
    builtin_style_pack_id, default_active_style_pack_id, CapsulePayload, ChineseScriptPreference,
    ComboBinding, CorrectionRule, CredentialsStatus, DeviceCustomKeyAction, DeviceCustomKeyGesture,
    DeviceCustomKeyId, DeviceCustomKeyMapping, DeviceCustomKeys, DictationInputSource,
    DictationSession, DictionaryEntry, HotkeyCapability, HotkeyStatus, OutputLanguagePreference,
    PolishMode, ShortcutBinding, StylePack, StylePackKind, StylePackRuntimeDiagnostics,
    StyleSystemPrompts, UserPreferences, VocabPresetStore, WindowsImeStatus,
};

type CoordinatorState<'a> = State<'a, Arc<Coordinator>>;

pub mod device;
pub(crate) use device::*;
pub use device::{
    DeviceSettingsSnapshot, DeviceSettingsUpdateRequest, EmbeddedBleRepairResult,
    EmbeddedBleRuntimeStatus, FirmwareOtaBleTransferResult, FirmwareOtaPackagePayload,
    FirmwareOtaPreflightSnapshot, WiredFirmwareFlashResult, WiredFirmwarePackagePayload,
    WiredFirmwareSerialPort,
};

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
pub fn get_voiceprint_status(
    coord: CoordinatorState<'_>,
) -> crate::speaker_verification::VoiceprintStatus {
    let wake_phrase = coord.prefs().get().voice_wake_phrase;
    crate::speaker_verification::status_for_phrase(&wake_phrase)
}

#[tauri::command]
pub async fn start_voiceprint_enrollment(
    coord: CoordinatorState<'_>,
) -> Result<crate::speaker_verification::VoiceprintStatus, String> {
    let wake_phrase = coord.prefs().get().voice_wake_phrase;
    tauri::async_runtime::spawn_blocking(move || {
        crate::speaker_verification::start_enrollment(&wake_phrase)
    })
    .await
    .map_err(|err| format!("声纹登记任务失败: {err}"))?
}

#[tauri::command]
pub async fn cancel_voiceprint_enrollment(
    coord: CoordinatorState<'_>,
) -> Result<crate::speaker_verification::VoiceprintStatus, String> {
    let wake_phrase = coord.prefs().get().voice_wake_phrase;
    tauri::async_runtime::spawn_blocking(move || {
        crate::speaker_verification::cancel_enrollment(&wake_phrase)
    })
    .await
    .map_err(|err| format!("取消声纹登记任务失败: {err}"))?
}

#[tauri::command]
pub async fn delete_voiceprint(
    coord: CoordinatorState<'_>,
) -> Result<crate::speaker_verification::VoiceprintStatus, String> {
    let wake_phrase = coord.prefs().get().voice_wake_phrase;
    tauri::async_runtime::spawn_blocking(move || {
        crate::speaker_verification::delete_template()?;
        Ok(crate::speaker_verification::status_for_phrase(&wake_phrase))
    })
    .await
    .map_err(|err| format!("删除声纹任务失败: {err}"))?
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
pub fn record_ui_timeline_event(coord: CoordinatorState<'_>, payload: UiTimelineEvent) {
    if payload.source == "frontend.capsule"
        && payload.event == "visible"
        && payload.state.as_deref() == Some("recording")
    {
        if let Some(session_id) = payload
            .detail
            .as_ref()
            .and_then(|detail| detail.get("sessionId"))
            .and_then(Value::as_str)
        {
            coord.acknowledge_automatic_wake_capsule_visible(session_id);
        }
    }
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
                .map(crate::capsule_log::sanitize_diagnostic_value)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "{}".into())
        ),
    );
}

/// Restore the latest display-only capsule state after the capsule WebView has
/// been recreated or missed an event while hidden. It cannot trigger dictation
/// or insertion.
#[tauri::command]
pub fn get_capsule_state(coord: CoordinatorState<'_>) -> Option<CapsulePayload> {
    coord.capsule_latest_payload()
}

trait SettingsWriter {
    fn sync_active_providers(
        &self,
        active_asr_provider: &str,
        active_llm_provider: &str,
    ) -> Result<(), String>;
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
    fn sync_active_providers(
        &self,
        active_asr_provider: &str,
        active_llm_provider: &str,
    ) -> Result<(), String> {
        let active_asr_provider = active_asr_provider.trim();
        if !active_asr_provider.is_empty() {
            CredentialsVault::set_active_asr_provider(active_asr_provider)
                .map_err(|e| e.to_string())?;
        }
        let active_llm_provider = active_llm_provider.trim();
        if !active_llm_provider.is_empty() {
            CredentialsVault::set_active_llm_provider(active_llm_provider)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

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
    fn sync_active_providers(
        &self,
        active_asr_provider: &str,
        active_llm_provider: &str,
    ) -> Result<(), String> {
        (**self).sync_active_providers(active_asr_provider, active_llm_provider)
    }

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
    prefs.active_asr_provider_default_migrated = true;
    prefs.device_custom_keys_default_migrated = true;
    reject_hotkey_collisions(&prefs)?;
    validate_device_custom_keys(&prefs.device_custom_keys)?;
    validate_device_custom_keys(&prefs.device_custom_key_double_clicks)?;
    validate_device_custom_keys(&prefs.device_custom_key_long_presses)?;
    coord.sync_active_providers(&prefs.active_asr_provider, &prefs.active_llm_provider)?;
    let device_ble_name = prefs.device_ble_name.clone();
    coord.write_settings(prefs)?;
    crate::embedded_ble::set_configured_bluetooth_target_name(&device_ble_name);
    coord.refresh_dictation_hotkey();
    coord.refresh_qa_hotkey();
    coord.refresh_combo_hotkey();
    coord.refresh_translation_hotkey();
    coord.refresh_switch_style_hotkey();
    coord.refresh_open_app_hotkey();
    coord.refresh_device_custom_key_hotkeys();
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
    let previous_wake_phrase =
        crate::wake_phrase::normalize_configured_phrase(&previous_prefs.voice_wake_phrase)?;
    let next_wake_phrase =
        crate::wake_phrase::normalize_configured_phrase(&prefs.voice_wake_phrase)?;
    let wake_phrase_changed = previous_wake_phrase != next_wake_phrase;
    prefs.voice_wake_phrase = next_wake_phrase.clone();
    if wake_phrase_changed {
        #[cfg(target_os = "windows")]
        crate::wake_phrase::prepare(&next_wake_phrase)?;
        crate::speaker_verification::invalidate_for_phrase_change(
            &previous_wake_phrase,
            &next_wake_phrase,
        )?;
    }
    if prefs.dictation_input_source != previous_prefs.dictation_input_source {
        prefs.dictation_input_source_user_overridden = true;
    }
    if !prefs.dictation_input_source_user_overridden {
        prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    }
    // Keep the retired streaming-only field synchronized for downgrade compatibility.
    prefs.streaming_insert_save_clipboard = prefs.copy_dictation_to_clipboard;
    let next_input_source = prefs.dictation_input_source;
    let input_source_changed = next_input_source != previous_prefs.dictation_input_source;
    let should_sync_device_firmware =
        device::device_firmware_settings_changed(&previous_prefs, &prefs);
    if should_sync_device_firmware {
        device::validate_device_firmware_preferences(&prefs)?;
        device::sync_device_firmware_preferences(&previous_prefs, &prefs)?;
    }
    // 广播给所有 webview。issue #205：QaPanel 跑在独立 webview，
    // 没有 HotkeySettingsContext，必须靠事件感知录音键变化，否则面板可见时
    // 用户改键会让浮窗里的 "{recordHotkey}" 文案一直停留在旧值。
    persist_settings(&*coord, prefs.clone())?;
    if input_source_changed {
        coord.refresh_embedded_ble_listener();
    }
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
pub fn get_credentials(coord: CoordinatorState<'_>) -> CredentialsStatus {
    let snap = CredentialsVault::snapshot();
    let prefs = coord.prefs().get();
    let active_asr_provider = if prefs.active_asr_provider.trim().is_empty() {
        CredentialsVault::get_active_asr()
    } else {
        prefs.active_asr_provider.clone()
    };
    let active_llm_provider = if prefs.active_llm_provider.trim().is_empty() {
        CredentialsVault::get_active_llm()
    } else {
        prefs.active_llm_provider.clone()
    };
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
    let provider = provider.trim().to_string();
    if provider.is_empty() {
        return Err("ASR provider id is empty".to_string());
    }
    if provider == FOUNDRY_LOCAL_PROVIDER_ID && !active_foundry_asr_is_supported(&provider) {
        return Err("Foundry Local Whisper is only available on Windows".to_string());
    }
    CredentialsVault::set_active_asr_provider(&provider).map_err(|e| e.to_string())?;
    {
        let _settings_guard = settings_update_lock().lock();
        let mut prefs = coord.prefs().get();
        if prefs.active_asr_provider != provider || !prefs.active_asr_provider_default_migrated {
            let previous_provider = prefs.active_asr_provider.clone();
            prefs.active_asr_provider = provider.clone();
            prefs.active_asr_provider_default_migrated = true;
            if let Err(err) = coord.prefs().set(prefs) {
                let _ = CredentialsVault::set_active_asr_provider(&previous_provider);
                return Err(err.to_string());
            }
        }
    }
    let release_plan = local_asr_release_plan_for_provider(&provider);
    if provider == crate::asr::local::PROVIDER_ID {
        // 切到本地 ASR → 后台预加载模型，下次按 hotkey 时不必等数秒。
        coord.preload_local_asr_in_background();
    }
    if provider == FOUNDRY_LOCAL_PROVIDER_ID {
        coord.preload_foundry_local_asr_in_background("active_asr_provider_switch");
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

include!("style_pack_commands.rs");

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

include!("local_asr_commands.rs");

include!("diagnostics_export.rs");

include!("marketplace.rs");

#[tauri::command]
pub fn get_embedded_ble_runtime_status(coord: CoordinatorState<'_>) -> EmbeddedBleRuntimeStatus {
    device::get_embedded_ble_runtime_status(coord)
}

fn get_dictation_runtime_snapshot_request_error(request_id: u32) -> Option<String> {
    (request_id == 0).then(|| "requestId must be non-zero".to_string())
}

#[tauri::command]
pub fn get_dictation_runtime_snapshot(
    coord: CoordinatorState<'_>,
    request_id: u32,
) -> Result<crate::coordinator::DictationRuntimeSnapshot, String> {
    if let Some(error) = get_dictation_runtime_snapshot_request_error(request_id) {
        return Err(error);
    }
    Ok(coord.dictation_runtime_snapshot(request_id))
}

#[tauri::command]
pub async fn get_device_settings(
    coord: CoordinatorState<'_>,
    app: AppHandle,
) -> Result<DeviceSettingsSnapshot, String> {
    device::get_device_settings(coord, app).await
}

#[tauri::command]
pub async fn set_device_settings(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    request: DeviceSettingsUpdateRequest,
) -> Result<DeviceSettingsSnapshot, String> {
    device::set_device_settings(coord, app, request).await
}

#[tauri::command]
pub async fn get_firmware_ota_preflight_snapshot(
    coord: CoordinatorState<'_>,
    _protocol_name: Option<String>,
) -> Result<FirmwareOtaPreflightSnapshot, String> {
    device::get_firmware_ota_preflight_snapshot(coord, _protocol_name).await
}

#[tauri::command]
pub fn load_firmware_ota_package(path: String) -> Result<FirmwareOtaPackagePayload, String> {
    device::load_firmware_ota_package(path)
}

#[tauri::command]
pub fn list_wired_firmware_ports() -> Result<Vec<WiredFirmwareSerialPort>, String> {
    device::list_wired_firmware_ports()
}

#[tauri::command]
pub fn load_wired_firmware_package(path: String) -> Result<WiredFirmwarePackagePayload, String> {
    device::load_wired_firmware_package(path)
}

#[tauri::command]
pub async fn flash_wired_firmware_package(
    app: AppHandle,
    path: String,
    port: Option<String>,
    baud: Option<u32>,
    preserve_ota_data: Option<bool>,
) -> Result<WiredFirmwareFlashResult, String> {
    device::flash_wired_firmware_package(app, path, port, baud, preserve_ota_data).await
}

#[tauri::command]
pub async fn repair_wired_firmware_bootloader(
    app: AppHandle,
    path: String,
    port: Option<String>,
    baud: Option<u32>,
) -> Result<WiredFirmwareFlashResult, String> {
    device::repair_wired_firmware_bootloader(app, path, port, baud).await
}

#[tauri::command]
pub async fn transfer_firmware_ota_ble(
    app: AppHandle,
    coord: CoordinatorState<'_>,
    manifest: Value,
    firmware_bytes: Vec<u8>,
    expected_sha256: String,
) -> Result<FirmwareOtaBleTransferResult, String> {
    device::transfer_firmware_ota_ble(app, coord, manifest, firmware_bytes, expected_sha256).await
}

#[tauri::command]
pub async fn submit_embedded_audio_ble_stream(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    device::submit_embedded_audio_ble_stream(coord, timeout_ms).await
}

#[cfg(test)]
#[path = "../commands_tests.rs"]
mod tests;
