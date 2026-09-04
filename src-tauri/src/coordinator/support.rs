//! Shared coordinator helpers: capsule UI, focus capture, ASR bridge.
//! Kept as a submodule so the main coordinator surface stays smaller.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{async_runtime, AppHandle, Emitter, Manager};

use crate::coordinator_state::{
    apply_dictation_event, publishable_dictation_snapshot, startup_race_status, DictationEvent,
    DictationSnapshot, DictationTransition, DictationUiState, SessionId, SessionPhase,
    StartupRaceStatus,
};
use crate::types::{CapsulePayload, CapsuleState};
#[cfg(target_os = "windows")]
use crate::windows_ime_ipc::ImeSubmitTarget;

use super::qa::QaPhase;
use super::recording_gate::{self, RecordIntent};
use super::{extra_asr_hotword_phrases, Inner};
pub(super) fn enabled_phrases(inner: &Arc<Inner>) -> Vec<String> {
    let mut phrases: Vec<String> = inner
        .vocab
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.enabled)
        .map(|e| e.phrase)
        .collect();
    for phrase in extra_asr_hotword_phrases() {
        if !phrases.iter().any(|existing| existing == &phrase) {
            phrases.push(phrase);
        }
    }
    phrases
}

/// 终止态（Done / Cancelled / Error）后延迟 N ms 把胶囊改回 Idle，让浮窗自动消失。
/// 硬件 BLE 听写日常成功路径：字已上屏，只留极短完成反馈。
/// 1050ms 会让 owner 体感「结尾拖」——收尾合同看的是停说→上屏，不是胶囊挂多久。
pub(super) const CAPSULE_SUCCESS_HIDE_DELAY_MS: u64 = 380;
pub(super) const CAPSULE_AUTO_HIDE_DELAY_MS: u64 = 0;
pub(super) const CAPSULE_ACTIONABLE_ERROR_HIDE_DELAY_MS: u64 = 6_000;
pub(super) const CAPSULE_EMPTY_TRANSCRIPT_HIDE_DELAY_MS: u64 = 1_500;
pub(super) const CAPSULE_STREAM_ERROR_HIDE_DELAY_MS: u64 = 6_000;
pub(super) const CAPSULE_RECORDING_WINDOW_KEEPALIVE_MS: u64 = 1_000;
pub(super) const CAPSULE_RECORDING_FRONTEND_TICK_MS: u64 = 50;

/// Coordinator 全局超时保护：防止 ASR await_final_result() 永远挂起。
/// 设置为 15 秒（比 ASR 的 12 秒 FINAL_RESULT_TIMEOUT 稍长），
/// 只在 ASR 超时机制失效时作为最后的防线触发。
pub(super) const COORDINATOR_GLOBAL_TIMEOUT_SECS: u64 = 15;

#[cfg(target_os = "windows")]
pub(super) fn foundry_audio_transcribe_timeout_duration() -> std::time::Duration {
    std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
}

