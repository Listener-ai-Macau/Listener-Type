#![allow(dead_code)]

//! Listener Type Tauri backend.
//!
//! Modules mirror the original Swift libraries (one purpose per file):
//! - hotkey: global hotkey monitor
//! - recorder: microphone capture (16 kHz mono Int16 PCM)
//! - asr: streaming ASR providers (Volcengine SAUC bigmodel)
//! - polish: OpenAI-compatible chat completions client
//! - insertion: cursor-position text insertion (AX / paste)
//! - persistence: history + preferences + credentials vault
//! - coordinator: dictation state machine glue
//! - commands: Tauri IPC surface

mod asr;
mod audio_mute;
mod audio_transport_codec;
mod capsule_log;
mod cli;
mod combo_hotkey;
mod commands;
mod coordinator;
mod coordinator_state;
mod correction;
pub(crate) use denzic_audio_v1_core as embedded_audio;
pub(crate) use denzic_observability_v1_core as observability_v1;
mod device_control_platform;
mod embedded_ble;
mod firmware_ota;
mod github_oauth;
mod global_hotkey_runtime;
mod hotkey;
mod insertion;
mod llm_gemini;
mod marketplace_backend;
mod observability;
mod permissions;
mod persistence;
mod polish;
mod qa_hotkey;
mod recorder;
mod selection;
mod shortcut_binding;
mod shortcut_dispatch;
mod speaker_verification;
mod startup_evidence;
mod timeline;
mod types;
mod unicode_keystroke;
mod wake_phrase;
mod windows_ime_ipc;
mod windows_ime_profile;
mod windows_ime_protocol;
mod windows_ime_session;

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const LOG_ROTATE_LIMIT_BYTES: u64 = 10 * 1024 * 1024;
const LOG_SEGMENT_LIMIT_BYTES: u64 = LOG_ROTATE_LIMIT_BYTES / 2;
const LOG_MAX_RECORD_BYTES: usize = 64 * 1024;
const SUPPRESS_CAPSULE_WINDOW_ENV: &str = "LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW";
const FORCE_RAW_OUTPUT_ENV: &str = "LISTENER_TYPE_FORCE_RAW_OUTPUT";
#[cfg(target_os = "windows")]
const LISTENER_TYPE_WEBVIEW2_ADDITIONAL_BROWSER_ARGS_ENV: &str =
    "LISTENER_TYPE_WEBVIEW2_ADDITIONAL_BROWSER_ARGS";
#[cfg(target_os = "windows")]
const WRY_DEFAULT_DISABLED_WEBVIEW2_FEATURES: &str = "msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// 第一次 show 时把 QA 浮窗摆到屏幕底部居中；之后的 show 不再 reposition，
/// 让用户拖动后的位置在 hide → show 之间得以保持。详见 issue #118 v2。
static QA_WINDOW_POSITIONED: AtomicBool = AtomicBool::new(false);
static APP_QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static TRAY_MICROPHONE_WATCHER_STOPPING: AtomicBool = AtomicBool::new(false);
use tauri::menu::{
    CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, Submenu, SubmenuBuilder,
};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, RunEvent, Runtime,
    WebviewWindow,
};

use crate::types::{DictationInputSource, PolishMode};

#[cfg(target_os = "windows")]
fn merge_webview2_test_browser_args(existing: Option<&str>, requested: &str) -> Option<String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return existing
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
    }

    let mut merged = match existing.map(str::trim).filter(|value| !value.is_empty()) {
        Some(existing) => format!("{existing} {requested}"),
        None => requested.to_string(),
    };
    if !merged.contains("--disable-features=") {
        merged = format!(
            "--disable-features={} {}",
            WRY_DEFAULT_DISABLED_WEBVIEW2_FEATURES, merged
        );
    }
    Some(merged)
}

