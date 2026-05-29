//! Tauri command surface — every IPC entry the React UI invokes lives here.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::asr::local::foundry::{
    model_alias_is_known, FoundryCatalogModel, FoundryPrepareProgressPayload, FoundryRuntimeStatus,
    DEFAULT_MODEL_ALIAS, PROVIDER_ID as FOUNDRY_LOCAL_PROVIDER_ID,
};
use crate::asr::local::FoundryLocalRuntime;
use crate::coordinator::{
    Coordinator, EmbeddedBleWakeRecoverySnapshot, FirmwareWakePolicySnapshot,
};
use crate::coordinator_state::SessionPhase;
use crate::permissions::{self, PermissionStatus};
use crate::persistence::{
    sync_style_pack_preferences, CredentialAccount, CredentialsSnapshot, CredentialsVault,
    PreferencesStore,
};
use crate::polish::{
    http_client_builder_with_proxy, CodexOAuthConfig, CodexOAuthCredentials, CodexOAuthLLMProvider,
    LLMError, OpenAICompatibleConfig, OpenAICompatibleLLMProvider, ProviderProxyConfig,
    CODEX_DEFAULT_MODEL, CODEX_OAUTH_PROVIDER_ID,
};
use crate::recorder::{AudioConsumer, Recorder};
use crate::types::{
    builtin_style_pack_id, default_active_style_pack_id, ChineseScriptPreference, ComboBinding,
    CorrectionRule, CredentialsStatus, DictationSession, DictionaryEntry, HotkeyCapability,
    HotkeyStatus, OutputLanguagePreference, PolishMode, ShortcutBinding, StylePack, StylePackKind,
    StylePackRuntimeDiagnostics, StyleSystemPrompts, UpdateChannel, UserPreferences,
    VocabPresetStore, WindowsImeStatus,
};

type CoordinatorState<'a> = State<'a, Arc<Coordinator>>;
pub type MicrophoneMonitorState = Mutex<Option<Recorder>>;
pub type TrayMicrophoneMenuState = Mutex<Vec<TrayMicrophoneMenuItem>>;

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

trait SettingsWriter {
    fn write_settings(&self, prefs: UserPreferences) -> Result<(), String>;
    fn refresh_dictation_hotkey(&self);
    fn refresh_qa_hotkey(&self);
    fn refresh_combo_hotkey(&self);
    fn refresh_translation_hotkey(&self);
    fn refresh_switch_style_hotkey(&self);
    fn refresh_open_app_hotkey(&self);
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
}