/// 本地 Qwen3-ASR 的动态转写超时。固定 15 秒在长录音（≥ 30s）+ 慢机器
/// （RTF ≈ 0.3–0.5）上必然超时把整段内容丢掉。改用 max(15, ceil(audio_s
/// × 0.6) + 10)：基础保留 15s 兜住短录音；长录音按音频长度的 0.6 倍 +
/// 10s 余量，覆盖 RTF ≤ 0.5 的机器。
pub(super) fn local_qwen_transcribe_timeout(audio_secs: f64) -> std::time::Duration {
    let secs = ((audio_secs * 0.6).ceil() as u64)
        .saturating_add(10)
        .max(COORDINATOR_GLOBAL_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

/// 检查 begin_session 的 await 间隙是否被 cancel_session 打断。
/// 必须在持有 state lock 的瞬间读，结果一拿就过期，所以用 helper 名字提醒只在
/// 「准备做下一步副作用前」用。
pub(super) fn startup_race_status_for_starting(
    inner: &Arc<Inner>,
    captured_session_id: SessionId,
) -> StartupRaceStatus {
    let state = inner.state.lock();
    startup_race_status(&state, captured_session_id)
}

pub(super) fn transition_pipeline_error_if_session_matches(
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let mut state = inner.state.lock();
    let _ = apply_dictation_event(&mut state, DictationEvent::PipelineError { session_id });
}

pub(super) fn listening_session_has_no_current_asr(inner: &Arc<Inner>) -> bool {
    let (phase, session_id) = {
        let state = inner.state.lock();
        (state.phase, state.session_id)
    };
    phase == SessionPhase::Listening
        && inner
            .asr
            .lock()
            .as_ref()
            .map(|resource| resource.session_id != session_id)
            .unwrap_or(true)
}

pub(super) fn schedule_capsule_idle(
    inner: &Arc<Inner>,
    delay_ms: u64,
    session_id: Option<SessionId>,
) {
    let inner_clone = Arc::clone(inner);
    async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        // 必须 dictation **和** QA 同时空闲才能隐藏胶囊。否则旧 dictation Done timer
        // 的尾巴会在新 QA 录音/思考中把胶囊意外收掉（issue #118 v2 复现）。
        let dictation_idle = inner_clone.state.lock().phase == SessionPhase::Idle;
        let qa_idle = inner_clone.qa_state.lock().phase == QaPhase::Idle;
        if dictation_idle && qa_idle {
            emit_capsule_with_session(
                &inner_clone,
                session_id,
                CapsuleState::Idle,
                0.0,
                0,
                None,
                None,
            );
        }
    });
}

#[cfg(target_os = "windows")]
pub(super) fn capture_focus_target() -> Option<usize> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        None
    } else {
        Some(foreground.0 as usize)
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn capture_focus_target() -> Option<usize> {
    None
}

/// 捕获用户开始 dictation 时的前台 app 标签（"localizedName (bundle.id)"），用作 LLM
/// polish/translate 的上下文前提，让模型按 app 调风格。详见 issue #116。
///
/// macOS 走 NSWorkspace.frontmostApplication（公开 API，无需额外权限）；
/// Windows 复用前台 HWND 拿窗口标题；Linux/其他平台返回 None。
#[cfg(target_os = "macos")]
pub(super) fn capture_frontmost_app() -> Option<String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    unsafe {
        let cls = AnyClass::get("NSWorkspace")?;
        let workspace: *mut AnyObject = msg_send![cls, sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let name_obj: *mut AnyObject = msg_send![app, localizedName];
        let bundle_obj: *mut AnyObject = msg_send![app, bundleIdentifier];
        let name = nsstring_to_string(name_obj);
        let bundle = nsstring_to_string(bundle_obj);
        match (name, bundle) {
            (Some(n), Some(b)) => Some(format!("{n} ({b})")),
            (Some(n), None) => Some(n),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn nsstring_to_string(ns_string: *mut objc2::runtime::AnyObject) -> Option<String> {
    use objc2::msg_send;
    if ns_string.is_null() {
        return None;
    }
    let utf8: *const std::os::raw::c_char = unsafe { msg_send![ns_string, UTF8String] };
    if utf8.is_null() {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(utf8) };
    let s = cstr.to_string_lossy().into_owned();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(target_os = "windows")]
pub(super) fn capture_frontmost_app() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let copied = GetWindowTextW(hwnd, &mut buf);
        if copied <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..copied as usize]);
        if title.is_empty() {
            None
        } else {
            Some(title)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn capture_frontmost_app() -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
pub(super) fn restore_focus_target_if_possible(target: Option<usize>) -> bool {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
        SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let Some(raw_target) = target else {
        log::warn!("[coord] no original Windows insertion target captured");
        return false;
    };
    let hwnd = HWND(raw_target as *mut c_void);
    if hwnd.0.is_null() {
        return false;
    }
    if !unsafe { IsWindow(hwnd).as_bool() } {
        log::warn!("[coord] original Windows insertion target is no longer a valid window");
        return false;
    }

    let foreground = unsafe { GetForegroundWindow() };
    if foreground == hwnd {
        return true;
    }

    if unsafe { IsIconic(hwnd).as_bool() } {
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }
    let current_thread_id = unsafe { GetCurrentThreadId() };
    let mut foreground_process_id = 0;
    let foreground_thread_id =
        unsafe { GetWindowThreadProcessId(foreground, Some(&mut foreground_process_id)) };
    let mut target_process_id = 0;
    let target_thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut target_process_id)) };
    let attach_foreground = foreground_thread_id != 0 && foreground_thread_id != current_thread_id;
    let attach_target = target_thread_id != 0 && target_thread_id != current_thread_id;
    if attach_foreground {
        let _ = unsafe { AttachThreadInput(current_thread_id, foreground_thread_id, true) };
    }
    if attach_target {
        let _ = unsafe { AttachThreadInput(current_thread_id, target_thread_id, true) };
    }
    let _ = unsafe { BringWindowToTop(hwnd) };
    let _ = unsafe { SetForegroundWindow(hwnd) };
    let _ = unsafe { SetFocus(hwnd) };
    std::thread::sleep(std::time::Duration::from_millis(90));
    if attach_target {
        let _ = unsafe { AttachThreadInput(current_thread_id, target_thread_id, false) };
    }
    if attach_foreground {
        let _ = unsafe { AttachThreadInput(current_thread_id, foreground_thread_id, false) };
    }

    let foreground = unsafe { GetForegroundWindow() };
    if foreground != hwnd {
        log::warn!(
            "[coord] failed to restore original Windows insertion target before paste target_thread={target_thread_id} foreground_thread={foreground_thread_id}"
        );
        return false;
    }
    true
}