#[cfg(target_os = "windows")]
fn apply_webview2_test_browser_args_from_env<R: Runtime>(context: &mut tauri::Context<R>) {
    let Some(requested) = std::env::var(LISTENER_TYPE_WEBVIEW2_ADDITIONAL_BROWSER_ARGS_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    for window in &mut context.config_mut().app.windows {
        window.additional_browser_args =
            merge_webview2_test_browser_args(window.additional_browser_args.as_deref(), &requested);
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_webview2_test_browser_args_from_env<R: Runtime>(_context: &mut tauri::Context<R>) {}

fn exit_after_headless_cli(exit_code: i32) -> ! {
    // `process::exit` skips logger drop, so persist the result line before returning to the caller.
    log::logger().flush();
    std::process::exit(exit_code);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    startup_evidence::begin_process_evidence();
    let first_run_args: Vec<String> = std::env::args().collect();
    if let Some(intent) = cli::parse_cli_intent(&first_run_args) {
        match intent {
            cli::CliIntent::LocalWakeHelper => {
                exit_after_headless_cli(asr::local::wake_helper::run_helper());
            }
            cli::CliIntent::SubmitEmbeddedAudioBleOnce { .. }
            | cli::CliIntent::SubmitEmbeddedAudioBleStream { .. }
            | cli::CliIntent::SendEmbeddedAudioControlStop { .. }
            | cli::CliIntent::ReadEmbeddedAudioBleStatus { .. }
            | cli::CliIntent::ProbeListenerOtaV1Gatt { .. }
            | cli::CliIntent::ProbeListenerOtaV1ActiveHandoff { .. }
            | cli::CliIntent::PromptEmbeddedBlePairing { .. }
            | cli::CliIntent::PromptEmbeddedBlePairingOnly { .. }
            | cli::CliIntent::CleanupEmbeddedBlePairing { .. } => {
                exit_after_headless_cli(run_embedded_ble_headless_cli(intent));
            }
            cli::CliIntent::ProbeEmbeddedAudioBleSubscription { .. }
                if std::env::var_os("LISTENER_TYPE_FORCE_HEADLESS_BLE_CLI").is_some() =>
            {
                exit_after_headless_cli(run_embedded_ble_headless_cli(intent));
            }
            cli::CliIntent::FirmwareOta { .. } => {
                exit_after_headless_cli(run_firmware_ota_headless_cli(intent));
            }
            cli::CliIntent::WiredFirmware { .. } => {
                exit_after_headless_cli(run_wired_firmware_headless_cli(intent));
            }
            _ => {}
        }
    }

    #[cfg(target_os = "windows")]
    embedded_ble::warm_native_windows_hid_present_pairing_snapshot();

    let foundry_local_runtime = Arc::new(asr::local::FoundryLocalRuntime::new());
    #[cfg(target_os = "windows")]
    let coordinator = Arc::new(coordinator::Coordinator::new_with_foundry_runtime(
        Arc::clone(&foundry_local_runtime),
    ));
    #[cfg(not(target_os = "windows"))]
    let coordinator = Arc::new(coordinator::Coordinator::new());
    let local_asr_download_manager = Arc::new(asr::local::DownloadManager::new());
    let mut tauri_context = tauri::generate_context!();
    apply_webview2_test_browser_args_from_env(&mut tauri_context);

    tauri::Builder::default()
        // 单实例锁：第二个进程启动时立即退出，激活信号转给已运行实例的主窗口。
        // 否则两份 Listener Type（如 /Applications/ + dev build）会各自抓全局热键，
        // 导致按一次键、两个进程同时跑流水线、文本被插入两遍。见 issue #50。
        //
        // 第二个进程的 argv 还有一个用处：作为 Linux/Wayland 下的「触发器入口」。
        // 桌面环境快捷键执行 `listener-type --toggle-dictation` 时，第二个进程被本插件
        // 拦截 → argv 直接转给主实例 coordinator。详见 issue #420 / `cli.rs`。
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // tauri-plugin-single-instance may pass only the forwarded arguments on
            // Windows, while `parse_cli_intent` accepts a normal argv vector and
            // skips argv[0]. Prefix a harmless dummy so both shapes parse the same.
            let mut forwarded_argv = Vec::with_capacity(argv.len() + 1);
            forwarded_argv.push("listener-type".to_string());
            forwarded_argv.extend(argv);
            if let Some(intent) = cli::parse_cli_intent(&forwarded_argv) {
                let dispatch_options = CliDispatchOptions {
                    suppress_capsule_window: cli::suppress_capsule_window_requested(
                        &forwarded_argv,
                    ),
                    force_raw_output: cli::force_raw_output_requested(&forwarded_argv),
                };
                log::info!(
                    "[single-instance] another instance launched with intent={intent:?}, dispatching"
                );
                dispatch_cli_intent(app, intent, dispatch_options);
                return;
            }
            log::info!(
                "[single-instance] another instance launched, focusing existing main window"
            );
            show_main_window(app);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        // 跨平台开机自启：mac 写 LaunchAgent plist，linux 写 ~/.config/autostart/*.desktop，
        // windows 写 HKCU\Software\Microsoft\Windows\CurrentVersion\Run。前端 toggle 直接
        // 调插件 isEnabled / enable / disable，不维持本地 prefs，让 OS 当唯一真相。issue #194。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(coordinator.clone())
        .manage(local_asr_download_manager.clone())
        .manage(foundry_local_runtime.clone())
        .manage(commands::MicrophoneMonitorState::new(None))
        .manage(commands::TrayMicrophoneMenuState::new(Vec::new()))
        .setup(move |app| {
            init_file_logger();
            log::info!("=== Listener Type 启动 ===");
            #[cfg(target_os = "windows")]
            if std::env::var_os(LISTENER_TYPE_WEBVIEW2_ADDITIONAL_BROWSER_ARGS_ENV).is_some() {
                log::info!(
                    "[automation] WebView2 additional browser args applied from {}",
                    LISTENER_TYPE_WEBVIEW2_ADDITIONAL_BROWSER_ARGS_ENV
                );
            }

            // Panic hook: catch Rust panics and emit to frontend so the user sees
            // an error card instead of a silent white-screen crash.
            {
                let handle = app.handle().clone();
                std::panic::set_hook(Box::new(move |info| {
                    let msg = format!("{info}");
                    log::error!("[panic] {msg}");
                    let payload = serde_json::json!({ "message": msg });
                    let _ = handle.emit("panic:error", payload);
                }));
            }

            // Capsule 启动时定位到屏幕底部居中并隐藏；coordinator 按需显示。
            // 与 Swift `CapsuleWindowController.repositionToBottomCenter` 同语义。
            if let Some(capsule) = app.get_webview_window("capsule") {
                prepare_capsule_window_for_overlay(&capsule);
                if let Err(e) = position_capsule_bottom_center(app.handle(), &capsule, false) {
                    log::warn!("[capsule] position failed: {e}");
                }
                let _ = capsule.hide();
            }

            // QA 浮窗（issue #118）：紧贴胶囊上方 8pt、屏幕底部居中、380×440。
            // 启动时 hide()，等 coordinator 在 open_qa_panel 时再 show + 首次定位。
            // tauri.conf.json 里需要声明 label="qa" 的窗口（前端 agent 负责）；
            // 这里 get_webview_window 返回 None 时直接跳过，不影响主流程。
            if let Some(qa) = app.get_webview_window("qa") {
                if let Err(e) = position_qa_window(&qa) {
                    log::warn!("[qa] position failed: {e}");
                }
                #[cfg(target_os = "macos")]
                make_qa_window_draggable_macos(&qa);
                let _ = qa.hide();
            } else {
                log::info!("[qa] qa 窗口未在 tauri.conf.json 中声明，前端 agent 会补上");
            }

            // 主窗口磨砂：macOS 用 NSVisualEffectView，Windows 用 Mica。
            // 没这一层的话 transparent: true 让窗口透明 → 背后只是空，不是磨砂。
            //
            // decorations:true 由 Tauri 配置统一保留：macOS 用系统红黄绿；
            // Windows 用原生 Win11 标题栏、拖动、圆角与 resize border。
            if let Some(main) = app.get_webview_window("main") {
                #[cfg(target_os = "macos")]
                {
                    use window_vibrancy::{
                        apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
                    };
                    if let Err(e) = main.set_decorations(true) {
                        log::warn!("[main] enable native decorations failed: {e}");
                    }
                    if let Err(e) = apply_vibrancy(
                        &main,
                        NSVisualEffectMaterial::HudWindow,
                        Some(NSVisualEffectState::Active),
                        Some(20.0),
                    ) {
                        log::warn!("[main] vibrancy failed: {e}");
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    use window_vibrancy::apply_mica;
                    // Windows 走 Tauri decorations:true 原生 Win11 标题栏 / 关闭按钮 /
                    // 拖动 / 圆角 / resize border。保留 apply_mica 给原生 chrome 提供
                    // 磨砂材质，配合 WindowChrome 半透明 background 让 sidebar 透出玻璃感。
                    if let Err(e) = apply_mica(&main, None) {
                        log::warn!("[main] mica failed: {e}");
                    }
                    // Win11 22H2+: 把原生标题栏底色调成白色，与应用 sidebar 视觉统一。
                    // 老版 Windows 静默失败，不阻塞。
                    apply_windows_caption_color_for_theme(&main, coordinator.prefs().get().dark_mode);
                }
                // 静默启动开关：prefs.start_minimized = true 或测试脚本设置
                // LISTENER_TYPE_HIDE_MAIN_ON_START=1 → 不弹主窗口，用户从菜单栏 /
                // 托盘点击访问。LISTENER_TYPE_SHOW_MAIN_ON_START=1 仍保留老的强制
                // show 路径（手动 dispatch 测试 / dev 用），优先级最高。
                let force_show = should_force_show_main_on_start();
                let hide_main_on_start = std::env::var("LISTENER_TYPE_HIDE_MAIN_ON_START")
                    .ok()
                    .as_deref()
                    == Some("1");
                let suppress_show = !force_show
                    && (hide_main_on_start || coordinator.prefs().get().start_minimized);
                if suppress_show {
                    log::info!(
                        "[main] start minimized/hidden requested → 跳过初始 show，等用户点托盘"
                    );
                } else if let Err(e) = main.show() {
                    log::warn!("[main] initial show failed: {e}");
                }
            }

            // 启动时主动弹 Accessibility 授权框（与 Swift `AppDelegate` 行为一致）。
            // 用户首次必看到系统提示；已授权则静默返回。
            #[cfg(target_os = "macos")]
            {
                let status = permissions::request_accessibility();
                log::info!("[startup] Accessibility status = {:?}", status);
            }

            // 菜单栏图标 — 与 Swift `MenuBarController` 同语义：
            // 左键点 → 显示/聚焦主窗口；右键菜单只保留日常切换项与退出。
            let tray_menu = build_tray_menu(app, &coordinator)?;
            let menu = tray_menu.menu;

            // 与 Swift `StatusBarIcon.swift` 行为一致：用全彩 AppIcon，**不**走 template 模式
            // （走 template 会被 macOS 染成单色 → 看起来像个黑方块）。
            if let Some(icon) = app.default_window_icon() {
                {
                    let state = app.state::<commands::TrayMicrophoneMenuState>();
                    *state.lock() = tray_menu.microphone_items;
                }
                let _tray = TrayIconBuilder::with_id("main-tray")
                    .icon(icon.clone())
                    .icon_as_template(false)
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(move |app, event| match event.id.as_ref() {
                        "quit" => request_app_quit(app),
                        "dark-mode" => handle_dark_mode_toggle(app),
                        id => {
                            if handle_style_tray_menu_event(app, id) {
                                return;
                            }
                            if handle_input_source_tray_menu_event(app, id) {
                                return;
                            }
                            handle_microphone_tray_menu_event(app, id);
                        }
                    })
                    .on_tray_icon_event(move |tray, event| match event {
                        TrayIconEvent::Enter { .. } => {
                            if let Err(err) = refresh_tray_microphone_menu(tray.app_handle()) {
                                log::warn!(
                                    "[tray] refresh microphone menu on hover failed: {err}"
                                );
                            }
                        }
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            ..
                        } => show_main_window(tray.app_handle()),
                        _ => {}
                    })
                    .build(app)?;
                start_tray_microphone_watcher(app.handle().clone());
            } else {
                log::warn!("[startup] default window icon missing; tray icon disabled");
            }

            let app_handle = app.handle().clone();
            coordinator.bind_app(app_handle);
            let wake_phrase = coordinator.prefs().get().voice_wake_phrase;
            std::thread::Builder::new()
                .name("voice-activation-preload".to_string())
                .spawn(move || {
                    let started = std::time::Instant::now();
                    // Always warm KWS for automatic wake. Voiceprint prepare only when enrolled.
                    let enrolled =
                        crate::speaker_verification::is_enrolled_for_phrase(&wake_phrase);
                    let speaker_phrase = wake_phrase.clone();
                    let speaker_prepare = enrolled.then(|| {
                        std::thread::spawn(move || {
                            crate::speaker_verification::prepare_for_phrase(&speaker_phrase)
                        })
                    });
                    let wake_result = crate::wake_phrase::prepare(&wake_phrase);
                    let speaker_result = match speaker_prepare {
                        Some(handle) => handle
                            .join()
                            .unwrap_or_else(|_| Err("声纹预热线程异常退出".to_string())),
                        None => Ok(()),
                    };
                    match (wake_result, speaker_result) {
                        (Ok(()), Ok(())) => log::info!(
                            "[wake-phrase] automatic wake prepared enrolled={} elapsed_ms={}",
                            enrolled,
                            started.elapsed().as_millis()
                        ),
                        (wake, speaker) => log::warn!(
                            "[wake-phrase] automatic wake prepare incomplete enrolled={enrolled} wake={wake:?} speaker={speaker:?}"
                        ),
                    }
                })
                .ok();
            // Spin up hotkey listener; coordinator owns the lifecycle.
            coordinator.start_hotkey_listener();
            coordinator.auto_select_embedded_ble_input_source_in_background();
            coordinator.preload_foundry_local_asr_in_background("startup");
            // QA / custom combo hotkeys use `global-hotkey` (Carbon on macOS).
            // Start those after RunEvent::Ready, when the AppKit event loop is live.
            if should_force_show_main_on_start() {
                log::info!("[main] force show requested during setup");
                show_main_window(app.handle());
            }

            // Wayland 下没有可用的全局键盘监听（issue #420）。Coordinator 已通过 stub adapter
            // 把 hotkey 状态标记为 Installed，整个应用照常起来。前端走 pull 模型：RecordingSection
            // mount 时调 `is_wayland_cli_mode` 取状态再渲染 CLI 引导 callout。原本用一次性 event 通知
            // 行不通——Settings 模态是按需 mount，事件不缓冲不 replay，listener 几乎必然错过。
            if hotkey::is_wayland_session() {
                log::info!("[startup] Wayland session — frontend will pull via is_wayland_cli_mode");
            }

            // 首次启动也可能带 CLI flag（用户双击 .desktop 之前先用 CLI 起一遍）。
            // 等 coordinator 准备好后再 dispatch；GUI 仍然照常起来。
            if let Some(intent) = cli::parse_cli_intent(&first_run_args) {
                let dispatch_options = CliDispatchOptions {
                    suppress_capsule_window: cli::suppress_capsule_window_requested(
                        &first_run_args,
                    ),
                    force_raw_output: cli::force_raw_output_requested(&first_run_args)
                        || std::env::var(FORCE_RAW_OUTPUT_ENV)
                            .map(|value| value == "1")
                            .unwrap_or(false),
                };
                log::info!("[startup] first-run CLI intent={intent:?}, dispatching");
                dispatch_cli_intent(app.handle(), intent, dispatch_options);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::get_voiceprint_status,
            commands::is_main_window_start_hidden,
            commands::get_default_style_system_prompts,
            commands::list_installed_applications,
            commands::record_ui_timeline_event,
            commands::set_settings,
            commands::start_voiceprint_enrollment,
            commands::delete_voiceprint,
            commands::refresh_device_settings_status,
            commands::get_hotkey_status,
            commands::get_hotkey_capability,
            commands::is_wayland_cli_mode,
            commands::set_shortcut_recording_active,
            commands::get_windows_ime_status,
            commands::list_microphone_devices,
            commands::start_microphone_level_monitor,
            commands::stop_microphone_level_monitor,
            commands::get_credentials,
            commands::set_credential,
            commands::list_history,
            commands::delete_history_entry,
            commands::clear_history,
            commands::read_audio_recording,
            commands::marketplace_list,
            commands::marketplace_detail,
            commands::marketplace_install,
            commands::marketplace_upload,
            commands::marketplace_like,
            commands::marketplace_my_likes,
            commands::marketplace_my_packs,
            commands::marketplace_delete,
            commands::github_device_flow_start,
            commands::github_device_flow_poll,
            commands::list_vocab,
            commands::add_vocab,
            commands::remove_vocab,
            commands::set_vocab_enabled,
            commands::list_correction_rules,
            commands::add_correction_rule,
            commands::remove_correction_rule,
            commands::set_correction_rule_enabled,
            commands::list_vocab_presets,
            commands::save_vocab_presets,
            commands::start_dictation,
            commands::stop_dictation,
            commands::submit_embedded_audio_notifications,
            commands::submit_embedded_audio_streaming_notifications,
            commands::submit_embedded_audio_file,
            commands::submit_embedded_audio_streaming_file,
            commands::submit_embedded_audio_ble_once,
            commands::probe_embedded_audio_ble_subscription,
            commands::repair_embedded_ble_connection,
            commands::recover_embedded_ble_device,
            commands::get_embedded_ble_runtime_status,
            commands::get_device_settings,
            commands::set_device_settings,
            commands::get_firmware_ota_preflight_snapshot,
            commands::load_firmware_ota_package,
            commands::list_wired_firmware_ports,
            commands::load_wired_firmware_package,
            commands::flash_wired_firmware_package,
            commands::repair_wired_firmware_bootloader,
            commands::transfer_firmware_ota_ble,
            commands::submit_embedded_audio_ble_stream,
            commands::cancel_dictation,
            commands::handle_window_hotkey_event,
            #[cfg(debug_assertions)]
            commands::inject_hotkey_click_for_dev,
            commands::repolish,
            commands::list_style_packs,
            commands::create_style_pack_from_template,
            commands::save_style_pack,
            commands::preview_style_pack_runtime,
            commands::set_active_style_pack,
            commands::set_style_pack_enabled,
            commands::reset_builtin_style_pack,
            commands::delete_style_pack,
            commands::import_style_pack_from_zip,
            commands::export_style_pack_to_zip,
            commands::set_default_polish_mode,
            commands::set_style_enabled,
            commands::check_accessibility_permission,
            commands::request_accessibility_permission,
            commands::check_microphone_permission,
            commands::request_microphone_permission,
            commands::open_system_settings,
            commands::trigger_microphone_prompt,
            commands::read_credential,
            commands::set_active_asr_provider,
            commands::set_active_llm_provider,
            commands::get_qa_hotkey_label,
            commands::set_qa_hotkey,
            commands::validate_shortcut_binding,
            commands::set_dictation_hotkey,
            commands::set_translation_hotkey,
            commands::set_switch_style_hotkey,
            commands::set_open_app_hotkey,
            commands::qa_window_dismiss,
            commands::qa_window_pin,
            commands::validate_combo_hotkey,
            commands::set_combo_hotkey,
            commands::validate_provider_credentials,
            commands::list_provider_models,
            commands::local_asr_get_settings,
            commands::local_asr_set_active_model,
            commands::local_asr_set_mirror,
            commands::local_asr_list_models,
            commands::local_asr_fetch_remote_info,
            commands::local_asr_download_model,
            commands::local_asr_cancel_download,
            commands::local_asr_delete_model,
            commands::local_asr_test_model,
            commands::local_asr_engine_status,
            commands::local_asr_release_engine,
            commands::local_asr_preload,
            commands::local_asr_set_keep_loaded_secs,
            commands::foundry_local_asr_status,
            commands::foundry_local_asr_catalog,
            commands::foundry_local_asr_set_model,
            commands::foundry_local_asr_set_language_hint,
            commands::foundry_local_asr_set_runtime_source,
            commands::foundry_local_asr_prepare,
            commands::foundry_local_asr_cancel_prepare,
            commands::foundry_local_asr_release,
            commands::export_error_log,
            commands::export_diagnostic_package,
            restart_app,
        ])
        .build(tauri_context)
        .expect("error while building tauri application")
        .run(|app, event| match event {
            RunEvent::Ready => {
                let coordinator = app.state::<Arc<coordinator::Coordinator>>();
                if should_force_show_main_on_start() {
                    log::info!("[main] force show requested after RunEvent::Ready");
                    show_main_window(app);
                }
                // 同步启动 QA hotkey listener。和 dictation hotkey 平行，互不抢状态。
                coordinator.start_qa_hotkey_listener();
                // 启动自定义组合键监听器。当 trigger == Custom 时替代 modifier-only 监听器。
                coordinator.start_combo_hotkey_listener();
                coordinator.start_translation_hotkey_listener();
                coordinator.start_switch_style_hotkey_listener();
                coordinator.start_open_app_hotkey_listener();
                coordinator.start_device_custom_key_hotkey_listeners();
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_main_window(app),
            RunEvent::ExitRequested { code, api, .. } => {
                if should_keep_alive_on_exit_request(
                    code,
                    APP_QUIT_REQUESTED.load(Ordering::Relaxed),
                ) {
                    log::warn!(
                        "[main] exit requested without explicit quit; keeping Listener Type alive"
                    );
                    api.prevent_exit();
                }
            }
            RunEvent::WindowEvent { label, event, .. } => {
                if label == "capsule"
                    && matches!(
                        &event,
                        tauri::WindowEvent::Moved(_)
                            | tauri::WindowEvent::ScaleFactorChanged { .. }
                    )
                {
                    let coordinator = app.state::<Arc<coordinator::Coordinator>>();
                    let force = matches!(
                        &event,
                        tauri::WindowEvent::ScaleFactorChanged { .. }
                    );
                    coordinator.reposition_capsule_after_display_change(app, force);
                }
                if label == "main" {
                    if let tauri::WindowEvent::CloseRequested { ref api, .. } = event {
                        if should_hide_main_on_close(APP_QUIT_REQUESTED.load(Ordering::Relaxed)) {
                            api.prevent_close();
                            hide_main_window(app);
                        }
                    }
                }
            }
            RunEvent::Exit => {
                log::info!("[main] exit");
                TRAY_MICROPHONE_WATCHER_STOPPING.store(true, Ordering::Relaxed);
                let coordinator = app.state::<Arc<coordinator::Coordinator>>();
                coordinator.request_shutdown();
                coordinator.stop_hotkey_listener();
                coordinator.stop_qa_hotkey_listener();
                coordinator.stop_combo_hotkey_listener();
                coordinator.stop_translation_hotkey_listener();
                coordinator.stop_switch_style_hotkey_listener();
                coordinator.stop_open_app_hotkey_listener();
                coordinator.stop_device_custom_key_hotkey_listeners();
            }
            _ => {}
        });
}

fn request_app_quit<R: Runtime>(app: &AppHandle<R>) {
    log::info!("[main] explicit quit requested");
    APP_QUIT_REQUESTED.store(true, Ordering::Relaxed);
    TRAY_MICROPHONE_WATCHER_STOPPING.store(true, Ordering::Relaxed);
    let coordinator = app.state::<Arc<coordinator::Coordinator>>();
    coordinator.request_shutdown();
    app.exit(0);
}

fn should_keep_alive_on_exit_request(code: Option<i32>, explicit_quit: bool) -> bool {
    code.is_none() && !explicit_quit
}

fn should_hide_main_on_close(explicit_quit: bool) -> bool {
    !explicit_quit
}

fn should_force_show_main_on_start() -> bool {
    std::env::var("LISTENER_TYPE_SHOW_MAIN_ON_START")
        .ok()
        .as_deref()
        == Some("1")
        || std::env::args().any(|arg| arg == "--show-main")
}

struct MicrophoneTrayMenu {
    submenu: Submenu<tauri::Wry>,
    items: Vec<commands::TrayMicrophoneMenuItem>,
}

struct StyleTrayMenu {
    submenu: Submenu<tauri::Wry>,
}

struct InputSourceTrayMenu {
    submenu: Submenu<tauri::Wry>,
}

struct TrayMenu {
    menu: Menu<tauri::Wry>,
    microphone_items: Vec<commands::TrayMicrophoneMenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayPolishModeMenuEntry {
    id: String,
    label: &'static str,
    mode: PolishMode,
    checked: bool,
}

fn tray_style_menu_enabled() -> bool {
    cfg!(target_os = "windows")
}

fn tray_polish_mode_menu_entries(selected: PolishMode) -> Vec<TrayPolishModeMenuEntry> {
    [
        (PolishMode::Raw, "style-raw"),
        (PolishMode::Light, "style-light"),
        (PolishMode::Structured, "style-structured"),
        (PolishMode::Formal, "style-formal"),
    ]
    .into_iter()
    .map(|(mode, id)| TrayPolishModeMenuEntry {
        id: id.to_string(),
        label: mode.display_name(),
        mode,
        checked: mode == selected,
    })
    .collect()
}

fn parse_tray_polish_mode_id(id: &str) -> Option<PolishMode> {
    match id {
        "style-raw" => Some(PolishMode::Raw),
        "style-light" => Some(PolishMode::Light),
        "style-structured" => Some(PolishMode::Structured),
        "style-formal" => Some(PolishMode::Formal),
        _ => None,
    }
}

fn parse_tray_input_source_id(id: &str) -> Option<DictationInputSource> {
    match id {
        "input-source-microphone" => Some(DictationInputSource::Microphone),
        "input-source-embedded-ble" => Some(DictationInputSource::EmbeddedBle),
        _ => None,
    }
}

fn build_tray_menu<M: Manager<tauri::Wry>>(
    app: &M,
    coordinator: &Arc<coordinator::Coordinator>,
) -> tauri::Result<TrayMenu> {
    let input_source_menu = build_input_source_tray_menu(app, coordinator)?;
    let microphone_menu = build_microphone_tray_menu(app, coordinator)?;
    let dark_mode = CheckMenuItemBuilder::with_id("dark-mode", "深色模式")
        .checked(coordinator.prefs().get().dark_mode)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "退出 Listener Type").build(app)?;
    let mut builder = MenuBuilder::new(app);
    let style_menu = if tray_style_menu_enabled() {
        Some(build_style_tray_menu(app, coordinator)?)
    } else {
        None
    };
    if let Some(style_menu) = &style_menu {
        builder = builder.item(&style_menu.submenu);
    }
    let menu = builder
        .items(&[
            &dark_mode,
            &input_source_menu.submenu,
            &microphone_menu.submenu,
            &quit,
        ])
        .build()?;
    Ok(TrayMenu {
        menu,
        microphone_items: microphone_menu.items,
    })
}

fn build_style_tray_menu<M: Manager<tauri::Wry>>(
    app: &M,
    coordinator: &Arc<coordinator::Coordinator>,
) -> tauri::Result<StyleTrayMenu> {
    let prefs = coordinator.prefs().get();
    let selected = coordinator
        .style_packs()
        .get_or_default_active(&prefs.active_style_pack_id)
        .map(|pack| pack.base_mode)
        .unwrap_or(prefs.default_mode);
    let mut submenu = SubmenuBuilder::with_id(app, "style", "输出风格");
    for entry in tray_polish_mode_menu_entries(selected) {
        let item = CheckMenuItemBuilder::with_id(&entry.id, entry.label)
            .checked(entry.checked)
            .build(app)?;
        submenu = submenu.item(&item);
    }
    Ok(StyleTrayMenu {
        submenu: submenu.build()?,
    })
}

fn build_input_source_tray_menu<M: Manager<tauri::Wry>>(
    app: &M,
    coordinator: &Arc<coordinator::Coordinator>,
) -> tauri::Result<InputSourceTrayMenu> {
    let selected = coordinator.prefs().get().dictation_input_source;
    let microphone = CheckMenuItemBuilder::with_id("input-source-microphone", "麦克风")
        .checked(selected == DictationInputSource::Microphone)
        .build(app)?;
    let embedded_ble = CheckMenuItemBuilder::with_id("input-source-embedded-ble", "Listener BLE")
        .checked(selected == DictationInputSource::EmbeddedBle)
        .build(app)?;
    let submenu = SubmenuBuilder::with_id(app, "input-source", "输入源")
        .items(&[&microphone, &embedded_ble])
        .build()?;
    Ok(InputSourceTrayMenu { submenu })
}

fn build_microphone_tray_menu<M: Manager<tauri::Wry>>(
    app: &M,
    coordinator: &Arc<coordinator::Coordinator>,
) -> tauri::Result<MicrophoneTrayMenu> {
    let selected = coordinator.prefs().get().microphone_device_name;
    let mut items = Vec::new();
    let mut submenu = SubmenuBuilder::with_id(app, "microphone", "选择麦克风");
    let devices = match recorder::list_input_devices() {
        Ok(devices) => devices,
        Err(err) => {
            log::warn!("[tray] list microphone devices failed: {err}");
            Vec::new()
        }
    };
    let selected_available =
        selected.trim().is_empty() || devices.iter().any(|device| device.name == selected);

    let default_item = CheckMenuItemBuilder::with_id("mic-default", "系统默认麦克风")
        .checked(selected.trim().is_empty() || !selected_available)
        .build(app)?;
    submenu = submenu.item(&default_item);
    items.push(commands::TrayMicrophoneMenuItem {
        id: "mic-default".to_string(),
        device_name: String::new(),
        item: default_item,
    });

    if devices.is_empty() {
        let empty = MenuItemBuilder::with_id("mic-empty", "未发现麦克风")
            .enabled(false)
            .build(app)?;
        submenu = submenu.item(&empty);
    } else {
        for (index, device) in devices.into_iter().enumerate() {
            let id = format!("mic-device-{index}");
            let label = if device.is_default {
                format!("{}（系统默认）", device.name)
            } else {
                device.name.clone()
            };
            let item = CheckMenuItemBuilder::with_id(&id, label)
                .checked(selected == device.name)
                .build(app)?;
            submenu = submenu.item(&item);
            items.push(commands::TrayMicrophoneMenuItem {
                id,
                device_name: device.name,
                item,
            });
        }
    }

    Ok(MicrophoneTrayMenu {
        submenu: submenu.build()?,
        items,
    })
}

pub(crate) fn refresh_tray_microphone_menu(app: &AppHandle) -> tauri::Result<()> {
    let coordinator = app.state::<Arc<coordinator::Coordinator>>();
    let tray_menu = build_tray_menu(app, &coordinator)?;
    if let Some(tray) = app.tray_by_id("main-tray") {
        tray.set_menu(Some(tray_menu.menu))?;
    }
    let state = app.state::<commands::TrayMicrophoneMenuState>();
    *state.lock() = tray_menu.microphone_items;
    Ok(())
}

fn microphone_device_signature() -> Option<Vec<(String, bool)>> {
    match recorder::list_input_devices() {
        Ok(devices) => Some(
            devices
                .into_iter()
                .map(|device| (device.name, device.is_default))
                .collect(),
        ),
        Err(err) => {
            log::warn!("[tray] watch microphone devices failed: {err}");
            None
        }
    }
}

fn start_tray_microphone_watcher(app: AppHandle) {
    TRAY_MICROPHONE_WATCHER_STOPPING.store(false, Ordering::Relaxed);
    if let Err(err) = std::thread::Builder::new()
        .name("listener-type-tray-mic-watch".into())
        .spawn(move || {
            let mut last_signature = microphone_device_signature();
            while !TRAY_MICROPHONE_WATCHER_STOPPING.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1500));
                if TRAY_MICROPHONE_WATCHER_STOPPING.load(Ordering::Relaxed) {
                    break;
                }
                let signature = microphone_device_signature();
                if signature == last_signature {
                    continue;
                }
                last_signature = signature;
                let app = app.clone();
                let refresh_app = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Err(err) = refresh_tray_microphone_menu(&refresh_app) {
                        log::warn!(
                            "[tray] refresh microphone menu after device change failed: {err}"
                        );
                    }
                    let _ = refresh_app.emit("microphone:devices-changed", serde_json::json!({}));
                });
            }
        })
    {
        log::warn!("[tray] start microphone watcher failed: {err}");
    }
}