fn persist_settings<T: SettingsWriter>(
    coord: &T,
    mut prefs: UserPreferences,
) -> Result<(), String> {
    sync_dictation_hotkey_legacy_fields(&mut prefs);
    reject_hotkey_collisions(&prefs)?;
    coord.write_settings(prefs)?;
    coord.refresh_dictation_hotkey();
    coord.refresh_qa_hotkey();
    coord.refresh_combo_hotkey();
    coord.refresh_translation_hotkey();
    coord.refresh_switch_style_hotkey();
    coord.refresh_open_app_hotkey();
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
    // 广播给所有 webview。issue #205：QaPanel 跑在独立 webview，
    // 没有 HotkeySettingsContext，必须靠事件感知录音键变化，否则面板可见时
    // 用户改键会让浮窗里的 "{recordHotkey}" 文案一直停留在旧值。
    persist_settings(&*coord, prefs.clone())?;
    coord.refresh_embedded_ble_listener();
    // refresh_tray_microphone_menu 内部会调用 NSStatusItem.set_menu，必须在主线程上跑。
    // set_settings 本身是同步 Tauri command，在 IPC handler 线程上执行；从这里直接调
    // 会触发 macOS 主线程断言或在 dispatch 队列上死锁，导致整个 UI 无响应（用户改
    // 偏好后所有按键都没反应即此根因）。dispatch 到主线程后立即返回，IPC 线程不阻塞。
    let app_for_main = app.clone();
    let prefs_for_main = prefs.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(err) = crate::refresh_tray_microphone_menu(&app_for_main) {
            log::warn!("[tray] refresh microphone menu after settings save failed: {err}");
            let tray_state = app_for_main.state::<TrayMicrophoneMenuState>();
            sync_tray_microphone_selection(
                &tray_state.lock(),
                &prefs_for_main.microphone_device_name,
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

// ─────────────────────────── release channel (Beta opt-in) ───────────────────────────
//
// 渠道偏好的写入路径跟 set_settings 复用 persist_settings：保持热键兜底归一化
// 跟其他 prefs 写入一致，且写完后 emit "prefs:changed"，让前端跨 webview 同步。
//
// 注意：plugin-updater 2.10 的 Builder 不暴露 endpoints() 运行时 API，因此切到 Beta
// 渠道**不会**改变 in-app「检查更新」的行为——它仍然只看正式版 manifest。Beta 用户
// 通过 `fetch_latest_beta_release` 获取最新 prerelease，由前端跳浏览器手动下载，
// 物理隔离 Beta 包不会通过 auto-update 推到正式版用户。详见 PR-B-2 description 与
// CLAUDE.md `Branch & release-channel workflow` 段落。

#[tauri::command]
pub fn get_update_channel(coord: CoordinatorState<'_>) -> UpdateChannel {
    coord.prefs().get().update_channel
}

#[tauri::command]
pub fn set_update_channel(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    channel: UpdateChannel,
) -> Result<(), String> {
    let mut prefs = coord.prefs().get();
    if prefs.update_channel == channel {
        return Ok(());
    }
    prefs.update_channel = channel;
    persist_settings(&*coord, prefs.clone())?;
    let _ = app.emit("prefs:changed", &prefs);
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestBetaRelease {
    pub tag_name: String,
    pub html_url: String,
    pub published_at: String,
}

/// 拉 GitHub Releases atom feed 找最新 Beta release（tag 以 `-beta-tauri` 结尾）。
///
/// 历史：之前用 `api.github.com/repos/.../releases` REST 端点，**未认证 60 req/h/IP**，
/// 多人多次切 Beta toggle 很容易撞 403 rate limit（用户报"获取 Beta 版本信息失败"
/// 即是这个）。换成 `releases.atom` 后是公开页面 + CDN cache，没有同等 rate 限制。
/// Atom feed 不显式标 prerelease，但项目约定 tag 后缀 `-beta-tauri` 必为 Beta，
/// 所以只用 tag 后缀过滤就够了。
///
/// 返回 `Ok(None)` = 当前没发过 Beta 版；`Err(String)` = 网络/解析故障。
#[tauri::command]
pub async fn fetch_latest_beta_release() -> Result<Option<LatestBetaRelease>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("Listener Type/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .get("https://github.com/Listener-ai-Macau/Listener-Type/releases.atom")
        .send()
        .await
        .map_err(|e| format!("fetch releases.atom: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("releases.atom status {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("read atom body: {e}"))?;
    Ok(parse_latest_beta_from_atom(&body))
}

/// 简单字符串解析 atom feed，避免引 XML 库。每个 `<entry>...</entry>` 内含一行
/// `<link rel="alternate" type="text/html" href=".../releases/tag/<tag>"/>`，
/// 用 `/releases/tag/` 这个唯一锚点抓 tag。
fn parse_latest_beta_from_atom(body: &str) -> Option<LatestBetaRelease> {
    for entry in body.split("<entry>").skip(1) {
        let entry_body = entry
            .split_once("</entry>")
            .map(|(b, _)| b)
            .unwrap_or(entry);
        let needle = "/releases/tag/";
        let tag_start = match entry_body.find(needle) {
            Some(i) => i + needle.len(),
            None => continue,
        };
        let tag_after = &entry_body[tag_start..];
        let tag_end = tag_after
            .find(|c: char| c == '"' || c == '<' || c == ' ' || c == '/')
            .unwrap_or(tag_after.len());
        let tag_name = tag_after[..tag_end].to_string();
        if !tag_name.ends_with("-beta-tauri") {
            continue;
        }
        let html_url =
            format!("https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/{tag_name}");
        let published_at =
            extract_between(entry_body, "<updated>", "</updated>").unwrap_or_default();
        return Some(LatestBetaRelease {
            tag_name,
            html_url,
            published_at,
        });
    }
    None
}

fn extract_between(haystack: &str, open: &str, close: &str) -> Option<String> {
    let start = haystack.find(open)? + open.len();
    let end = haystack[start..].find(close)?;
    Some(haystack[start..start + end].to_string())
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
    fetch_provider_models(&config)
        .await
        .map(|models| ProviderModelsResult { models })
}

struct ProviderConfig {
    provider_id: String,
    base_url: String,
    api_key: String,
    proxy_config: ProviderProxyConfig,
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
    let proxy_config = read_provider_proxy_config(kind, &provider_id)?;
    Ok(ProviderConfig {
        provider_id,
        base_url,
        api_key,
        proxy_config,
    })
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
    pub message: String,
    pub failure: Option<crate::embedded_ble::BleFailureClassification>,
    pub runtime: EmbeddedBleRuntimeStatus,
    pub firmware: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
}

#[tauri::command]
pub async fn repair_embedded_ble_connection(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<EmbeddedBleRepairResult, String> {
    let repair = coord.repair_embedded_ble_connection(timeout_ms).await;
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
    let firmware =
        tauri::async_runtime::spawn_blocking(crate::embedded_ble::firmware_ota_device_snapshot)
            .await
            .map_err(|err| format!("Listener BLE repair snapshot task failed: {err}"))?;

    match repair {
        Ok(snapshot) => Ok(EmbeddedBleRepairResult {
            recovered: true,
            user_action_required: false,
            open_bluetooth_settings: false,
            message: snapshot.user_guidance,
            failure: None,
            runtime,
            firmware,
        }),
        Err(err) => {
            let failure = crate::embedded_ble::classify_ble_failure(&err);
            let user_action_required = !failure.automatic_recovery;
            let open_bluetooth_settings = user_action_required
                && matches!(
                    failure.kind,
                    crate::embedded_ble::BleFailureKind::DeviceMissing
                        | crate::embedded_ble::BleFailureKind::StaleGattService
                        | crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded
                        | crate::embedded_ble::BleFailureKind::AccessDenied
                );
            Ok(EmbeddedBleRepairResult {
                recovered: false,
                user_action_required,
                open_bluetooth_settings,
                message: failure.user_action.to_string(),
                failure: Some(failure),
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
    let version = manifest.version;
    let manifest_chunk_bytes = manifest.gatt_chunk_bytes as usize;
    let transfer_version = version.clone();
    let transfer_sha256 = expected_sha256.clone();
    let app_for_progress = app;
    let transfer = tauri::async_runtime::spawn_blocking(move || {
        crate::embedded_ble::transfer_firmware_ota(
            &transfer_version,
            &transfer_sha256,
            &firmware_bytes,
            manifest_chunk_bytes,
            Some(&|bytes_sent, bytes_total| {
                let _ = app_for_progress.emit(
                    "firmware-ota:progress",
                    serde_json::json!({
                        "bytesSent": bytes_sent,
                        "bytesTotal": bytes_total,
                    }),
                );
            }),
        )
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
    crate::shortcut_binding::validate_binding(&binding).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_dictation_hotkey(
    coord: CoordinatorState<'_>,
    binding: ShortcutBinding,
) -> Result<(), String> {
    crate::shortcut_binding::validate_binding(&binding).map_err(|e| e.to_string())?;
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
    crate::combo_hotkey::validate_binding(&shortcut).map_err(|e| e.to_string())
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
    if let Some(qa_hotkey) = prefs.qa_hotkey.as_ref() {
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
) -> Result<(), String> {
    let package = build_diagnostic_package(coord.inner())?;
    let bytes = serde_json::to_vec_pretty(&package).map_err(|e| format!("生成诊断包失败：{e}"))?;
    let target = std::path::Path::new(&target_path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败：{e}"))?;
        }
    }
    std::fs::write(target, bytes).map_err(|e| format!("写入诊断包失败：{e}"))
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
                "WAV/audio recording files are not exported.",
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
// dev-mode auth：用户在 Settings 填 `marketplace_dev_login`（GitHub 风格 username），
// 后续 OAuth 接入时换成 token 字段。
//
// 5 个 IPC：
// - marketplace_list      列表 + 搜索 + 排序
// - marketplace_detail    详情（含完整 prompt）
// - marketplace_install   下载 ZIP + 直接调 import_from_zip 装到本地
// - marketplace_upload    把本地某个 style pack export ZIP → multipart 上传
// - marketplace_like      点赞

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceListItem {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub author_login: String,
    pub version: String,
    pub base_mode: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub like_count: i64,
    pub download_count: i64,
    pub published_at: String,
    pub updated_at: String,
    pub origin_pack_id: Option<String>,
    pub origin_author_login: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceDetail {
    #[serde(flatten)]
    pub summary: MarketplaceListItem,
    pub prompt: String,
    pub state: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceMyPackItem {
    #[serde(flatten)]
    pub summary: MarketplaceListItem,
    pub state: String,
}

/// Listener Type does not inherit any upstream-owned production marketplace.
///
/// Remote marketplace calls are disabled by default. A future Listener Type
/// backend can be enabled by setting `LISTENER_TYPE_MARKETPLACE_BASE_URL` or
/// by writing `prefs.marketplace_base_url` through a controlled config surface.
const MARKETPLACE_BACKEND_DISABLED: &str =
    "Listener Type marketplace backend is not configured; local style packs remain available.";

fn marketplace_url_from_prefs(prefs: &UserPreferences) -> Result<String, String> {
    let env_url = std::env::var("LISTENER_TYPE_MARKETPLACE_BASE_URL").unwrap_or_default();
    let configured = if env_url.trim().is_empty() {
        prefs.marketplace_base_url.trim()
    } else {
        env_url.trim()
    };
    if configured.is_empty() {
        return Err(MARKETPLACE_BACKEND_DISABLED.to_string());
    }
    let parsed =
        reqwest::Url::parse(configured).map_err(|e| format!("invalid marketplace url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("marketplace url must use http or https".into());
    }
    Ok(configured.trim_end_matches('/').to_string())
}

fn marketplace_dev_user(prefs: &UserPreferences) -> String {
    prefs.marketplace_dev_login.trim().to_string()
}

#[tauri::command]
pub async fn marketplace_list(
    coord: CoordinatorState<'_>,
    query: Option<String>,
    sort: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<MarketplaceListItem>, String> {
    let prefs = coord.prefs().get();
    let base = match marketplace_url_from_prefs(&prefs) {
        Ok(base) => base,
        Err(_) => return Ok(Vec::new()),
    };
    let mut url = reqwest::Url::parse(&format!("{base}/packs"))
        .map_err(|e| format!("invalid marketplace url: {e}"))?;
    if let Some(q) = query.as_deref() {
        if !q.trim().is_empty() {
            url.query_pairs_mut().append_pair("q", q.trim());
        }
    }
    if let Some(s) = sort.as_deref() {
        if !s.trim().is_empty() {
            url.query_pairs_mut().append_pair("sort", s.trim());
        }
    }
    if let Some(n) = limit {
        url.query_pairs_mut().append_pair("limit", &n.to_string());
    }
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("marketplace request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("marketplace HTTP {status}: {body}"));
    }
    let items: Vec<MarketplaceListItem> = resp
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;
    Ok(items)
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
    let base = marketplace_url_from_prefs(&prefs)?;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/packs/{pack_id}"))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("marketplace request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("marketplace HTTP {status}"));
    }
    resp.json::<MarketplaceDetail>()
        .await
        .map_err(|e| format!("parse failed: {e}"))
}

#[tauri::command]
pub async fn marketplace_install(
    coord: CoordinatorState<'_>,
    pack_id: String,
) -> Result<StylePack, String> {
    // 安全校验：pack_id 来自远端 backend，可能含路径遍历 segment。
    // 用跟 read_audio_recording 同样的 UUID-v4 白名单挡住 ../ / 绝对路径等。
    // backend 当前用 Uuid::new_v4 生成所有 id，合法 id 必然匹配。
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let base = marketplace_url_from_prefs(&prefs)?;
    let client = reqwest::Client::new();

    // 先拉 detail 拿 authorLogin —— 装好后本地写 originAuthorLogin，
    // 后续编辑+发布时 backend 据此判 supersede（原作者）vs derivative（他人 fork）。
    let detail_url = format!("{base}/packs/{pack_id}");
    let detail: serde_json::Value = client
        .get(&detail_url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("marketplace detail failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("marketplace detail HTTP error: {e}"))?
        .json()
        .await
        .map_err(|e| format!("parse detail failed: {e}"))?;
    let origin_author_login = detail
        .get("authorLogin")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let bytes = client
        .get(format!("{base}/packs/{pack_id}/download"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("marketplace download failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("marketplace HTTP error: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("read body failed: {e}"))?;

    // pack_id 已经过 UUID 白名单，拼临时文件路径安全。
    let tmp = std::env::temp_dir().join(format!("listener-type-marketplace-{pack_id}.zip"));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("write tmp zip: {e}"))?;
    let imported_result = coord
        .style_packs()
        .import_from_zip(&tmp)
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    let imported = imported_result?;

    // 绑定 origin —— 后续编辑+发布走 derivative / supersede 分支。
    coord
        .style_packs()
        .set_origin(&imported.id, Some(pack_id), origin_author_login)
        .map_err(|e| format!("set origin failed: {e}"))
}

#[tauri::command]
pub async fn marketplace_upload(
    coord: CoordinatorState<'_>,
    pack_id: String,
    origin_pack_id: Option<String>,
) -> Result<serde_json::Value, String> {
    // 本地 pack id 形态：`builtin.light` / 用户 slug / Uuid。用 local 白名单挡 `..` / `/` / `\`。
    if !is_valid_local_pack_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let base = marketplace_url_from_prefs(&prefs)?;
    let dev_user = marketplace_dev_user(&prefs);
    if dev_user.is_empty() {
        return Err("未登录：先在 Settings 填发布者名字".into());
    }

    // 拉本地 pack 拿 origin_pack_id —— 装过的 pack 这里有值，
    // backend 据此判同作者就 supersede 原行（新版本），他人就 derivative（独立新 row）。
    let local_pack = coord
        .style_packs()
        .get(&pack_id)
        .map_err(|e| format!("local pack not found: {e}"))?;
    let origin_pack_id = origin_pack_id
        .filter(|id| is_valid_session_id(id))
        .or_else(|| local_pack.origin_pack_id.clone());

    // 先 export 本地 pack → 临时 ZIP
    let tmp = std::env::temp_dir().join(format!("listener-type-marketplace-upload-{pack_id}.zip"));
    coord
        .style_packs()
        .export_to_zip(&pack_id, &tmp)
        .map_err(|e| format!("export local pack failed: {e}"))?;
    let bytes = std::fs::read(&tmp).map_err(|e| format!("read exported zip: {e}"))?;
    let _ = std::fs::remove_file(&tmp);

    let client = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(format!("{pack_id}.zip"))
        .mime_str("application/zip")
        .map_err(|e| format!("multipart build failed: {e}"))?;
    let mut form = reqwest::multipart::Form::new().part("file", part);
    if let Some(ref oid) = origin_pack_id {
        form = form.text("origin_pack_id", oid.clone());
    }
    let resp = client
        .post(format!("{base}/packs"))
        .header("X-Dev-User", dev_user)
        .timeout(std::time::Duration::from_secs(30))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("upload request failed: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .unwrap_or_else(|e| format!("read body failed: {e}"))
        .clone();
    if !status.is_success() {
        return Err(format!("upload HTTP {status}: {body}"));
    }
    let parsed = serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("parse upload response failed: {e}"))?;

    // 本地从未绑定 origin（首次上传一个本地原创 pack）→ 把 backend 分配的 pack id 写回本地，
    // 让用户在同设备上后续编辑能继续走「同作者 supersede」分支，更新自己原创的包。
    if origin_pack_id.is_none() {
        if let Some(remote_id) = parsed.get("id").and_then(|v| v.as_str()) {
            let prefs2 = coord.prefs().get();
            let dev_user2 = marketplace_dev_user(&prefs2);
            let _ = coord.style_packs().set_origin(
                &pack_id,
                Some(remote_id.to_string()),
                Some(dev_user2),
            );
        }
    }

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
    let base = marketplace_url_from_prefs(&prefs)?;
    let dev_user = marketplace_dev_user(&prefs);
    if dev_user.is_empty() {
        return Err("未登录：先在 Settings 填发布者名字".into());
    }
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/packs/{pack_id}/like"))
        .header("X-Dev-User", dev_user)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("like request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("like HTTP {}", resp.status()));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))
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
    let base = marketplace_url_from_prefs(&prefs)?;
    let dev_user = marketplace_dev_user(&prefs);
    if dev_user.is_empty() {
        return Err("未登录：先在 Settings 填发布者名字".into());
    }
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!("{base}/packs/{pack_id}"))
        .header("X-Dev-User", dev_user)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("delete request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("delete HTTP {status}: {body}"));
    }
    Ok(())
}

/// 拉当前用户赞过的所有 pack id，用于客户端市场页面渲染红心 + 「我赞过的」过滤。
#[tauri::command]
pub async fn marketplace_my_likes(coord: CoordinatorState<'_>) -> Result<Vec<String>, String> {
    let prefs = coord.prefs().get();
    let base = match marketplace_url_from_prefs(&prefs) {
        Ok(base) => base,
        Err(_) => return Ok(Vec::new()),
    };
    let dev_user = marketplace_dev_user(&prefs);
    if dev_user.is_empty() {
        return Ok(Vec::new()); // 未登录就空集合，UI 渲染无红心
    }
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/me/likes"))
        .header("X-Dev-User", dev_user)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("my-likes request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("my-likes HTTP {}", resp.status()));
    }
    resp.json::<Vec<String>>()
        .await
        .map_err(|e| format!("parse my-likes failed: {e}"))
}

/// 拉当前用户发布过的 pack（含审核中/已通过/已拒绝/已撤回），用于「我的发布」页面。
#[tauri::command]
pub async fn marketplace_my_packs(
    coord: CoordinatorState<'_>,
) -> Result<Vec<MarketplaceMyPackItem>, String> {
    let prefs = coord.prefs().get();
    let base = match marketplace_url_from_prefs(&prefs) {
        Ok(base) => base,
        Err(_) => return Ok(Vec::new()),
    };
    let dev_user = marketplace_dev_user(&prefs);
    if dev_user.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/me/packs"))
        .header("X-Dev-User", dev_user)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("my-packs request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("my-packs HTTP {}", resp.status()));
    }
    resp.json::<Vec<MarketplaceMyPackItem>>()
        .await
        .map_err(|e| format!("parse my-packs failed: {e}"))
}

// ─────────────────────── GitHub OAuth Device Flow (Phase 1) ───────────────────────
//
// 客户端直连 GitHub 拿 access_token + login，前端自动把 login 写进
// prefs.marketplaceDevLogin。Listener Type 后端未上线前，默认不内置 OAuth App，
// 因此该能力必须由环境变量或未来配置显式开启。
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

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubDeviceStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u32,
    pub expires_in: u32,
}

#[tauri::command]
pub async fn github_device_flow_start() -> Result<GithubDeviceStartResponse, String> {
    let client_id = get_github_oauth_client_id()?;
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", "Listener Type")
        .form(&[("client_id", client_id.as_str()), ("scope", "read:user")])
        .send()
        .await
        .map_err(|e| format!("调用 GitHub /login/device/code 失败：{e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 device/code 响应失败：{e}"))?;
    if !status.is_success() {
        let err = body["error"].as_str().unwrap_or("unknown_error");
        let desc = body["error_description"].as_str().unwrap_or("");
        return Err(format!("GitHub device/code {status} {err}: {desc}"));
    }
    Ok(GithubDeviceStartResponse {
        device_code: body["device_code"].as_str().unwrap_or("").to_string(),
        user_code: body["user_code"].as_str().unwrap_or("").to_string(),
        verification_uri: body["verification_uri"]
            .as_str()
            .unwrap_or("https://github.com/login/device")
            .to_string(),
        interval: body["interval"].as_u64().unwrap_or(5) as u32,
        expires_in: body["expires_in"].as_u64().unwrap_or(900) as u32,
    })
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
    let client = reqwest::Client::new();
    let token_resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .header("User-Agent", "Listener Type")
        .form(&[
            ("client_id", client_id.as_str()),
            ("device_code", device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("调用 GitHub /login/oauth/access_token 失败：{e}"))?;
    let body: serde_json::Value = token_resp
        .json()
        .await
        .map_err(|e| format!("解析 access_token 响应失败：{e}"))?;

    if let Some(token) = body["access_token"].as_str() {
        let user_resp = client
            .get("https://api.github.com/user")
            .header("User-Agent", "Listener Type")
            .header("Accept", "application/vnd.github+json")
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("调用 GitHub /user 失败：{e}"))?;
        let user_body: serde_json::Value = user_resp
            .json()
            .await
            .map_err(|e| format!("解析 /user 响应失败：{e}"))?;
        let login = user_body["login"].as_str().unwrap_or("").to_string();
        if login.is_empty() {
            return Ok(GithubDevicePollResult::Error {
                message: "GitHub /user 返回空 login".to_string(),
            });
        }
        return Ok(GithubDevicePollResult::Authorized { login });
    }

    let err = body["error"].as_str().unwrap_or("");
    let msg = match err {
        "authorization_pending" => return Ok(GithubDevicePollResult::Pending),
        "slow_down" => return Ok(GithubDevicePollResult::SlowDown),
        "expired_token" => "OAuth 设备码已过期，请重新发起登录".to_string(),
        "access_denied" => "你在 GitHub 上拒绝了授权".to_string(),
        other if !other.is_empty() => format!("OAuth 错误：{other}"),
        _ => "未知 OAuth 错误（access_token 缺失）".to_string(),
    };
    Ok(GithubDevicePollResult::Error { message: msg })
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use super::release_foundry_runtime_if_inactive;
    use super::{
        active_asr_is_keyless_for_validation, active_foundry_model_from_prefs,
        asr_configured_for_provider, asr_transcriptions_url, diagnostic_recent_errors,
        fetch_provider_models, firmware_ota_snapshot_version, firmware_ota_versions_match,
        is_diagnostic_error_line, is_gemini_base_url, is_valid_local_pack_id, is_valid_session_id,
        llm_configured_for_provider, load_firmware_ota_package,
        local_asr_release_plan_for_provider, models_url, normalize_foundry_language_hint,
        parse_gemini_model_ids, parse_latest_beta_from_atom, parse_model_ids, persist_settings,
        sanitize_diagnostic_log_line, validate_foundry_model_alias, ProviderConfig, SettingsWriter,
    };
    use crate::coordinator::Coordinator;
    use crate::embedded_audio::{SessionEndReason, SessionErrorCode, SessionStats};
    use crate::embedded_ble::FirmwareOtaDeviceSnapshot;
    use crate::persistence::CredentialsSnapshot;
    use crate::polish::ProviderProxyConfig;
    use crate::types::{
        ComboBinding, DictationSession, HotkeyBinding, HotkeyMode, HotkeyTrigger, InsertStatus,
        PolishMode, ShortcutBinding, UserPreferences,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

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

    #[derive(Default)]
    struct FakeSettingsWriter {
        saved: Mutex<Option<UserPreferences>>,
        dictation_refreshes: Mutex<u32>,
        qa_refreshes: Mutex<u32>,
        combo_refreshes: Mutex<u32>,
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
        assert!(value["ble"]["failureTaxonomy"].is_array());
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

    #[test]
    fn diagnostic_ble_failure_taxonomy_deduplicates_sources() {
        let snapshot = crate::embedded_ble::BleDiagnosticSnapshot {
            captured_at: "2026-05-28T00:00:00Z".to_string(),
            platform: "windows",
            audio_service_uuid: "audio",
            ota_service_uuid: "ota",
            dis_service_uuid: "dis",
            configured_device_address: Some("14C19F48FE72".to_string()),
            audio_services: Vec::new(),
            ota_services: Vec::new(),
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
    fn parse_latest_beta_from_atom_picks_first_beta_tagged_entry() {
        // Fixture trimmed from real `releases.atom`：包含一条 stable + 一条 Beta。
        // 解析必须跳过 stable（tag 不以 -beta-tauri 结尾），返回 Beta。
        let body = r#"<?xml version="1.0"?>
<feed>
  <entry>
    <id>tag:github.com,2008:Repository/X/v1.2.23-tauri</id>
    <updated>2026-05-07T09:05:00Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.2.23-tauri"/>
    <title>Listener Type v1.2.23-tauri</title>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/X/v1.2.24-2-beta-tauri</id>
    <updated>2026-05-08T01:27:23Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.2.24-2-beta-tauri"/>
    <title>Listener Type v1.2.24-2-beta-tauri</title>
  </entry>
</feed>"#;
        let got = parse_latest_beta_from_atom(body).expect("must find a Beta entry");
        assert_eq!(got.tag_name, "v1.2.24-2-beta-tauri");
        assert_eq!(
            got.html_url,
            "https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.2.24-2-beta-tauri"
        );
        assert_eq!(got.published_at, "2026-05-08T01:27:23Z");
    }

    #[test]
    fn parse_latest_beta_from_atom_returns_none_when_only_stable_releases() {
        let body = r#"<feed>
  <entry>
    <link rel="alternate" type="text/html" href="https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.2.23-tauri"/>
    <updated>2026-05-07T09:05:00Z</updated>
  </entry>
</feed>"#;
        assert!(parse_latest_beta_from_atom(body).is_none());
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