#[cfg(not(target_os = "windows"))]
pub(super) fn restore_focus_target_if_possible(_target: Option<usize>) -> bool {
    true
}

#[cfg(target_os = "windows")]
pub(super) fn windows_hwnd_is_present(hwnd: windows::Win32::Foundation::HWND) -> bool {
    hwnd != windows::Win32::Foundation::HWND::default()
}

#[cfg(target_os = "windows")]
pub(super) fn capture_ime_submit_target() -> Option<ImeSubmitTarget> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };

    let foreground = unsafe { GetForegroundWindow() };
    if !windows_hwnd_is_present(foreground) {
        return None;
    }

    let mut foreground_process_id = 0;
    let foreground_thread_id =
        unsafe { GetWindowThreadProcessId(foreground, Some(&mut foreground_process_id)) };
    if foreground_thread_id == 0 {
        return None;
    }

    let mut gui_info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let target_window = if unsafe { GetGUIThreadInfo(foreground_thread_id, &mut gui_info).is_ok() }
        && windows_hwnd_is_present(gui_info.hwndFocus)
    {
        gui_info.hwndFocus
    } else {
        foreground
    };

    let mut process_id = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(target_window, Some(&mut process_id)) };
    if process_id == 0 || thread_id == 0 {
        return None;
    }

    Some(ImeSubmitTarget {
        process_id,
        thread_id,
    })
}

#[cfg(target_os = "windows")]
pub(super) fn show_capsule_window_no_activate<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
    };

    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);

    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

// macOS / Linux 上不走 no-activate 路径：胶囊由 emit_capsule 的 fallback
// `window.show()` 直接显示，再用 restore_main_window_key_if_active 把焦点还给
// 主窗口。这是 1.2.11 的实现 — 单独走 orderFrontRegardless 会让胶囊在 webview
// 未完整初始化时偶发不可见。
#[cfg(not(target_os = "windows"))]
pub(super) fn show_capsule_window_no_activate<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    _window: &tauri::WebviewWindow<R>,
) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(super) fn hide_capsule_window_if_present() {
    use std::iter::once;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, ShowWindow, HWND_NOTOPMOST, SWP_HIDEWINDOW, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    };

    let title: Vec<u16> = "Listener Type Capsule"
        .encode_utf16()
        .chain(once(0))
        .collect();
    let hwnd = match unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) } {
        Ok(hwnd) => hwnd,
        Err(_) => return,
    };
    if hwnd == HWND::default() || hwnd.0.is_null() {
        return;
    }

    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_HIDEWINDOW,
        )
    };
}