fn handle_dark_mode_toggle(app: &AppHandle) {
    let coord = app.state::<Arc<coordinator::Coordinator>>();
    let mut prefs = coord.prefs().get();
    prefs.dark_mode = !prefs.dark_mode;
    if let Err(err) = coord.prefs().set(prefs.clone()) {
        log::warn!("[tray] save dark mode preference failed: {err}");
        return;
    }
    let _ = app.emit("prefs:changed", &prefs);
    let _ = app.emit("dark-mode-changed", prefs.dark_mode);
    #[cfg(target_os = "windows")]
    {
        if let Some(window) = app.get_webview_window("main") {
            apply_windows_caption_color_for_theme(&window, prefs.dark_mode);
        }
    }
}

fn handle_microphone_tray_menu_event(app: &AppHandle, id: &str) {
    let tray_items = app.state::<commands::TrayMicrophoneMenuState>();
    let items = tray_items.lock();
    let Some(selected) = items.iter().find(|item| item.id == id) else {
        return;
    };

    let coord = app.state::<Arc<coordinator::Coordinator>>();
    let mut prefs = coord.prefs().get();
    prefs.microphone_device_name = selected.device_name.clone();
    if let Err(err) = coord.prefs().set(prefs.clone()) {
        log::warn!("[tray] save microphone preference failed: {err}");
        return;
    }
    let _ = app.emit("prefs:changed", &prefs);

    commands::sync_tray_microphone_selection(&items, &selected.device_name);
}

fn handle_input_source_tray_menu_event(app: &AppHandle, id: &str) -> bool {
    let Some(source) = parse_tray_input_source_id(id) else {
        return false;
    };

    let coord = app.state::<Arc<coordinator::Coordinator>>();
    let mut prefs = coord.prefs().get();
    prefs.dictation_input_source = source;
    prefs.dictation_input_source_user_overridden = true;
    if let Err(err) = coord.prefs().set(prefs.clone()) {
        log::warn!("[tray] save input source preference failed: {err}");
        return true;
    }
    let _ = app.emit("prefs:changed", &prefs);
    coord.refresh_embedded_ble_listener();
    if let Err(err) = refresh_tray_microphone_menu(app) {
        log::warn!("[tray] refresh after input source change failed: {err}");
    }
    true
}

fn handle_style_tray_menu_event(app: &AppHandle, id: &str) -> bool {
    let Some(mode) = parse_tray_polish_mode_id(id) else {
        return false;
    };
    let coord = app.state::<Arc<coordinator::Coordinator>>();
    if let Err(err) = commands::activate_builtin_style_mode(&coord, app, mode) {
        log::warn!("[tray] activate builtin style mode failed: {err}");
        return true;
    }
    if let Err(err) = refresh_tray_microphone_menu(app) {
        log::warn!("[tray] refresh style menu after polish mode change failed: {err}");
    }
    true
}

/// 把 Win11 原生标题栏底色刷成白色，与应用 sidebar 视觉统一。需要 Win11 22H2+
/// (Build 22621+) 才支持 `DWMWA_CAPTION_COLOR`(35)；老 Windows 上 DwmSetWindowAttribute
/// 返回错误，仅打 warn 不阻塞启动。
#[cfg(target_os = "windows")]
fn apply_windows_caption_color<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    apply_windows_caption_color_for_theme(window, false);
}

#[cfg(target_os = "windows")]
fn apply_windows_caption_color_for_theme<R: Runtime>(window: &tauri::WebviewWindow<R>, dark: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR};

    let handle = match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle,
        Ok(other) => {
            log::warn!("[main] unexpected raw window handle for caption color: {other:?}");
            return;
        }
        Err(e) => {
            log::warn!("[main] read raw window handle for caption color failed: {e}");
            return;
        }
    };
    let hwnd = HWND(handle.hwnd.get() as *mut core::ffi::c_void);

    // COLORREF 0x00BBGGRR 编码。
    // Light: rgb(245,245,247) → 0x00F7F5F5（跟 WindowChrome glass 起始色一致）
    // Dark:  rgb(28,28,31)    → 0x001F1C1C（跟 --ol-canvas 一致）
    let colorref: u32 = if dark { 0x001F1C1C } else { 0x00F7F5F5 };
    unsafe {
        if let Err(e) = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &colorref as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&colorref) as u32,
        ) {
            log::warn!("[main] set caption color failed (likely pre-22H2 Win): {e}");
        }
    }
}

#[tauri::command]
fn restart_app(app: AppHandle) {
    // macOS：自动更新会让新装的 .app 带 com.apple.quarantine（无论 Tauri updater
    // 怎么解包，下载流由 LaunchServices 接管，输出物可能仍带 xattr）。如果不
    // strip，重启后 Gatekeeper 会拦着说"Listener Type 已损坏 / 来自未识别开发者"，
    // 用户必须自己开终端跑 xattr -cr 才能继续用 — 违反了"自动更新对用户应该零摩擦"。
    //
    // 在 restart 前阻塞地清一次 xattr。失败容忍（PATH 异常、xattr 不存在、磁盘
    // 只读等边角情况），不让它阻塞重启本身。
    #[cfg(target_os = "macos")]
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bundle) = exe
            .ancestors()
            .find(|p| p.extension().map(|e| e == "app").unwrap_or(false))
        {
            let _ = std::process::Command::new("/usr/bin/xattr")
                .arg("-cr")
                .arg(bundle)
                .status();
            log::info!("[updater] stripped xattr on {:?} before restart", bundle);
        }
    }
    app.restart();
}

