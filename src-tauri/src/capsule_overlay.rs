//! Native presentation ownership: a visible capsule stays above ordinary desktop
//! windows without taking the editor's foreground/focus. This has no dictation API.

#[cfg(target_os = "windows")]
mod native {
    use std::sync::{atomic::{AtomicIsize, Ordering}, OnceLock};
    use windows::Win32::{
        Foundation::{HWND, RECT},
        UI::{
            Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
            WindowsAndMessaging::{
                DispatchMessageW, GetAncestor, GetDesktopWindow, GetMessageW, GetWindow, GetWindowLongPtrW,
                GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
                SetWindowPos, EVENT_OBJECT_REORDER, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND,
                GA_ROOT, GWL_EXSTYLE, GW_HWNDPREV, HWND_TOPMOST, MSG,
                SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
                SWP_NOSIZE, WINEVENT_OUTOFCONTEXT, WS_EX_TOPMOST,
            },
        },
    };

    static TARGET: AtomicIsize = AtomicIsize::new(0);
    static STARTED: OnceLock<()> = OnceLock::new();
    #[cfg(test)]
    static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn overlaps(a: RECT, b: RECT) -> bool {
        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }

    /// Never show a hidden host or promote another process's recycled HWND.
    /// Inspect actual Z order, not the presence of a new ASR/preview payload.
    pub(super) fn repair_if_covered(hwnd: HWND) -> windows::core::Result<bool> {
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != std::process::id() || !IsWindowVisible(hwnd).as_bool()
                || IsIconic(hwnd).as_bool() {
                return Ok(false);
            }
            let demoted = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 == 0;
            let mut bounds = RECT::default();
            GetWindowRect(hwnd, &mut bounds)?;
            let mut covered = false;
            let mut above = GetWindow(hwnd, GW_HWNDPREV).unwrap_or_default();
            // Bound inspection even if a foreign process continuously changes Z order.
            for _ in 0..256 {
                if above.0.is_null() { break; }
                if IsWindowVisible(above).as_bool() && !IsIconic(above).as_bool() {
                    let mut other = RECT::default();
                    if GetWindowRect(above, &mut other).is_ok() && overlaps(bounds, other) {
                        covered = true;
                        break;
                    }
                }
                above = GetWindow(above, GW_HWNDPREV).unwrap_or_default();
            }
            if !demoted && !covered { return Ok(false); }
            // No activation, movement, resize, owner reordering or SHOW flag. Async
            // avoids blocking this observer on a busy window's input queue.
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER
                    | SWP_ASYNCWINDOWPOS)?;
            Ok(true)
        }
    }

    unsafe extern "system" fn on_window_event(
        _hook: HWINEVENTHOOK, event: u32, event_hwnd: HWND,
        object: i32, child: i32, _thread: u32, _time: u32,
    ) {
        let hwnd = HWND(TARGET.load(Ordering::Acquire) as *mut _);
        if hwnd.0.is_null() || event_hwnd == hwnd { return; }
        // Ignore text/caret/WebView accessibility updates. Desktop REORDER is
        // also relevant; window events otherwise must describe a top-level host.
        let desktop_reorder = event == EVENT_OBJECT_REORDER && object == -4 && child == 0
            && event_hwnd == GetDesktopWindow();
        if event != EVENT_SYSTEM_FOREGROUND && !desktop_reorder
            && (object != 0 || child != 0 || event_hwnd.0.is_null()
                || GetAncestor(event_hwnd, GA_ROOT) != event_hwnd) { return; }
        if let Err(err) = repair_if_covered(hwnd) {
            log::debug!("[capsule] native layer repair failed: {err}");
        }
    }

    pub(super) fn observe(hwnd: HWND) {
        TARGET.store(hwnd.0 as isize, Ordering::Release);
        STARTED.get_or_init(|| {
            // WinEvent OUTOFCONTEXT requires a message loop on its registering
            // thread. Hooks live on that thread for the application's lifetime;
            // no timers, polling, editor hooks or per-audio-packet work.
            if let Err(err) = std::thread::Builder::new().name("capsule-layer".into()).spawn(|| unsafe {
                let events = [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_SHOW, EVENT_OBJECT_REORDER];
                let hooks: Vec<_> = events.into_iter().map(|event| {
                    SetWinEventHook(event, event, None, Some(on_window_event), 0, 0,
                        WINEVENT_OUTOFCONTEXT)
                }).collect();
                if hooks.iter().any(|hook| hook.0.is_null()) {
                    log::warn!("[capsule] native layer observer could not register all hooks");
                    for hook in hooks { if !hook.0.is_null() { let _ = UnhookWinEvent(hook); } }
                    return;
                }
                #[cfg(test)]
                READY.store(true, Ordering::Release);
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                    DispatchMessageW(&message);
                }
                for hook in hooks { let _ = UnhookWinEvent(hook); }
            }) {
                log::warn!("[capsule] native layer observer thread failed: {err}");
            }
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::{core::w, Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetForegroundWindow, PeekMessageW, ShowWindow,
            HWND_NOTOPMOST, PM_REMOVE, SW_HIDE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_POPUP,
        }};

        // Actual native windows, offscreen and non-activating. No user editor,
        // device, clipboard, Tauri instance or dictation session is touched.
        #[test]
        fn capsule_layer_repairs_overlap_and_demotion_without_focus_or_visibility_changes() {
            unsafe {
                let foreground = GetForegroundWindow();
                let make = || CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    w!("STATIC"), w!("Listener capsule layer regression"), WS_POPUP,
                    -30000, -30000, 80, 40, None, None, None, None).unwrap();
                struct Window(HWND);
                impl Drop for Window { fn drop(&mut self) { unsafe { let _ = DestroyWindow(self.0); } } }
                let capsule = Window(make());
                let blocker = Window(make());
                let pump_until = |condition: &dyn Fn() -> bool| {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    while !condition() && std::time::Instant::now() < deadline {
                        let mut msg = MSG::default();
                        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            DispatchMessageW(&msg);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    assert!(condition(), "native layer event did not repair the visible host");
                };
                assert!(!repair_if_covered(capsule.0).unwrap());
                let _ = ShowWindow(capsule.0, SW_SHOWNOACTIVATE);
                assert!(repair_if_covered(capsule.0).unwrap());
                assert_ne!(GetWindowLongPtrW(capsule.0, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0, 0);
                let _ = ShowWindow(blocker.0, SW_SHOWNOACTIVATE);
                SetWindowPos(blocker.0, HWND_TOPMOST, 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).unwrap();
                assert_eq!(GetWindow(capsule.0, GW_HWNDPREV).unwrap(), blocker.0);
                assert!(repair_if_covered(capsule.0).unwrap());
                assert_ne!(GetWindow(capsule.0, GW_HWNDPREV).unwrap_or_default(), blocker.0);
                let _ = ShowWindow(blocker.0, SW_HIDE);
                assert!(!repair_if_covered(capsule.0).unwrap());
                // The actual registered WinEvent observer must work even with no
                // preview/PCM events. Pump only this offscreen window's thread.
                observe(capsule.0);
                pump_until(&|| READY.load(Ordering::Acquire));
                let _ = ShowWindow(blocker.0, SW_SHOWNOACTIVATE);
                SetWindowPos(blocker.0, HWND_TOPMOST, 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).unwrap();
                pump_until(&|| GetWindow(capsule.0, GW_HWNDPREV).unwrap_or_default() != blocker.0);
                // Once both hosts are already visible, promotion emits a desktop
                // client REORDER (OBJID_CLIENT=-4), not another SHOW/foreground.
                SetWindowPos(blocker.0, HWND_TOPMOST, 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).unwrap();
                pump_until(&|| GetWindow(capsule.0, GW_HWNDPREV).unwrap_or_default() != blocker.0);
                // A demotion followed by an ordinary window change also repairs
                // the actual extended style, rather than only a cached policy.
                SetWindowPos(capsule.0, HWND_NOTOPMOST, 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE).unwrap();
                let _ = ShowWindow(blocker.0, SW_HIDE);
                let _ = ShowWindow(blocker.0, SW_SHOWNOACTIVATE);
                pump_until(&|| GetWindowLongPtrW(capsule.0, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0);
                let _ = ShowWindow(capsule.0, SW_HIDE);
                assert!(!repair_if_covered(capsule.0).unwrap());
                assert!(!IsWindowVisible(capsule.0).as_bool());
                assert_eq!(GetForegroundWindow(), foreground);
                TARGET.store(0, Ordering::Release);
            }
        }
    }
}

pub(crate) fn observe<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(raw) = handle.as_raw() {
                native::observe(windows::Win32::Foundation::HWND(raw.hwnd.get() as *mut _));
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}