#[cfg(not(target_os = "windows"))]
pub(super) fn hide_capsule_window_if_present() {}

pub(super) fn emit_capsule(
    inner: &Arc<Inner>,
    state: CapsuleState,
    level: f32,
    elapsed_ms: u64,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    emit_capsule_with_session(
        inner,
        None,
        state,
        level,
        elapsed_ms,
        message,
        inserted_chars,
    );
}

pub(super) fn emit_capsule_for_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
    state: CapsuleState,
    level: f32,
    elapsed_ms: u64,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    emit_capsule_with_session(
        inner,
        Some(session_id),
        state,
        level,
        elapsed_ms,
        message,
        inserted_chars,
    );
}

pub(super) fn capsule_state_from_dictation(ui_state: DictationUiState) -> CapsuleState {
    match ui_state {
        DictationUiState::Recording => CapsuleState::Recording,
        DictationUiState::Transcribing => CapsuleState::Transcribing,
        DictationUiState::Polishing => CapsuleState::Polishing,
        DictationUiState::Done => CapsuleState::Done,
        DictationUiState::Cancelled => CapsuleState::Cancelled,
        DictationUiState::Error => CapsuleState::Error,
        DictationUiState::Idle => CapsuleState::Idle,
    }
}

pub(super) fn emit_dictation_snapshot(
    inner: &Arc<Inner>,
    snapshot: DictationSnapshot,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    emit_capsule_for_session(
        inner,
        snapshot.session_id,
        capsule_state_from_dictation(snapshot.state),
        level,
        snapshot.elapsed_ms,
        message,
        inserted_chars,
    );
}

pub(super) fn publish_dictation_transition(
    inner: &Arc<Inner>,
    transition: DictationTransition,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    if let Some(snapshot) = transition.snapshot() {
        emit_dictation_snapshot(inner, snapshot, level, message, inserted_chars);
        true
    } else {
        false
    }
}

pub(super) fn publish_dictation_capsule(
    inner: &Arc<Inner>,
    session_id: SessionId,
    ui_state: DictationUiState,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    let snapshot = {
        let state = inner.state.lock();
        publishable_dictation_snapshot(&state, session_id, ui_state).ok()
    };
    let Some(snapshot) = snapshot else {
        return false;
    };
    emit_dictation_snapshot(inner, snapshot, level, message, inserted_chars);
    true
}

pub(super) fn apply_capsule_window_request<R: tauri::Runtime>(
    inner: &Arc<Inner>,
    app: &AppHandle<R>,
    seq: u64,
    session_id_for_log: &str,
    state: CapsuleState,
    elapsed_ms: u64,
    translation: bool,
    show_capsule: bool,
    visible: bool,
) {
    let Some(window) = app.get_webview_window("capsule") else {
        log::warn!("[capsule] emit requested but capsule window is missing");
        crate::timeline::mark(
            "backend.capsule",
            "missing_window",
            format!(
                "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms}"
            ),
        );
        return;
    };
    crate::prepare_capsule_window_for_overlay(&window);
    maybe_position_capsule_bottom_center(inner, app, &window, translation);
    if show_capsule && visible {
        let shown_no_activate = show_capsule_window_no_activate(app, &window);
        crate::timeline::mark(
            "backend.capsule",
            "show_request",
            format!(
                "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms} shown_no_activate={shown_no_activate}"
            ),
        );
        log::info!("[capsule] show request state={state:?} shown_no_activate={shown_no_activate}");
        if !shown_no_activate {
            log::warn!("[capsule] no-activate show failed; falling back to window.show()");
            match window.show() {
                Ok(()) => crate::timeline::mark(
                    "backend.capsule",
                    "show_fallback",
                    format!(
                        "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms}"
                    ),
                ),
                Err(err) => {
                    log::warn!("[capsule] show fallback failed: {err}");
                    crate::timeline::mark(
                        "backend.capsule",
                        "show_fallback_failed",
                        format!(
                            "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms} err={err}"
                        ),
                    );
                }
            }
        }
        #[cfg(target_os = "macos")]
        crate::restore_main_window_key_if_active(app);
    } else {
        crate::timeline::mark(
            "backend.capsule",
            "hide_request",
            format!(
                "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms} show_capsule={show_capsule} visible={visible}"
            ),
        );
        log::info!(
            "[capsule] hide request state={state:?} show_capsule={show_capsule} visible={visible}"
        );
        hide_capsule_window_if_present();
        let _ = window.hide();
    }
}