/// 把日志同时写到 stderr + ~/Library/Logs/Listener Type/listener-type.log（match Swift `Log.swift`）。
fn init_file_logger() {
    use simplelog::{
        ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, TermLogger, TerminalMode,
        WriteLogger,
    };
    let log_dir = log_dir_path();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = log_dir.join("listener-type.log");
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let config = ConfigBuilder::new().set_time_format_rfc3339().build();
    let mut loggers: Vec<Box<dyn simplelog::SharedLogger>> = vec![TermLogger::new(
        level,
        config.clone(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )];
    let rotation_err = match OnlineRotatingLogWriter::open(log_file) {
        Ok(writer) => {
            loggers.push(WriteLogger::new(level, config, writer));
            None
        }
        Err(error) => Some(error),
    };
    let _ = CombinedLogger::init(loggers);
    if let Some(e) = rotation_err {
        log::warn!("[logger] 日志轮转失败: {e}");
    }
}

struct OnlineRotatingLogWriter {
    path: std::path::PathBuf,
    archive: std::path::PathBuf,
    file: Option<std::fs::File>,
    active_len: u64,
    pending_record: Vec<u8>,
    discarding_record_tail: bool,
}

impl OnlineRotatingLogWriter {
    fn open(path: std::path::PathBuf) -> std::io::Result<Self> {
        rotate_log_if_too_large(&path)?;
        let archive = path.with_file_name("listener-type.log.1");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let active_len = file.metadata()?.len();
        Ok(Self {
            path,
            archive,
            file: Some(file),
            active_len,
            pending_record: Vec::with_capacity(1024),
            discarding_record_tail: false,
        })
    }

    fn write_pending_record(&mut self) -> std::io::Result<()> {
        if self.pending_record.is_empty() {
            return Ok(());
        }
        let record = std::mem::take(&mut self.pending_record);
        if self.active_len > 0
            && self.active_len.saturating_add(record.len() as u64) > LOG_SEGMENT_LIMIT_BYTES
        {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("rotating log file is unavailable"))?;
        std::io::Write::write_all(file, &record)?;
        self.active_len = self.active_len.saturating_add(record.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        if let Some(mut file) = self.file.take() {
            std::io::Write::flush(&mut file)?;
        }
        match std::fs::remove_file(&self.archive) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        std::fs::rename(&self.path, &self.archive)?;
        self.file = Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        self.active_len = 0;
        Ok(())
    }
}

impl std::io::Write for OnlineRotatingLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &byte in buf {
            if self.discarding_record_tail {
                if byte == b'\n' {
                    self.pending_record
                        .extend_from_slice(b"...[diagnostic record truncated]\n");
                    self.discarding_record_tail = false;
                    self.write_pending_record()?;
                }
                continue;
            }

            if byte == b'\n' {
                self.pending_record.push(byte);
                self.write_pending_record()?;
            } else if self.pending_record.len() < LOG_MAX_RECORD_BYTES {
                self.pending_record.push(byte);
            } else {
                self.discarding_record_tail = true;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.discarding_record_tail {
            self.pending_record
                .extend_from_slice(b"...[diagnostic record truncated]");
            self.discarding_record_tail = false;
        }
        self.write_pending_record()?;
        if let Some(file) = self.file.as_mut() {
            std::io::Write::flush(file)?;
        }
        Ok(())
    }
}

fn rotate_log_if_too_large(path: &std::path::Path) -> std::io::Result<()> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() <= LOG_SEGMENT_LIMIT_BYTES {
        return Ok(());
    }

    let archive = path.with_file_name("listener-type.log.1");
    match std::fs::remove_file(&archive) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut source = std::fs::File::open(path)?;
    let keep = metadata.len().min(LOG_SEGMENT_LIMIT_BYTES);
    std::io::Seek::seek(
        &mut source,
        std::io::SeekFrom::Start(metadata.len().saturating_sub(keep)),
    )?;
    let mut newest = Vec::with_capacity(keep as usize);
    std::io::Read::read_to_end(&mut source, &mut newest)?;
    std::fs::write(&archive, newest)?;
    std::fs::remove_file(path)
}

fn app_profile_dir_name() -> &'static str {
    "Listener Type"
}

pub fn log_dir_path() -> std::path::PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir()
            .join("listener-type-test-logs")
            .join(std::process::id().to_string())
    }

    #[cfg(not(test))]
    {
        #[cfg(target_os = "macos")]
        {
            if let Ok(home) = std::env::var("HOME") {
                return std::path::PathBuf::from(home)
                    .join("Library")
                    .join("Logs")
                    .join(app_profile_dir_name());
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                return std::path::PathBuf::from(local)
                    .join(app_profile_dir_name())
                    .join("Logs");
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Ok(home) = std::env::var("HOME") {
                return std::path::PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join(app_profile_dir_name())
                    .join("logs");
            }
        }
        std::env::temp_dir().join(app_profile_dir_name())
    }
}

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    activate_window_mode(app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        restore_main_window_native(&w);
        let _ = w.unminimize();
        restore_main_window_layout_if_needed(&w);
        if main_window_needs_recenter(&w) {
            let _ = w.center();
            restore_main_window_native(&w);
        }
        let _ = w.show();
        let _ = w.unminimize();
        restore_main_window_native(&w);
        let _ = w.set_focus();
    }
    activate_app(app);
}

fn restore_main_window_layout_if_needed<R: Runtime>(window: &WebviewWindow<R>) {
    if let Ok(size) = window.outer_size() {
        if size.width < 400 || size.height < 300 {
            log::warn!(
                "[main] restoring tiny main window size {}x{} to default",
                size.width,
                size.height
            );
            let _ = window.set_size(LogicalSize::new(1240.0, 800.0));
        }
    }
    if main_window_needs_recenter(window) {
        let _ = window.center();
    }
}

fn main_window_needs_recenter<R: Runtime>(window: &WebviewWindow<R>) -> bool {
    if let Ok(position) = window.outer_position() {
        if position.x <= -10_000 || position.y <= -10_000 {
            return true;
        }
    }
    if let Ok(size) = window.outer_size() {
        if size.width < 400 || size.height < 300 {
            return true;
        }
    }
    false
}

#[cfg(target_os = "windows")]
fn restore_main_window_native<R: Runtime>(window: &WebviewWindow<R>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetWindowRect, GetWindowTextLengthW,
        GetWindowThreadProcessId, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_NOTOPMOST,
        HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE, SW_SHOW,
    };

    struct RestoreWindowState {
        process_id: u32,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    }

    unsafe extern "system" fn show_same_process_host_windows(
        candidate: HWND,
        lparam: LPARAM,
    ) -> BOOL {
        let state = &*(lparam.0 as *const RestoreWindowState);
        let mut candidate_process_id = 0;
        GetWindowThreadProcessId(candidate, Some(&mut candidate_process_id));
        if candidate_process_id != state.process_id {
            return BOOL(1);
        }
        if GetWindowTextLengthW(candidate) > 0 {
            return BOOL(1);
        }

        let mut rect = RECT::default();
        if GetWindowRect(candidate, &mut rect).is_err() {
            return BOOL(1);
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width < 400 || height < 300 {
            return BOOL(1);
        }

        let _ = ShowWindow(candidate, SW_SHOW);
        let _ = SetWindowPos(
            candidate,
            HWND_TOPMOST,
            state.left,
            state.top,
            state.width,
            state.height,
            SWP_SHOWWINDOW,
        );
        let _ = SetWindowPos(
            candidate,
            HWND_NOTOPMOST,
            state.left,
            state.top,
            state.width,
            state.height,
            SWP_SHOWWINDOW,
        );
        BOOL(1)
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW;
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);

        let mut process_id = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        let mut rect = RECT::default();
        if process_id != 0 && GetWindowRect(hwnd, &mut rect).is_ok() {
            let state = RestoreWindowState {
                process_id,
                left: rect.left,
                top: rect.top,
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
            };
            let _ = EnumWindows(
                Some(show_same_process_host_windows),
                LPARAM(&state as *const RestoreWindowState as isize),
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn restore_main_window_native<R: Runtime>(_window: &WebviewWindow<R>) {}

/// 把 CLI intent 路由到 coordinator。两个入口共用：
/// 1. 首次启动（lib.rs setup 末尾）
/// 2. single-instance 回调（第二个进程被拦截后转发 argv）
///
/// 异步动作（start_dictation / stop_dictation 是 async）通过 tauri 自带 runtime spawn，
/// 不阻塞回调线程。所有动作都按 coordinator 当前状态自检：
/// - ToggleDictation 在 Idle → start，在 Listening → stop，Starting/Processing/Inserting 忽略并记日志
/// - ToggleQa 直接转发到 handle_qa_hotkey_pressed（语义等同于按一次 QA 热键）
/// - CancelDictation 直接调 cancel（cancel 本身在非 Listening 时也安全）
#[derive(Clone, Copy, Debug, Default)]
struct CliDispatchOptions {
    suppress_capsule_window: bool,
    force_raw_output: bool,
}

struct ScopedCapsuleSuppression {
    previous: Option<String>,
    active: bool,
}

impl ScopedCapsuleSuppression {
    fn apply(active: bool) -> Self {
        if active {
            let previous = std::env::var(SUPPRESS_CAPSULE_WINDOW_ENV).ok();
            std::env::set_var(SUPPRESS_CAPSULE_WINDOW_ENV, "1");
            log::info!("[cli] capsule window suppressed for automation intent");
            Self {
                previous,
                active: true,
            }
        } else {
            Self {
                previous: None,
                active: false,
            }
        }
    }
}

impl Drop for ScopedCapsuleSuppression {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Some(previous) = self.previous.as_ref() {
            std::env::set_var(SUPPRESS_CAPSULE_WINDOW_ENV, previous);
        } else {
            std::env::remove_var(SUPPRESS_CAPSULE_WINDOW_ENV);
        }
        log::info!("[cli] capsule window suppression restored after automation intent");
    }
}

struct ScopedForceRawOutput {
    previous: Option<String>,
    active: bool,
}

impl ScopedForceRawOutput {
    fn apply(active: bool) -> Self {
        if active {
            let previous = std::env::var(FORCE_RAW_OUTPUT_ENV).ok();
            std::env::set_var(FORCE_RAW_OUTPUT_ENV, "1");
            log::info!("[cli] force raw output scoped for automation intent");
            Self {
                previous,
                active: true,
            }
        } else {
            Self {
                previous: None,
                active: false,
            }
        }
    }
}

impl Drop for ScopedForceRawOutput {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Some(previous) = self.previous.take() {
            std::env::set_var(FORCE_RAW_OUTPUT_ENV, previous);
        } else {
            std::env::remove_var(FORCE_RAW_OUTPUT_ENV);
        }
    }
}

fn dispatch_cli_intent<R: Runtime>(
    app: &AppHandle<R>,
    intent: cli::CliIntent,
    options: CliDispatchOptions,
) {
    let coordinator = app
        .try_state::<Arc<coordinator::Coordinator>>()
        .map(|s| Arc::clone(&*s));
    let Some(coordinator) = coordinator else {
        log::warn!("[cli] coordinator not yet managed; dropping intent={intent:?}");
        return;
    };
    match intent {
        cli::CliIntent::Quit => {
            log::info!("[cli] quit: requesting normal Listener Type shutdown");
            request_app_quit(app);
        }
        cli::CliIntent::ToggleDictation => {
            let coord = Arc::clone(&coordinator);
            tauri::async_runtime::spawn(async move {
                let phase = coord.dictation_phase_for_cli();
                use coordinator_state::SessionPhase;
                match phase {
                    SessionPhase::Idle => {
                        log::info!("[cli] toggle-dictation: Idle → start_dictation");
                        if let Err(e) = coord.start_dictation().await {
                            log::warn!("[cli] start_dictation failed: {e}");
                        }
                    }
                    SessionPhase::Listening => {
                        log::info!("[cli] toggle-dictation: Listening → stop_dictation");
                        if let Err(e) = coord.stop_dictation().await {
                            log::warn!("[cli] stop_dictation failed: {e}");
                        }
                    }
                    SessionPhase::Starting => {
                        // 复用 stop_dictation 自身的 Starting → pending_stop 处理，
                        // 与按一次主热键的行为对齐（issue #51）。
                        log::info!("[cli] toggle-dictation: Starting → stop_dictation (pending)");
                        if let Err(e) = coord.stop_dictation().await {
                            log::warn!("[cli] stop_dictation failed: {e}");
                        }
                    }
                    other => {
                        log::info!("[cli] toggle-dictation ignored (phase={other:?})");
                    }
                }
            });
        }
        cli::CliIntent::ToggleQa => {
            let coord = Arc::clone(&coordinator);
            tauri::async_runtime::spawn(async move {
                log::info!("[cli] toggle-qa: dispatching to qa hotkey handler");
                coord.cli_toggle_qa_panel().await;
            });
        }
        cli::CliIntent::CancelDictation => {
            log::info!("[cli] cancel-dictation: invoking cancel");
            coordinator.cancel_dictation();
        }
        cli::CliIntent::SubmitEmbeddedAudioFile { path, format } => {
            let coord = Arc::clone(&coordinator);
            let suppress_capsule_window = options.suppress_capsule_window;
            let force_raw_output = options.force_raw_output;
            tauri::async_runtime::spawn(async move {
                let _suppression = ScopedCapsuleSuppression::apply(suppress_capsule_window);
                let _force_raw = ScopedForceRawOutput::apply(force_raw_output);
                log::info!(
                    "[cli] submit-embedded-audio-file: path={} format={format:?}",
                    path.display()
                );
                match coord.submit_embedded_audio_file(path, format).await {
                    Ok(result) => log::info!(
                        "[cli] submit-embedded-audio-file done: pcm_bytes={} missing_packets={}",
                        result.reconstructed_pcm_bytes,
                        result.stats.missing_packet_count
                    ),
                    Err(err) => log::warn!("[cli] submit-embedded-audio-file failed: {err}"),
                }
            });
        }
        cli::CliIntent::SubmitEmbeddedAudioStreamingFile { path, format } => {
            let coord = Arc::clone(&coordinator);
            let suppress_capsule_window = options.suppress_capsule_window;
            let force_raw_output = options.force_raw_output;
            tauri::async_runtime::spawn(async move {
                let _suppression = ScopedCapsuleSuppression::apply(suppress_capsule_window);
                let _force_raw = ScopedForceRawOutput::apply(force_raw_output);
                log::info!(
                    "[cli] submit-embedded-audio-streaming-file: path={} format={format:?}",
                    path.display()
                );
                match coord.submit_embedded_audio_streaming_file(path, format).await {
                    Ok(result) => log::info!(
                        "[cli] submit-embedded-audio-streaming-file done: pcm_bytes={} missing_packets={}",
                        result.reconstructed_pcm_bytes,
                        result.stats.missing_packet_count
                    ),
                    Err(err) => {
                        log::warn!("[cli] submit-embedded-audio-streaming-file failed: {err}")
                    }
                }
            });
        }
        cli::CliIntent::SubmitEmbeddedAudioBleOnce { timeout_ms } => {
            let coord = Arc::clone(&coordinator);
            tauri::async_runtime::spawn(async move {
                log::info!("[cli] submit-embedded-audio-ble-once: timeout_ms={timeout_ms:?}");
                match coord.submit_embedded_audio_ble_once(timeout_ms).await {
                    Ok(result) => {
                        let result_json = serde_json::to_string(&result)
                            .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
                        println!("embedded_audio_ble_once_result_json={result_json}");
                        log::debug!("embedded_audio_ble_once_result_json={result_json}");
                        log::info!(
                            "[cli] submit-embedded-audio-ble-once done: pcm_bytes={} missing_packets={} final_text_chars={}",
                            result.reconstructed_pcm_bytes,
                            result.stats.missing_packet_count,
                            result
                                .transcript
                                .as_ref()
                                .map(|transcript| transcript.final_text.chars().count())
                                .unwrap_or(0)
                        );
                    }
                    Err(err) => log::warn!("[cli] submit-embedded-audio-ble-once failed: {err}"),
                }
            });
        }
        cli::CliIntent::SubmitEmbeddedAudioBleStream { timeout_ms } => {
            let coord = Arc::clone(&coordinator);
            tauri::async_runtime::spawn(async move {
                log::info!("[cli] submit-embedded-audio-ble-stream: timeout_ms={timeout_ms:?}");
                match coord.submit_embedded_audio_ble_stream(timeout_ms).await {
                    Ok(result) => {
                        let result_json = serde_json::to_string(&result)
                            .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
                        println!("embedded_audio_ble_stream_result_json={result_json}");
                        log::debug!("embedded_audio_ble_stream_result_json={result_json}");
                        log::info!(
                            "[cli] submit-embedded-audio-ble-stream done: pcm_bytes={} missing_packets={} final_text_chars={}",
                            result.reconstructed_pcm_bytes,
                            result.stats.missing_packet_count,
                            result
                                .transcript
                                .as_ref()
                                .map(|transcript| transcript.final_text.chars().count())
                                .unwrap_or(0)
                        );
                    }
                    Err(err) => log::warn!("[cli] submit-embedded-audio-ble-stream failed: {err}"),
                }
            });
        }
        cli::CliIntent::ProbeEmbeddedAudioBleSubscription { timeout_ms } => {
            let coord = Arc::clone(&coordinator);
            tauri::async_runtime::spawn(async move {
                log::info!(
                    "[cli] probe-embedded-audio-ble-subscription: timeout_ms={timeout_ms:?}"
                );
                match coord
                    .probe_embedded_audio_ble_subscription(timeout_ms)
                    .await
                {
                    Ok(()) => {
                        println!("embedded_ble_probe_result=PASS");
                        log::info!("[cli] probe-embedded-audio-ble-subscription PASS");
                    }
                    Err(err) => {
                        println!("embedded_ble_probe_result=FAIL error={err}");
                        log::warn!("[cli] probe-embedded-audio-ble-subscription failed: {err}");
                    }
                }
            });
        }
        cli::CliIntent::SendEmbeddedAudioControlStop { .. } => {
            log::warn!("[cli] embedded BLE control stop is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::ReadEmbeddedAudioBleStatus { .. } => {
            log::warn!("[cli] embedded BLE status read is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::ProbeListenerOtaV1Gatt { .. } => {
            log::warn!("[cli] Listener OTA v1 GATT probe is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::ProbeListenerOtaV1ActiveHandoff { .. } => {
            log::warn!("[cli] Listener OTA v1 active-link probe is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::PromptEmbeddedBlePairing { .. } => {
            log::warn!("[cli] embedded BLE pairing prompt is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::PromptEmbeddedBlePairingOnly { .. } => {
            log::warn!("[cli] embedded BLE pairing-only prompt is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::CleanupEmbeddedBlePairing { .. } => {
            log::warn!("[cli] embedded BLE pairing cleanup is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::LocalWakeHelper => {
            log::warn!("[cli] local wake helper is headless-only and was ignored by the running GUI instance");
        }
        cli::CliIntent::FirmwareOta {
            manifest_path,
            firmware_path,
            preflight_only,
            transfer,
        } => {
            let phase = coordinator.dictation_phase_for_cli();
            tauri::async_runtime::spawn(async move {
                let options = firmware_ota::FirmwareOtaHeadlessOptions {
                    manifest_path,
                    firmware_path,
                    preflight_only,
                    transfer,
                    desktop_version: env!("CARGO_PKG_VERSION").to_string(),
                    expected_hardware_revision: "keyboard-v2-n16r8".to_string(),
                    current_firmware_version: None,
                    recording_active: phase != coordinator_state::SessionPhase::Idle,
                    dictation_phase: Some(format!("{phase:?}")),
                };
                let report = firmware_ota::run_headless(options).await;
                match serde_json::to_string(&report) {
                    Ok(json) => {
                        log::info!("firmware_ota_result_json={json}");
                        if report.status == "PASS" {
                            log::info!("[cli] firmware OTA {} PASS", report.mode);
                        } else {
                            log::warn!(
                                "[cli] firmware OTA {} FAIL: {}",
                                report.mode,
                                report.errors.join("; ")
                            );
                        }
                    }
                    Err(err) => log::warn!("[cli] firmware OTA report serialization failed: {err}"),
                }
            });
        }
        cli::CliIntent::WiredFirmware { .. } => {
            log::warn!("[cli] wired firmware commands are headless-only and were ignored by the running GUI instance");
        }
    }
}

fn run_embedded_ble_headless_cli(intent: cli::CliIntent) -> i32 {
    init_file_logger();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("listener-type-embedded-ble-cli")
        .enable_time()
        .enable_io()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("embedded_ble_error=failed to create runtime: {err}");
            return 2;
        }
    };

    let foundry_local_runtime = Arc::new(asr::local::FoundryLocalRuntime::new());
    #[cfg(target_os = "windows")]
    let coordinator = Arc::new(coordinator::Coordinator::new_with_foundry_runtime(
        Arc::clone(&foundry_local_runtime),
    ));
    #[cfg(not(target_os = "windows"))]
    let coordinator = Arc::new(coordinator::Coordinator::new());
    sync_headless_embedded_ble_target_name_from_firmware(&coordinator);

    match intent {
        cli::CliIntent::ProbeListenerOtaV1ActiveHandoff { timeout_ms } => {
            let target_timeout_ms = timeout_ms.unwrap_or(1_500).clamp(1_000, 1_500);
            log::info!(
                "[cli] headless probe-listener-ota-v1-active-handoff: target_timeout_ms={target_timeout_ms}"
            );
            let report = runtime.block_on(async {
                let listener_ready_started = Instant::now();
                coordinator.auto_select_embedded_ble_input_source_in_background();
                let listener_ready = coordinator
                    .wait_for_embedded_ble_listener_ready_before_firmware_ota(Duration::from_secs(8))
                    .await;
                let listener_ready_elapsed_ms = listener_ready_started.elapsed().as_millis() as u64;
                let listener_ready = match listener_ready {
                    Ok(true) => true,
                    Ok(false) => false,
                    Err(error) => {
                        return serde_json::json!({
                            "status": "FAIL",
                            "backend": "listener-type-rust-winrt",
                            "backgroundListenerReady": false,
                            "listenerReadyElapsedMs": listener_ready_elapsed_ms,
                            "otaPayloadWriteStarted": false,
                            "error": error,
                        });
                    }
                };
                if !listener_ready {
                    return serde_json::json!({
                        "status": "FAIL",
                        "backend": "listener-type-rust-winrt",
                        "backgroundListenerReady": false,
                        "listenerReadyElapsedMs": listener_ready_elapsed_ms,
                        "otaPayloadWriteStarted": false,
                        "error": "Embedded BLE is not the active Type input, so no active-capture OTA handoff was available.",
                    });
                }

                let handoff_started = Instant::now();
                if let Err(error) = crate::embedded_ble::request_listener_ota_v1_active_link(None) {
                    return serde_json::json!({
                        "status": "FAIL",
                        "backend": "listener-type-rust-winrt",
                        "backgroundListenerReady": true,
                        "listenerReadyElapsedMs": listener_ready_elapsed_ms,
                        "handoffElapsedMs": handoff_started.elapsed().as_millis() as u64,
                        "otaPayloadWriteStarted": false,
                        "error": error,
                    });
                }
                let handoff_elapsed_ms = handoff_started.elapsed().as_millis() as u64;
                let target_probe_started = Instant::now();
                let snapshot = crate::embedded_ble::listener_ota_v1_gatt_probe_after_active_link_hint(
                    Duration::from_millis(target_timeout_ms),
                );
                let target_probe_elapsed_ms = target_probe_started.elapsed().as_millis() as u64;
                let active_handoff_elapsed_ms = handoff_elapsed_ms.saturating_add(target_probe_elapsed_ms);
                let has_capability = snapshot
                    .capabilities
                    .iter()
                    .any(|item| item == crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY);
                let status = if snapshot.connected
                    && has_capability
                    && active_handoff_elapsed_ms <= u64::from(target_timeout_ms)
                {
                    "PASS"
                } else {
                    "FAIL"
                };
                serde_json::json!({
                    "status": status,
                    "backend": "listener-type-rust-winrt",
                    "backgroundListenerReady": true,
                    "listenerReadyElapsedMs": listener_ready_elapsed_ms,
                    "handoffElapsedMs": handoff_elapsed_ms,
                    "targetProbeElapsedMs": target_probe_elapsed_ms,
                    "activeHandoffElapsedMs": active_handoff_elapsed_ms,
                    "maxActiveHandoffMs": target_timeout_ms,
                    "otaPayloadWriteStarted": false,
                    "snapshot": snapshot,
                })
            });
            coordinator.request_shutdown();
            let report_json = serde_json::to_string(&report)
                .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
            headless_print_line(format!(
                "listener_ota_v1_active_handoff_probe_json={report_json}"
            ));
            log::info!("listener_ota_v1_active_handoff_probe_json={report_json}");
            if report.get("status").and_then(|value| value.as_str()) == Some("PASS") {
                0
            } else {
                1
            }
        }
        cli::CliIntent::ProbeListenerOtaV1Gatt { timeout_ms } => {
            log::info!("[cli] headless probe-listener-ota-v1-gatt: timeout_ms={timeout_ms:?}");
            let timeout_ms = timeout_ms.unwrap_or(20_000).clamp(1_000, 30_000);
            let snapshot = crate::embedded_ble::listener_ota_v1_gatt_probe_snapshot(
                Duration::from_millis(timeout_ms),
            );
            let has_capability = snapshot
                .capabilities
                .iter()
                .any(|item| item == crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY);
            let status = if snapshot.connected && has_capability {
                "PASS"
            } else {
                "FAIL"
            };
            let report = serde_json::json!({
                "status": status,
                "timeoutMs": timeout_ms,
                "backend": "listener-type-rust-winrt",
                "snapshot": snapshot,
            });
            let report_json = serde_json::to_string(&report)
                .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
            headless_print_line(format!("listener_ota_v1_gatt_probe_json={report_json}"));
            log::info!("listener_ota_v1_gatt_probe_json={report_json}");
            if status == "PASS" {
                0
            } else {
                1
            }
        }
        cli::CliIntent::ProbeEmbeddedAudioBleSubscription { timeout_ms } => {
            log::info!(
                "[cli] headless probe-embedded-audio-ble-subscription: timeout_ms={timeout_ms:?}"
            );
            let timeout =
                std::time::Duration::from_millis(timeout_ms.unwrap_or(10_000).clamp(1_000, 30_000));
            match crate::embedded_ble::probe_notify_subscription(timeout) {
                Ok(()) => {
                    headless_print_line("embedded_ble_probe_result=PASS");
                    log::info!("[cli] probe-embedded-audio-ble-subscription PASS");
                    0
                }
                Err(err) => {
                    headless_print_line(format!("embedded_ble_probe_result=FAIL error={err}"));
                    log::warn!("[cli] probe-embedded-audio-ble-subscription failed: {err}");
                    1
                }
            }
        }
        cli::CliIntent::SendEmbeddedAudioControlStop { timeout_ms } => {
            log::info!(
                "[cli] headless send-embedded-audio-control-stop: timeout_ms={timeout_ms:?}"
            );
            let timeout =
                std::time::Duration::from_millis(timeout_ms.unwrap_or(5_000).clamp(500, 30_000));
            match crate::embedded_ble::send_recording_control_stop(timeout) {
                Ok(()) => {
                    headless_print_line("embedded_ble_control_stop_result=PASS");
                    log::info!("[cli] send-embedded-audio-control-stop PASS");
                    0
                }
                Err(err) => {
                    headless_print_line(format!(
                        "embedded_ble_control_stop_result=FAIL error={err}"
                    ));
                    log::warn!("[cli] send-embedded-audio-control-stop failed: {err}");
                    1
                }
            }
        }
        cli::CliIntent::ReadEmbeddedAudioBleStatus { timeout_ms } => {
            log::info!("[cli] headless read-embedded-audio-ble-status: timeout_ms={timeout_ms:?}");
            let timeout =
                std::time::Duration::from_millis(timeout_ms.unwrap_or(10_000).clamp(1_000, 30_000));
            match crate::embedded_ble::read_embedded_audio_status(timeout) {
                Ok(status) => {
                    let status_json = serde_json::to_string(&status)
                        .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
                    headless_print_line(format!("embedded_audio_ble_status_json={status_json}"));
                    log::info!("embedded_audio_ble_status_json={status_json}");
                    0
                }
                Err(err) => {
                    headless_print_line(format!(
                        "embedded_audio_ble_status_result=FAIL error={err}"
                    ));
                    log::warn!("[cli] read-embedded-audio-ble-status failed: {err}");
                    1
                }
            }
        }
        cli::CliIntent::PromptEmbeddedBlePairing { expected_name } => {
            log::info!(
                "[cli] headless prompt-embedded-ble-pairing: expected_name={expected_name:?}"
            );
            let type_recovery_command_confirmed =
                match crate::embedded_ble::send_recording_control_recovery(
                    std::time::Duration::from_secs(5),
                ) {
                    Ok(()) => {
                        log::info!(
                        "[cli] headless prompt-embedded-ble-pairing opened Listener recovery pairing window"
                    );
                        std::thread::sleep(std::time::Duration::from_millis(2600));
                        true
                    }
                    Err(err) => {
                        log::warn!(
                        "[cli] headless prompt-embedded-ble-pairing recovery command skipped: {err}"
                    );
                        false
                    }
                };
            let result = if type_recovery_command_confirmed {
                crate::embedded_ble::prompt_listener_pairing_after_type_recovery(
                    expected_name.as_deref(),
                )
            } else {
                crate::embedded_ble::prompt_listener_pairing_for_recovery(expected_name.as_deref())
            };
            let result_json = serde_json::to_string(&result)
                .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
            headless_print_line(format!("embedded_ble_pairing_prompt_json={result_json}"));
            log::info!("embedded_ble_pairing_prompt_json={result_json}");
            if result.open_bluetooth_settings {
                1
            } else {
                0
            }
        }
        cli::CliIntent::PromptEmbeddedBlePairingOnly { expected_name } => {
            log::info!(
                "[cli] headless prompt-embedded-ble-pairing-only: expected_name={expected_name:?}"
            );
            let result =
                crate::embedded_ble::prompt_listener_pairing_for_recovery(expected_name.as_deref());
            let result_json = serde_json::to_string(&result)
                .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
            headless_print_line(format!("embedded_ble_pairing_prompt_json={result_json}"));
            log::info!("embedded_ble_pairing_prompt_json={result_json}");
            if result.open_bluetooth_settings {
                1
            } else {
                0
            }
        }
        cli::CliIntent::CleanupEmbeddedBlePairing { expected_name } => {
            log::info!(
                "[cli] headless cleanup-embedded-ble-pairing: expected_name={expected_name:?}"
            );
            if let Some(name) = expected_name.as_deref() {
                crate::embedded_ble::set_configured_bluetooth_target_name(name);
            }
            let extra_names = expected_name.iter().cloned().collect::<Vec<_>>();
            let result = crate::embedded_ble::unpair_listener_devices_for_names(&extra_names);
            let result_json = serde_json::to_string(&result)
                .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
            headless_print_line(format!("embedded_ble_cleanup_json={result_json}"));
            log::info!("embedded_ble_cleanup_json={result_json}");
            if result.needs_user_action && result.failed_devices > 0 {
                1
            } else {
                0
            }
        }
        cli::CliIntent::SubmitEmbeddedAudioBleOnce { timeout_ms } => {
            log::info!("[cli] headless submit-embedded-audio-ble-once: timeout_ms={timeout_ms:?}");
            match runtime.block_on(coordinator.submit_embedded_audio_ble_once(timeout_ms)) {
                Ok(result) => {
                    let result_json = serde_json::to_string(&result)
                        .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
                    headless_print_line(format!(
                        "embedded_audio_ble_once_result_json={result_json}"
                    ));
                    log::debug!("embedded_audio_ble_once_result_json={result_json}");
                    log::info!(
                        "[cli] submit-embedded-audio-ble-once done: pcm_bytes={} missing_packets={} final_text_chars={}",
                        result.reconstructed_pcm_bytes,
                        result.stats.missing_packet_count,
                        result
                            .transcript
                            .as_ref()
                            .map(|transcript| transcript.final_text.chars().count())
                            .unwrap_or(0)
                    );
                    0
                }
                Err(err) => {
                    log::warn!("[cli] submit-embedded-audio-ble-once failed: {err}");
                    1
                }
            }
        }
        cli::CliIntent::SubmitEmbeddedAudioBleStream { timeout_ms } => {
            log::info!(
                "[cli] headless submit-embedded-audio-ble-stream: timeout_ms={timeout_ms:?}"
            );
            match runtime.block_on(coordinator.submit_embedded_audio_ble_stream(timeout_ms)) {
                Ok(result) => {
                    let result_json = serde_json::to_string(&result)
                        .unwrap_or_else(|err| format!("{{\"jsonError\":\"{err}\"}}"));
                    headless_print_line(format!(
                        "embedded_audio_ble_stream_result_json={result_json}"
                    ));
                    log::debug!("embedded_audio_ble_stream_result_json={result_json}");
                    log::info!(
                        "[cli] submit-embedded-audio-ble-stream done: pcm_bytes={} missing_packets={} final_text_chars={}",
                        result.reconstructed_pcm_bytes,
                        result.stats.missing_packet_count,
                        result
                            .transcript
                            .as_ref()
                            .map(|transcript| transcript.final_text.chars().count())
                            .unwrap_or(0)
                    );
                    0
                }
                Err(err) => {
                    log::warn!("[cli] submit-embedded-audio-ble-stream failed: {err}");
                    1
                }
            }
        }
        _ => 2,
    }
}

#[cfg(target_os = "windows")]
fn sync_headless_embedded_ble_target_name_from_firmware(coordinator: &coordinator::Coordinator) {
    let status = match crate::embedded_ble::read_device_settings_status(Duration::from_secs(2)) {
        Ok(status) => status,
        Err(err) => {
            log::info!(
                "[cli] headless BLE target name sync skipped; device settings unavailable: {err}"
            );
            return;
        }
    };
    let firmware_name = status.ble_name.trim();
    let valid = crate::types::device_ble_name_is_valid(firmware_name);
    if status.ble_name_pending_restart || !valid {
        log::info!(
            "[cli] headless BLE target name sync skipped firmware_name={firmware_name:?} pending={} valid={valid}",
            status.ble_name_pending_restart
        );
        return;
    }

    crate::embedded_ble::set_configured_bluetooth_target_name(firmware_name);
    let mut prefs = coordinator.prefs().get();
    if prefs.device_ble_name == firmware_name {
        return;
    }
    let previous = prefs.device_ble_name.clone();
    prefs.device_ble_name = firmware_name.to_string();
    match coordinator.prefs().set(prefs) {
        Ok(()) => log::warn!(
            "[cli] headless BLE target name synced from firmware previous={previous:?} firmware_name={firmware_name:?}"
        ),
        Err(err) => log::warn!(
            "[cli] headless BLE target name sync could not persist previous={previous:?} firmware_name={firmware_name:?}: {err}"
        ),
    }
}

#[cfg(not(target_os = "windows"))]
fn sync_headless_embedded_ble_target_name_from_firmware(_coordinator: &coordinator::Coordinator) {}

fn headless_print_line(line: impl AsRef<str>) {
    use std::io::Write;

    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{}", line.as_ref());
}

fn run_firmware_ota_headless_cli(intent: cli::CliIntent) -> i32 {
    init_file_logger();

    let cli::CliIntent::FirmwareOta {
        manifest_path,
        firmware_path,
        preflight_only,
        transfer,
    } = intent
    else {
        return 2;
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("firmware_ota_error=failed to create runtime: {err}");
            return 2;
        }
    };

    let report = runtime.block_on(firmware_ota::run_headless(
        firmware_ota::FirmwareOtaHeadlessOptions {
            manifest_path,
            firmware_path,
            preflight_only,
            transfer,
            desktop_version: env!("CARGO_PKG_VERSION").to_string(),
            expected_hardware_revision: "keyboard-v2-n16r8".to_string(),
            current_firmware_version: None,
            recording_active: false,
            dictation_phase: Some("Headless".to_string()),
        },
    ));
    match serde_json::to_string(&report) {
        Ok(json) => {
            let line = format!("firmware_ota_result_json={json}");
            headless_print_line(&line);
            log::info!("{line}");
        }
        Err(err) => {
            eprintln!("firmware_ota_error=report serialization failed: {err}");
            return 2;
        }
    }
    if report.status == "PASS" {
        0
    } else {
        1
    }
}

fn run_wired_firmware_headless_cli(intent: cli::CliIntent) -> i32 {
    init_file_logger();

    let cli::CliIntent::WiredFirmware {
        package_path,
        port,
        baud,
        action,
    } = intent
    else {
        return 2;
    };

    let mode = match action {
        cli::WiredFirmwareCliAction::Check => "check",
        cli::WiredFirmwareCliAction::Flash => "flash",
        cli::WiredFirmwareCliAction::BootRepair => "bootRepair",
    };
    let result = match action {
        cli::WiredFirmwareCliAction::Check => {
            commands::load_wired_firmware_package(package_path.to_string_lossy().to_string())
                .map(|package| serde_json::json!({ "package": package }))
        }
        cli::WiredFirmwareCliAction::Flash => {
            commands::run_wired_firmware_flash(&package_path, port.as_deref(), baud, false)
                .map(|flash| serde_json::json!({ "result": flash }))
        }
        cli::WiredFirmwareCliAction::BootRepair => {
            commands::run_wired_bootloader_repair(&package_path, port.as_deref(), baud)
                .map(|repair| serde_json::json!({ "result": repair }))
        }
    };

    let payload = match result {
        Ok(extra) => serde_json::json!({
            "status": "PASS",
            "mode": mode,
            "packagePath": package_path,
            "port": port,
            "baud": baud,
            "desktopVersion": env!("CARGO_PKG_VERSION"),
            "extra": extra,
        }),
        Err(error) => serde_json::json!({
            "status": "FAIL",
            "mode": mode,
            "packagePath": package_path,
            "port": port,
            "baud": baud,
            "desktopVersion": env!("CARGO_PKG_VERSION"),
            "error": error,
        }),
    };

    match serde_json::to_string(&payload) {
        Ok(json) => println!("wired_firmware_result_json={json}"),
        Err(err) => {
            eprintln!("wired_firmware_error=report serialization failed: {err}");
            return 2;
        }
    }
    if payload
        .get("status")
        .and_then(|value| value.as_str())
        .is_some_and(|status| status == "PASS")
    {
        0
    } else {
        1
    }
}

pub(crate) fn request_microphone_from_foreground<R: Runtime>(
    app: &AppHandle<R>,
) -> permissions::PermissionStatus {
    show_main_window(app);
    wait_for_app_activation(app);
    permissions::request_microphone()
}

fn hide_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    activate_menu_bar_mode(app);
}

#[cfg(target_os = "macos")]
fn activate_window_mode<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    let _ = app.set_dock_visibility(true);
    let _ = app.show();
}

#[cfg(not(target_os = "macos"))]
fn activate_window_mode<R: Runtime>(_app: &AppHandle<R>) {}

#[cfg(target_os = "macos")]
fn activate_menu_bar_mode<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let _ = app.set_dock_visibility(false);
}

#[cfg(not(target_os = "macos"))]
fn activate_menu_bar_mode<R: Runtime>(_app: &AppHandle<R>) {}

#[cfg(target_os = "macos")]
fn activate_app<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.run_on_main_thread(|| {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject, Bool};

        unsafe {
            let Some(cls) = AnyClass::get("NSApplication") else {
                return;
            };
            let ns_app: *mut AnyObject = msg_send![cls, sharedApplication];
            if !ns_app.is_null() {
                let _: () = msg_send![ns_app, activateIgnoringOtherApps: Bool::YES];
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn activate_app<R: Runtime>(_app: &AppHandle<R>) {}

/// 展示胶囊后调用：若 Listener Type 已是前台 app，用 makeKeyWindow 还原主窗口焦点。
/// 不调 NSApp.activate，不抢其他 app 焦点，符合 CLAUDE.md 约束。
#[cfg(target_os = "macos")]
pub(crate) fn restore_main_window_key_if_active<R: Runtime>(app: &AppHandle<R>) {
    let main = app.get_webview_window("main");
    let _ = app.run_on_main_thread(move || {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject, Bool};
        unsafe {
            let Some(cls) = AnyClass::get("NSApplication") else {
                return;
            };
            let ns_app: *mut AnyObject = msg_send![cls, sharedApplication];
            if ns_app.is_null() {
                return;
            }
            let is_active: Bool = msg_send![ns_app, isActive];
            if !is_active.as_bool() {
                return;
            }
            let Some(main) = main else {
                return;
            };
            match main.ns_window() {
                Ok(handle) => {
                    let main_win = handle as *mut AnyObject;
                    if !main_win.is_null() {
                        let _: () = msg_send![main_win, makeKeyWindow];
                    }
                }
                Err(e) => log::warn!("[main] ns_window unavailable for key restore: {e}"),
            };
        }
    });
}

#[cfg(target_os = "macos")]
fn wait_for_app_activation<R: Runtime>(app: &AppHandle<R>) {
    let (tx, rx) = mpsc::channel();
    let _ = app.run_on_main_thread(move || {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject, Bool};

        unsafe {
            let Some(cls) = AnyClass::get("NSApplication") else {
                let _ = tx.send(());
                return;
            };
            let ns_app: *mut AnyObject = msg_send![cls, sharedApplication];
            if !ns_app.is_null() {
                let _: () = msg_send![ns_app, activateIgnoringOtherApps: Bool::YES];
            }
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(Duration::from_millis(800));
    std::thread::sleep(Duration::from_millis(150));
}

#[cfg(not(target_os = "macos"))]
fn wait_for_app_activation<R: Runtime>(_app: &AppHandle<R>) {}

/// QA 浮窗的目标尺寸（issue #118）。胶囊默认 220×96 + Dock 80pt + 8pt gap，
/// 算下来 QA 窗口顶部坐标 = h - 80 - 96 - 8 - 280。
const QA_WINDOW_WIDTH: f64 = 380.0;
const QA_WINDOW_HEIGHT: f64 = 440.0;
/// 胶囊与 QA 窗口的间距，与设计稿一致。
const QA_WINDOW_GAP_TO_CAPSULE: f64 = 8.0;
/// 给 macOS Dock 留的下边距（与 capsule 同源）。
const DOCK_BOTTOM_PADDING_FOR_QA: f64 = 80.0;

/// 把 QA 浮窗放到屏幕底部居中、紧贴胶囊上方。tauri 启动期 + show 之前都会调一次，
/// 防止用户切换显示器后位置错乱。
fn position_qa_window<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> tauri::Result<()> {
    let monitor = match window.current_monitor()? {
        Some(m) => m,
        None => return Ok(()),
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let logical_w = size.width as f64 / scale;
    let logical_h = size.height as f64 / scale;
    let capsule_height = capsule_height_for_qa();
    let x = ((logical_w - QA_WINDOW_WIDTH) / 2.0).max(0.0);
    let y = (logical_h
        - DOCK_BOTTOM_PADDING_FOR_QA
        - capsule_height
        - QA_WINDOW_GAP_TO_CAPSULE
        - QA_WINDOW_HEIGHT)
        .max(0.0);
    window.set_size(tauri::LogicalSize::new(QA_WINDOW_WIDTH, QA_WINDOW_HEIGHT))?;
    window.set_position(LogicalPosition::new(x, y))?;
    Ok(())
}

/// 显示 QA 窗口并发一条状态事件（前端订阅 `qa:state`）。
/// `content_kind` 是不透明字符串（"loading" / "answer" / "idle" 等），
/// 让前端 React 视图自行决定渲染哪一种。**不**抢前台 app 焦点（保证 Cmd+C
/// fallback 仍能从原 app 拿到选区）。
pub(crate) fn show_qa_window<R: tauri::Runtime>(app: &AppHandle<R>, content_kind: &str) {
    let Some(window) = app.get_webview_window("qa") else {
        log::info!("[qa] show 跳过：qa 窗口不存在 (content_kind={content_kind})");
        return;
    };
    // 仅首次 show 时居中；之后保留用户拖动后的位置。
    if !QA_WINDOW_POSITIONED.load(Ordering::Relaxed) {
        if let Err(e) = position_qa_window(&window) {
            log::warn!("[qa] position before first show failed: {e}");
        }
        QA_WINDOW_POSITIONED.store(true, Ordering::Relaxed);
    }
    // macOS：不用 window.show()（它会 makeKeyAndOrderFront 把 Listener Type 推成 frontmost，
    // 之后 capture_selection 的 AX read / Cmd+C fallback 都跑在 Listener Type 自己的 webview 上
    // → 抓不到原 app 选区）。改用 orderFrontRegardless 让窗口可见但**不**成为 key window，
    // frontmost 仍是用户原 app，AX 还能读到选区。这是 Spotlight / Raycast 的标准做法。
    //
    // ⚠️ 关键：NSWindow 任何操作必须在主线程，macOS 26 是硬断言（违反直接 SIGTRAP）。
    // show_qa_window 经常从 tokio worker 调（qa_hotkey_bridge_loop），所以裸 ObjC msg_send
    // 必须用 `app.run_on_main_thread` dispatch 到主线程。详见 issue #118 v2。
    #[cfg(target_os = "macos")]
    {
        let window_clone = window.clone();
        let _ = app.run_on_main_thread(move || {
            use objc2::msg_send;
            use objc2::runtime::AnyObject;
            match window_clone.ns_window() {
                Ok(handle) => {
                    let ns = handle as *mut AnyObject;
                    if ns.is_null() {
                        log::warn!("[qa] ns_window null; falling back to window.show()");
                        let _ = window_clone.show();
                    } else {
                        unsafe {
                            let _: () = msg_send![ns, orderFrontRegardless];
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[qa] ns_window unavailable: {e}; falling back to window.show()");
                    let _ = window_clone.show();
                }
            }
        });
    }
    #[cfg(target_os = "windows")]
    if !show_qa_window_no_activate(&window) {
        log::warn!("[qa] show_no_activate failed; falling back to window.show()");
        if let Err(e) = window.show() {
            log::warn!("[qa] show fallback failed: {e}");
        }
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    if let Err(e) = window.show() {
        log::warn!("[qa] show failed: {e}");
    }
    let _ = app.emit_to(
        "qa",
        "qa:state",
        serde_json::json!({ "kind": content_kind }),
    );
}

/// QA 浮窗的拖动修复（macOS）。
///
/// 配置 `focus: false` 让 Tauri 把窗口创建为 nonactivating panel 风格（避免抢前台 app
/// 焦点）。代价是 AppKit 的 `performWindowDragWithEvent:` 在 nonactivating 窗口上无效，
/// 所以 `data-tauri-drag-region` 和 `WebviewWindow::start_dragging()` 都拖不动。
///
/// 解法是把 NSWindow 的 `movableByWindowBackground` 打开——这条路径不依赖窗口是否成为
/// key window，跟 Spotlight / Raycast 的浮窗是同一手法。设一次就够，整个生命周期保持。
#[cfg(target_os = "macos")]
fn make_qa_window_draggable_macos<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    let Ok(handle) = window.ns_window() else {
        log::warn!("[qa] ns_window unavailable; drag fix skipped");
        return;
    };
    let ns_window = handle as *mut AnyObject;
    if ns_window.is_null() {
        log::warn!("[qa] ns_window null; drag fix skipped");
        return;
    }
    unsafe {
        let _: () = msg_send![ns_window, setMovableByWindowBackground: Bool::YES];
        let _: () = msg_send![ns_window, setMovable: Bool::YES];
    }
    log::info!("[qa] NSWindow movableByWindowBackground=YES");
}

/// 隐藏 QA 窗口。供 commands::qa_window_dismiss / coordinator session 收尾共用。
pub(crate) fn hide_qa_window<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("qa") {
        let _ = window.hide();
    }
}

pub(crate) fn prepare_capsule_window_for_overlay<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
) {
    if let Err(e) = window.set_focusable(false) {
        log::warn!("[capsule] set_focusable(false) failed: {e}");
    }
    apply_capsule_windows_no_activate_style(window);
}

#[cfg(target_os = "windows")]
fn apply_capsule_windows_no_activate_style<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW,
    };

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    if hwnd.0.is_null() {
        return;
    }

    let current = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let desired = (current as u32) | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
    if desired as isize != current {
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired as isize);
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_capsule_windows_no_activate_style<R: tauri::Runtime>(_window: &tauri::WebviewWindow<R>) {}

#[cfg(target_os = "windows")]
fn show_qa_window_no_activate<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};

    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    if hwnd.0.is_null() {
        return false;
    }

    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    true
}

/// 选择胶囊应落位的显示器：优先光标所在屏（用户正在看的屏幕），其次胶囊窗口
/// 当前所在屏，再退回主显示器，最后任一可用显示器。
/// 修复：窗口被旧 DPI/拓扑甩到屏外后 `current_monitor()` 返回 None、
/// 胶囊永远无法被重新定位回可见区域的问题。
pub(crate) fn capsule_target_monitor<R: tauri::Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
) -> Option<tauri::Monitor> {
    if let Ok(cursor) = app.cursor_position() {
        if let Ok(Some(monitor)) = app.monitor_from_point(cursor.x, cursor.y) {
            return Some(monitor);
        }
    }
    if let Ok(Some(monitor)) = window.current_monitor() {
        return Some(monitor);
    }
    if let Ok(Some(monitor)) = app.primary_monitor() {
        return Some(monitor);
    }
    app.available_monitors()
        .ok()
        .and_then(|monitors| monitors.into_iter().next())
}

/// 物理矩形是否与任一显示器有正面积交叠（纯函数，便于单测）。
fn rect_intersects_any_monitor(
    rect: (i32, i32, u32, u32),
    monitors: &[(i32, i32, u32, u32)],
) -> bool {
    let (x, y, w, h) = rect;
    let right = x.saturating_add(w as i32);
    let bottom = y.saturating_add(h as i32);
    monitors.iter().any(|&(mx, my, mw, mh)| {
        let mright = mx.saturating_add(mw as i32);
        let mbottom = my.saturating_add(mh as i32);
        right > mx && x < mright && bottom > my && y < mbottom
    })
}

/// 胶囊窗口是否完全落在所有显示器可见区域之外。
pub(crate) fn capsule_window_off_all_monitors<R: tauri::Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
) -> bool {
    let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return false;
    };
    let Ok(monitors) = app.available_monitors() else {
        return false;
    };
    if monitors.is_empty() {
        return false;
    }
    let monitor_rects: Vec<(i32, i32, u32, u32)> = monitors
        .iter()
        .map(|m| {
            (
                m.position().x,
                m.position().y,
                m.size().width,
                m.size().height,
            )
        })
        .collect();
    !rect_intersects_any_monitor(
        (position.x, position.y, size.width, size.height),
        &monitor_rects,
    )
}

/// 把 capsule 窗口移到目标显示器底部居中，与 Swift `CapsuleWindowController.repositionToBottomCenter` 同效。
/// 留 80pt 给 macOS Dock；Windows 任务栏一般在底部 48pt 以内，整体也合适。
pub(crate) fn position_capsule_bottom_center<R: tauri::Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    translation_active: bool,
) -> tauri::Result<()> {
    let Some(monitor) = capsule_target_monitor(app, window) else {
        return Ok(());
    };
    position_capsule_bottom_center_on_monitor(window, &monitor, translation_active)
}