pub(super) fn emit_capsule_with_session(
    inner: &Arc<Inner>,
    event_session_id: Option<SessionId>,
    state: CapsuleState,
    level: f32,
    elapsed_ms: u64,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    // Single door: recording-related capsules during OTA (and future policies).
    if !recording_gate::try_admit_arc(
        inner,
        RecordIntent::ShowCapsule { state },
        &format!("emit_capsule session={event_session_id:?} state={state:?}"),
    ) {
        return;
    }
    let app_opt = inner.app.lock().clone();
    let Some(app) = app_opt else { return };
    let session_id = event_session_id.map(|id| id.to_string());
    let translation = inner.translation_modifier_seen.load(Ordering::SeqCst);
    let visible = !matches!(state, CapsuleState::Idle);
    let show_capsule = inner.prefs.get().show_capsule
        && std::env::var("LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW")
            .ok()
            .as_deref()
            != Some("1");
    let now = Instant::now();
    let should_emit_frontend = {
        let mut throttle = inner.capsule_ui_throttle.lock();
        throttle.should_emit_frontend(
            CapsuleFrontendRequest {
                session_id: session_id.clone(),
                state,
                visible,
                translation,
                show_capsule,
                message: message.clone(),
                inserted_chars,
            },
            now,
        )
    };
    if !should_emit_frontend {
        return;
    }

    let seq = inner.capsule_sequence.fetch_add(1, Ordering::SeqCst) + 1;
    let payload = CapsulePayload {
        seq,
        session_id,
        state,
        level,
        elapsed_ms,
        message,
        inserted_chars,
        translation,
    };

    let should_trace_emit = {
        let mut throttle = inner.capsule_ui_throttle.lock();
        throttle.should_record_backend_emit(&payload, now)
    };
    if should_trace_emit {
        crate::capsule_log::record_backend_emit(&payload, visible, show_capsule);
    }
    let session_id_for_log = payload
        .session_id
        .clone()
        .unwrap_or_else(|| "-".to_string());
    if should_trace_emit {
        crate::timeline::mark(
            "backend.capsule",
            "emit_request",
            format!(
                "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms} level={level:.3} visible={visible} has_message={} message_chars={}",
                payload.message.is_some(),
                payload
                    .message
                    .as_deref()
                    .map(|value| value.chars().count())
                    .unwrap_or(0)
            ),
        );
    }

    let run_window_ops = {
        let mut throttle = inner.capsule_ui_throttle.lock();
        throttle.should_run_window_ops(
            CapsuleWindowRequest {
                session_id: payload.session_id.clone(),
                state,
                visible,
                translation,
                show_capsule,
            },
            now,
        )
    };

    if run_window_ops {
        #[cfg(target_os = "windows")]
        apply_capsule_window_request(
            inner,
            &app,
            seq,
            &session_id_for_log,
            state,
            elapsed_ms,
            translation,
            show_capsule,
            visible,
        );

        #[cfg(not(target_os = "windows"))]
        {
            let inner_for_main = Arc::clone(inner);
            let app_for_main = app.clone();
            let session_id_for_main = session_id_for_log.clone();
            if let Err(err) = app.run_on_main_thread(move || {
                apply_capsule_window_request(
                    &inner_for_main,
                    &app_for_main,
                    seq,
                    &session_id_for_main,
                    state,
                    elapsed_ms,
                    translation,
                    show_capsule,
                    visible,
                );
            }) {
                log::warn!("[capsule] main-thread window request dispatch failed: {err}");
                crate::timeline::mark(
                "backend.capsule",
                "main_thread_dispatch_failed",
                format!(
                    "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms} err={err}"
                ),
            );
            }
        }
    }

    if should_trace_emit {
        crate::timeline::mark(
            "backend.capsule",
            "emit_to_frontend",
            format!(
                "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms}"
            ),
        );
    }
    let _ = app.emit_to("capsule", "capsule:state", payload);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapsuleWindowRequest {
    pub(super) session_id: Option<String>,
    pub(super) state: CapsuleState,
    pub(super) visible: bool,
    pub(super) translation: bool,
    pub(super) show_capsule: bool,
}

impl CapsuleWindowRequest {
    fn visible_recording(&self) -> bool {
        self.visible && self.show_capsule && matches!(self.state, CapsuleState::Recording)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapsuleFrontendRequest {
    pub(super) session_id: Option<String>,
    pub(super) state: CapsuleState,
    pub(super) visible: bool,
    pub(super) translation: bool,
    pub(super) show_capsule: bool,
    pub(super) message: Option<String>,
    pub(super) inserted_chars: Option<u32>,
}

impl CapsuleFrontendRequest {
    fn visible_recording_level_tick(&self) -> bool {
        self.visible
            && self.show_capsule
            && matches!(self.state, CapsuleState::Recording)
            && self.message.is_none()
            && self.inserted_chars.is_none()
    }
}

#[derive(Debug, Default)]
pub(super) struct CapsuleUiThrottleState {
    last_request: Option<CapsuleWindowRequest>,
    last_recording_keepalive_at: Option<Instant>,
    last_frontend_request: Option<CapsuleFrontendRequest>,
    last_frontend_emit_at: Option<Instant>,
    last_recording_diagnostic_session_id: Option<String>,
    last_recording_diagnostic_at: Option<Instant>,
}

impl CapsuleUiThrottleState {
    pub(super) fn should_emit_frontend(
        &mut self,
        request: CapsuleFrontendRequest,
        now: Instant,
    ) -> bool {
        if self.last_frontend_request.as_ref() != Some(&request) {
            self.last_frontend_request = Some(request);
            self.last_frontend_emit_at = Some(now);
            return true;
        }

        if request.visible_recording_level_tick() {
            let due = self
                .last_frontend_emit_at
                .map(|last| {
                    now.duration_since(last)
                        >= Duration::from_millis(CAPSULE_RECORDING_FRONTEND_TICK_MS)
                })
                .unwrap_or(true);
            if due {
                self.last_frontend_emit_at = Some(now);
                return true;
            }
        }

        false
    }

    pub(super) fn should_record_backend_emit(
        &mut self,
        payload: &CapsulePayload,
        now: Instant,
    ) -> bool {
        let plain_recording_level_tick = matches!(payload.state, CapsuleState::Recording)
            && payload.message.is_none()
            && payload.inserted_chars.is_none();
        if !plain_recording_level_tick {
            self.last_recording_diagnostic_session_id = None;
            self.last_recording_diagnostic_at = None;
            return true;
        }

        if self.last_recording_diagnostic_session_id.as_deref() != payload.session_id.as_deref() {
            self.last_recording_diagnostic_session_id = payload.session_id.clone();
            self.last_recording_diagnostic_at = Some(now);
            return true;
        }

        let due = self
            .last_recording_diagnostic_at
            .map(|last| now.duration_since(last) >= Duration::from_secs(1))
            .unwrap_or(true);
        if due {
            self.last_recording_diagnostic_at = Some(now);
        }
        due
    }

    pub(super) fn should_run_window_ops(
        &mut self,
        request: CapsuleWindowRequest,
        now: Instant,
    ) -> bool {
        let visible_recording = request.visible_recording();
        if self.last_request.as_ref() != Some(&request) {
            self.last_request = Some(request);
            self.last_recording_keepalive_at = visible_recording.then_some(now);
            return true;
        }

        if visible_recording {
            let due = self
                .last_recording_keepalive_at
                .map(|last| {
                    now.duration_since(last)
                        >= Duration::from_millis(CAPSULE_RECORDING_WINDOW_KEEPALIVE_MS)
                })
                .unwrap_or(true);
            if due {
                self.last_recording_keepalive_at = Some(now);
                return true;
            }
        }

        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CapsuleLayoutState {
    pub(super) translation_active: bool,
    pub(super) monitor_x: i32,
    pub(super) monitor_y: i32,
    pub(super) monitor_width: u32,
    pub(super) monitor_height: u32,
    pub(super) scale_bits: u64,
}

pub(super) fn maybe_position_capsule_bottom_center<R: tauri::Runtime>(
    inner: &Arc<Inner>,
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    translation_active: bool,
) {
    let Some(monitor) = crate::capsule_target_monitor(app, window) else {
        return;
    };
    let next = CapsuleLayoutState {
        translation_active,
        monitor_x: monitor.position().x,
        monitor_y: monitor.position().y,
        monitor_width: monitor.size().width,
        monitor_height: monitor.size().height,
        scale_bits: monitor.scale_factor().to_bits(),
    };
    // 窗口被甩到所有显示器之外时（DPI/拓扑变化后常见），缓存状态不再可信，
    // 必须强制重定位；否则胶囊一旦出屏就永远回不来。
    let off_screen = crate::capsule_window_off_all_monitors(app, window);
    {
        let last = inner.capsule_layout.lock();
        if !off_screen && last.as_ref() == Some(&next) {
            return;
        }
    }
    if crate::position_capsule_bottom_center(app, window, translation_active).is_ok() {
        let mut last = inner.capsule_layout.lock();
        *last = Some(next);
    }
}

// ─────────────────────────── audio bridge ───────────────────────────

pub(super) struct DeferredAsrBridge {
    state: Mutex<DeferredAsrState>,
}

pub(super) struct DeferredAsrState {
    target: Option<Arc<dyn crate::asr::AudioConsumer>>,
    pending_audio: Vec<u8>,
    attaching: bool,
}

impl DeferredAsrBridge {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(DeferredAsrState {
                target: None,
                pending_audio: Vec::new(),
                attaching: false,
            }),
        }
    }

    pub(super) fn attach(&self, target: Arc<dyn crate::asr::AudioConsumer>) -> usize {
        let mut flushed_bytes = 0;
        {
            let mut state = self.state.lock();
            state.attaching = true;
        }

        loop {
            let pending = {
                let mut state = self.state.lock();
                if state.pending_audio.is_empty() {
                    state.target = Some(Arc::clone(&target));
                    state.attaching = false;
                    return flushed_bytes;
                }
                std::mem::take(&mut state.pending_audio)
            };
            flushed_bytes += pending.len();
            target.consume_pcm_chunk(&pending);
        }
    }
}

impl crate::recorder::AudioConsumer for DeferredAsrBridge {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        let target = {
            let mut state = self.state.lock();
            if state.attaching {
                state.pending_audio.extend_from_slice(pcm);
                return;
            }
            if let Some(target) = state.target.as_ref() {
                Some(Arc::clone(target))
            } else {
                state.pending_audio.extend_from_slice(pcm);
                None
            }
        };

        if let Some(target) = target {
            target.consume_pcm_chunk(pcm);
        }
    }
}