pub(crate) fn position_capsule_bottom_center_on_monitor<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    monitor: &tauri::Monitor,
    translation_active: bool,
) -> tauri::Result<()> {
    let bounds = capsule_window_bounds(translation_active);
    window.set_size(LogicalSize::new(bounds.width, bounds.height))?;

    let scale = monitor.scale_factor();
    let size = monitor.size();
    let origin = monitor.position();
    let (x, y) = capsule_bottom_center_physical_position(
        origin.x,
        origin.y,
        size.width,
        size.height,
        scale,
        bounds,
        capsule_visual_height(translation_active),
    );
    window.set_position(PhysicalPosition::new(x, y))?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CapsuleWindowBounds {
    width: f64,
    height: f64,
    bottom_inset: f64,
}

fn capsule_bottom_center_position(
    monitor_x: i32,
    monitor_y: i32,
    monitor_width: u32,
    monitor_height: u32,
    scale: f64,
    bounds: CapsuleWindowBounds,
    visual_height: f64,
) -> (f64, f64) {
    let logical_w = monitor_width as f64 / scale;
    let logical_h = monitor_height as f64 / scale;
    let local_x = ((logical_w - bounds.width) / 2.0).max(0.0);
    let local_y = (logical_h - visual_height - 80.0 - bounds.bottom_inset).max(0.0);

    // Tauri monitor origins are physical desktop coordinates. Convert the origin too,
    // then add the local anchor so secondary and negative-positioned displays stay valid.
    (
        monitor_x as f64 / scale + local_x,
        monitor_y as f64 / scale + local_y,
    )
}

fn capsule_bottom_center_physical_position(
    monitor_x: i32,
    monitor_y: i32,
    monitor_width: u32,
    monitor_height: u32,
    scale: f64,
    bounds: CapsuleWindowBounds,
    visual_height: f64,
) -> (i32, i32) {
    let (x, y) = capsule_bottom_center_position(
        monitor_x,
        monitor_y,
        monitor_width,
        monitor_height,
        scale,
        bounds,
        visual_height,
    );
    // `Monitor` coordinates and `set_position` both use physical desktop pixels here.
    ((x * scale).round() as i32, (y * scale).round() as i32)
}

fn capsule_window_bounds(translation_active: bool) -> CapsuleWindowBounds {
    #[cfg(target_os = "windows")]
    {
        const WINDOWS_CAPSULE_PILL_WIDTH: f64 = 280.0;
        const WINDOWS_CAPSULE_SIDE_INSET: f64 = 12.0;
        CapsuleWindowBounds {
            // Keep the Windows hitbox in sync with the frontend pill width plus
            // symmetric side insets for shadow room.
            width: WINDOWS_CAPSULE_PILL_WIDTH + WINDOWS_CAPSULE_SIDE_INSET * 2.0,
            height: if translation_active { 118.0 } else { 84.0 },
            bottom_inset: 12.0,
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // macOS / Linux：固定 220×110，与 1.2.11 行为一致 — 录音 / 翻译徽章
        // 共用同一个窗口尺寸，避免按 Shift 后窗口高度变化导致胶囊整体下移。
        let _ = translation_active;
        CapsuleWindowBounds {
            width: 220.0,
            height: 110.0,
            bottom_inset: 0.0,
        }
    }
}

fn capsule_visual_height(_translation_active: bool) -> f64 {
    #[cfg(target_os = "windows")]
    {
        52.0
    }

    #[cfg(not(target_os = "windows"))]
    {
        96.0
    }
}

fn capsule_height_for_qa() -> f64 {
    capsule_visual_height(false)
}

#[cfg(test)]
mod tests {
    use super::{
        capsule_bottom_center_physical_position, capsule_bottom_center_position,
        capsule_height_for_qa, capsule_visual_height, capsule_window_bounds, log_dir_path,
        parse_tray_polish_mode_id, rect_intersects_any_monitor, rotate_log_if_too_large,
        should_hide_main_on_close, should_keep_alive_on_exit_request,
        tray_polish_mode_menu_entries, tray_style_menu_enabled, CapsuleWindowBounds,
        OnlineRotatingLogWriter, LOG_MAX_RECORD_BYTES, LOG_ROTATE_LIMIT_BYTES,
        LOG_SEGMENT_LIMIT_BYTES,
    };
    #[cfg(target_os = "windows")]
    use super::{merge_webview2_test_browser_args, WRY_DEFAULT_DISABLED_WEBVIEW2_FEATURES};
    use crate::types::PolishMode;
    use std::io::Write;

    #[test]
    #[cfg(target_os = "windows")]
    fn webview2_test_browser_args_keep_wry_defaults() {
        let merged = merge_webview2_test_browser_args(
            None,
            "--remote-debugging-port=9224 --remote-allow-origins=*",
        )
        .expect("test browser args should be produced");

        assert!(merged.contains(WRY_DEFAULT_DISABLED_WEBVIEW2_FEATURES));
        assert!(merged.contains("--remote-debugging-port=9224"));
        assert!(merged.contains("--remote-allow-origins=*"));

        let existing = merge_webview2_test_browser_args(
            Some("--disable-features=AlreadyDisabled"),
            "--remote-debugging-port=9333",
        )
        .expect("existing browser args should be preserved");
        assert!(existing.contains("--disable-features=AlreadyDisabled"));
        assert!(!existing.contains(WRY_DEFAULT_DISABLED_WEBVIEW2_FEATURES));
        assert!(existing.contains("--remote-debugging-port=9333"));
    }

    #[test]
    fn headless_recovery_command_uses_confirmed_type_recovery_pairing_path() {
        let source = include_str!("lib.rs");
        let start = source
            .find("cli::CliIntent::PromptEmbeddedBlePairing { expected_name }")
            .expect("headless pairing CLI arm should exist");
        let end = source[start..]
            .find("cli::CliIntent::PromptEmbeddedBlePairingOnly")
            .map(|offset| start + offset)
            .expect("pairing-only CLI arm should exist");
        let recovery_body = &source[start..end];
        let command_index = recovery_body
            .find("send_recording_control_recovery")
            .expect("confirmed recovery CLI must command Listener into pairing first");
        let forced_pairing_index = recovery_body
            .find("prompt_listener_pairing_after_type_recovery")
            .expect("confirmed recovery CLI must use the stale-cache cleanup pairing path");
        let settle_index = recovery_body.find("from_millis(2600)").expect(
            "confirmed recovery CLI must wait for firmware async bond deletion before PairAsync",
        );
        assert!(
            command_index < forced_pairing_index,
            "Type must only force stale Windows cache cleanup after the firmware recovery command succeeds"
        );
        assert!(
            command_index < settle_index && settle_index < forced_pairing_index,
            "Type recovery must let firmware finish async bond deletion before starting Windows PairAsync"
        );

        let only_start = end;
        let only_end = source[only_start..]
            .find("cli::CliIntent::CleanupEmbeddedBlePairing")
            .map(|offset| only_start + offset)
            .expect("cleanup CLI arm should exist");
        let only_body = &source[only_start..only_end];
        assert!(
            !only_body.contains("prompt_listener_pairing_after_type_recovery"),
            "pairing-only scan must stay conservative and must not force stale cache removal"
        );
    }

    #[test]
    fn headless_ota_active_handoff_probe_uses_notify_and_never_starts_transfer() {
        let source = include_str!("lib.rs");
        let start = source
            .find("cli::CliIntent::ProbeListenerOtaV1ActiveHandoff { timeout_ms }")
            .expect("active-link OTA probe CLI arm should exist");
        let end = source[start..]
            .find("cli::CliIntent::ProbeListenerOtaV1Gatt { timeout_ms }")
            .map(|offset| start + offset)
            .expect("plain OTA GATT probe CLI arm should follow the active-link probe");
        let probe = &source[start..end];

        assert!(
            probe.contains("coordinator.auto_select_embedded_ble_input_source_in_background();")
        );
        assert!(probe.contains("wait_for_embedded_ble_listener_ready_before_firmware_ota"));
        assert!(probe.contains("request_listener_ota_v1_active_link(None)"));
        assert!(probe.contains("listener_ota_v1_gatt_probe_after_active_link_hint"));
        assert!(probe.contains("otaPayloadWriteStarted\": false"));
        assert!(
            !probe.contains("transfer_listener_ota_v1") && !probe.contains("prepared.transfer("),
            "active-link diagnostic must stop after readiness and never write OTA payload data"
        );
    }

    #[test]
    fn tray_style_menu_is_windows_only() {
        #[cfg(target_os = "windows")]
        assert!(tray_style_menu_enabled());

        #[cfg(not(target_os = "windows"))]
        assert!(!tray_style_menu_enabled());
    }

    #[test]
    fn window_close_hides_but_explicit_quit_exits() {
        assert!(should_keep_alive_on_exit_request(None, false));
        assert!(!should_keep_alive_on_exit_request(None, true));
        assert!(!should_keep_alive_on_exit_request(Some(0), false));

        assert!(should_hide_main_on_close(false));
        assert!(!should_hide_main_on_close(true));
    }

    #[test]
    fn explicit_quit_requests_coordinator_shutdown_before_exit() {
        let source = include_str!("lib.rs");
        let start = source
            .find("fn request_app_quit")
            .expect("explicit quit helper should exist");
        let end = source[start..]
            .find("fn should_keep_alive_on_exit_request")
            .map(|offset| start + offset)
            .expect("explicit quit helper boundary should exist");
        let body = &source[start..end];
        let quit_flag_index = body
            .find("APP_QUIT_REQUESTED.store(true")
            .expect("explicit quit must mark the app as intentionally quitting");
        let tray_watcher_index = body
            .find("TRAY_MICROPHONE_WATCHER_STOPPING.store(true")
            .expect("explicit quit must stop tray watcher work before exit");
        let shutdown_index = body
            .find("coordinator.request_shutdown();")
            .expect("explicit quit must ask the coordinator to shut down BLE first");
        let exit_index = body.find("app.exit(0);").expect("explicit quit must exit");

        assert!(
            quit_flag_index < shutdown_index,
            "explicit quit must set the quit flag before shutdown so ExitRequested is not kept alive"
        );
        assert!(
            tray_watcher_index < shutdown_index,
            "explicit quit must stop tray watcher refreshes before shutdown"
        );
        assert!(
            shutdown_index < exit_index,
            "Type must send its BLE shutdown signal before exiting the process"
        );
    }

    #[test]
    fn tray_style_menu_lists_builtin_modes_in_expected_order() {
        let entries = tray_polish_mode_menu_entries(PolishMode::Structured);

        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.id.as_str(), entry.label, entry.mode, entry.checked))
                .collect::<Vec<_>>(),
            vec![
                ("style-raw", "原文", PolishMode::Raw, false),
                ("style-light", "轻度润色", PolishMode::Light, false),
                ("style-structured", "清晰结构", PolishMode::Structured, true),
                ("style-formal", "正式表达", PolishMode::Formal, false),
            ]
        );
    }

    #[test]
    fn tray_style_menu_id_parsing_accepts_only_style_items() {
        assert_eq!(
            parse_tray_polish_mode_id("style-raw"),
            Some(PolishMode::Raw)
        );
        assert_eq!(
            parse_tray_polish_mode_id("style-light"),
            Some(PolishMode::Light)
        );
        assert_eq!(
            parse_tray_polish_mode_id("style-structured"),
            Some(PolishMode::Structured)
        );
        assert_eq!(
            parse_tray_polish_mode_id("style-formal"),
            Some(PolishMode::Formal)
        );
        assert_eq!(parse_tray_polish_mode_id("toggle"), None);
        assert_eq!(parse_tray_polish_mode_id("mic-default"), None);
    }

    #[test]
    fn capsule_window_bounds_leave_room_for_windows_shadow() {
        let bounds = capsule_window_bounds(false);
        #[cfg(target_os = "windows")]
        assert_eq!(
            (bounds.width, bounds.height, bounds.bottom_inset),
            (304.0, 84.0, 12.0)
        );

        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            (bounds.width, bounds.height, bounds.bottom_inset),
            (220.0, 110.0, 0.0)
        );
    }

    #[test]
    fn capsule_window_bounds_expand_for_translation_badge() {
        let bounds = capsule_window_bounds(true);
        #[cfg(target_os = "windows")]
        assert_eq!(
            (bounds.width, bounds.height, bounds.bottom_inset),
            (304.0, 118.0, 12.0)
        );

        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            (bounds.width, bounds.height, bounds.bottom_inset),
            (220.0, 110.0, 0.0)
        );
    }

    #[test]
    fn capsule_visual_height_matches_frontend_pill() {
        #[cfg(target_os = "windows")]
        assert_eq!(capsule_visual_height(true), 52.0);

        #[cfg(not(target_os = "windows"))]
        assert_eq!(capsule_visual_height(true), 96.0);
    }

    #[test]
    fn capsule_bottom_center_position_keeps_negative_monitor_origin() {
        let (x, y) = capsule_bottom_center_position(
            -1920,
            0,
            1920,
            1080,
            1.0,
            capsule_window_bounds(false),
            capsule_visual_height(false),
        );

        assert_eq!((x, y), (-1112.0, 936.0));
    }

    #[test]
    fn capsule_bottom_center_position_scales_monitor_origin_with_dpi() {
        let (x, y) = capsule_bottom_center_position(
            2880,
            -1440,
            2880,
            1620,
            1.5,
            capsule_window_bounds(false),
            capsule_visual_height(false),
        );

        assert_eq!((x, y), (2728.0, -24.0));
    }

    #[test]
    fn capsule_bottom_center_physical_position_keeps_desktop_coordinates() {
        let bounds = CapsuleWindowBounds {
            width: 304.0,
            height: 84.0,
            bottom_inset: 12.0,
        };
        let position =
            capsule_bottom_center_physical_position(2880, -1440, 2880, 1620, 1.5, bounds, 52.0);

        assert_eq!(position, (4092, -36));
    }

    #[test]
    fn rect_intersects_any_monitor_detects_off_screen_window() {
        let monitors = [(0, 0, 2560, 1440)];
        // 复现 2026-07-23 线上事故：胶囊被旧 DPI 甩到屏幕下方，完全不可见。
        assert!(!rect_intersects_any_monitor(
            (1395, 1613, 486, 134),
            &monitors
        ));
        assert!(rect_intersects_any_monitor(
            (1128, 1220, 486, 134),
            &monitors
        ));
    }

    #[test]
    fn rect_intersects_any_monitor_handles_negative_origins() {
        let monitors = [(-1920, 0, 1920, 1080), (0, 0, 2560, 1440)];
        assert!(rect_intersects_any_monitor((-100, 500, 200, 80), &monitors));
        assert!(!rect_intersects_any_monitor(
            (-3000, 500, 200, 80),
            &monitors
        ));
        // 仅边缘相贴不算可见。
        assert!(!rect_intersects_any_monitor(
            (2560, 100, 200, 80),
            &monitors
        ));
    }

    #[test]
    fn qa_anchor_uses_normal_capsule_height_source() {
        #[cfg(target_os = "windows")]
        assert_eq!(capsule_height_for_qa(), 52.0);

        #[cfg(not(target_os = "windows"))]
        assert_eq!(capsule_height_for_qa(), 96.0);
    }

    #[test]
    fn oversized_log_rotates_to_single_archive() {
        let dir =
            std::env::temp_dir().join(format!("listener-type-log-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("listener-type.log");
        let archive = dir.join("listener-type.log.1");

        {
            let mut file = std::fs::File::create(&log).unwrap();
            file.set_len(LOG_ROTATE_LIMIT_BYTES + 1).unwrap();
            file.write_all(b"x").unwrap();
        }
        std::fs::write(&archive, b"old").unwrap();

        rotate_log_if_too_large(&log).unwrap();

        assert!(!log.exists());
        assert!(archive.exists());
        assert_eq!(
            std::fs::metadata(&archive).unwrap().len(),
            LOG_SEGMENT_LIMIT_BYTES
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn online_log_rotation_bounds_three_times_cap_and_preserves_retained_order() {
        let dir = std::env::temp_dir().join(format!(
            "listener-type-log-online-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("listener-type.log");
        let archive = dir.join("listener-type.log.1");
        let mut writer = OnlineRotatingLogWriter::open(log.clone()).unwrap();
        let payload = "x".repeat(32 * 1024);
        let records = (3 * LOG_ROTATE_LIMIT_BYTES as usize / payload.len()) + 2;

        for sequence in 0..records {
            writeln!(writer, "{sequence:06}|{payload}").unwrap();
        }
        writer.flush().unwrap();
        drop(writer);

        let active_bytes = std::fs::metadata(&log).unwrap().len();
        let archive_bytes = std::fs::metadata(&archive).unwrap().len();
        assert!(
            active_bytes + archive_bytes
                <= LOG_ROTATE_LIMIT_BYTES + LOG_MAX_RECORD_BYTES as u64 + 64
        );
        let retained = [archive.clone(), log.clone()]
            .into_iter()
            .flat_map(|path| {
                std::fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .map(|line| {
                        line.split_once('|')
                            .unwrap()
                            .0
                            .parse::<usize>()
                            .unwrap()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert!(retained.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(retained.last().copied(), Some(records - 1));
        assert_eq!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("listener-type.log"))
                .count(),
            2
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_log_does_not_rotate() {
        let dir =
            std::env::temp_dir().join(format!("listener-type-log-small-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("listener-type.log");
        let archive = dir.join("listener-type.log.1");
        std::fs::write(&log, b"small").unwrap();

        rotate_log_if_too_large(&log).unwrap();

        assert!(log.exists());
        assert!(!archive.exists());
        assert_eq!(std::fs::read(&log).unwrap(), b"small");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_log_does_not_rotate() {
        let dir =
            std::env::temp_dir().join(format!("listener-type-log-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("listener-type.log");
        let archive = dir.join("listener-type.log.1");

        rotate_log_if_too_large(&log).unwrap();

        assert!(!log.exists());
        assert!(!archive.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unit_tests_do_not_write_runtime_app_log() {
        let path = log_dir_path();
        assert!(
            path.starts_with(std::env::temp_dir()),
            "test logger path must stay in temp, got {path:?}"
        );
        assert!(
            path.to_string_lossy().contains("listener-type-test-logs"),
            "test logger path should be visibly separated from runtime app logs"
        );
    }
}
