//! Cross-platform text insertion at the current cursor position.
//!
//! Strategy:
//! 1. Always copy the text to the clipboard first (so the user can manually
//!    `Cmd+V` / `Ctrl+V` if simulation fails).
//! 2. On macOS, simulate Cmd+V via raw `CGEventPost` FFI — **不能用 enigo**：
//!    enigo 在 macOS 上的 keycode_to_string 会同步调 `TSMGetInputSourceProperty`，
//!    macOS 14+ 强制断言主线程，从 tokio worker 线程调就 SIGTRAP（已踩坑）。
//!    Swift 原版 `TextInserter.simulatePaste()` 用的就是 CGEventCreateKeyboardEvent
//!    → CGEventPost，跟我们这里完全同源。
//! 3. 其他平台 (Windows/Linux) 仍用 enigo。

#[cfg(not(target_os = "macos"))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(target_os = "macos"))]
use std::time::Duration;

#[cfg(not(target_os = "macos"))]
use once_cell::sync::Lazy;
#[cfg(not(target_os = "macos"))]
use parking_lot::Mutex;

use crate::types::{InsertStatus, PasteShortcut};

#[cfg(target_os = "windows")]
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(750);

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(750);

pub struct TextInserter;

impl TextInserter {
    pub fn new() -> Self {
        Self
    }

    /// Insert `text` at the current cursor position.
    /// `restore_clipboard_after_paste` 仅在 Windows/Linux 路径下决定 paste 之后是否恢复
    /// 用户原剪贴板。macOS 走 AX 直写，参数被忽略。详见 issue #111。
    /// `paste_shortcut` 决定 Windows/Linux 上模拟按下的粘贴快捷键。详见 issue #360：
    /// kitty/alacritty 等终端只接受 Ctrl+Shift+V，硬编码 Ctrl+V 会被吞掉。
    #[cfg(not(target_os = "macos"))]
    pub fn insert(
        &self,
        text: &str,
        restore_clipboard_after_paste: bool,
        paste_shortcut: PasteShortcut,
    ) -> InsertStatus {
        if text.is_empty() {
            return InsertStatus::CopiedFallback;
        }
        insert_with_clipboard_restore(text, restore_clipboard_after_paste, paste_shortcut)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn insert_via_clipboard_fallback(
        &self,
        text: &str,
        restore_clipboard_after_paste: bool,
        paste_shortcut: PasteShortcut,
    ) -> InsertStatus {
        self.insert(text, restore_clipboard_after_paste, paste_shortcut)
    }

    #[cfg(target_os = "windows")]
    pub fn insert_via_unicode_keystrokes(&self, text: &str) -> InsertStatus {
        if text.is_empty() {
            return InsertStatus::CopiedFallback;
        }
        match windows_unicode::send_text(text) {
            Ok(()) => InsertStatus::Inserted,
            Err(err) => {
                log::warn!("[insertion] Unicode SendInput failed: {err}");
                InsertStatus::CopiedFallback
            }
        }
    }

    /// IME-safe final insert: temporarily arm en-US layout, then KEYEVENTF_UNICODE.
    /// Default product MSI does not register the optional TSF DLL; this is the
    /// primary "true insert" path that avoids clipboard paste flash when CJK IME
    /// would otherwise swallow bare Unicode events.
    #[cfg(target_os = "windows")]
    pub fn insert_via_unicode_keystrokes_ime_safe(&self, text: &str) -> InsertStatus {
        if text.is_empty() {
            return InsertStatus::CopiedFallback;
        }
        match crate::unicode_keystroke::type_unicode_chunk_ime_safe(text) {
            Ok(n) if n > 0 || text.is_empty() => InsertStatus::Inserted,
            Ok(_) => InsertStatus::CopiedFallback,
            Err(err) => {
                log::warn!("[insertion] IME-safe Unicode SendInput failed: {err}");
                InsertStatus::CopiedFallback
            }
        }
    }

    /// Insert `text` at the current cursor position.
    /// macOS 走 AX 直写 / Cmd+V：`_restore_clipboard_after_paste` 与 `_paste_shortcut`
    /// 仅为跨平台调用方对齐签名而存在，本路径不读它们。
    #[cfg(target_os = "macos")]
    pub fn insert(
        &self,
        text: &str,
        _restore_clipboard_after_paste: bool,
        _paste_shortcut: PasteShortcut,
    ) -> InsertStatus {
        if text.is_empty() {
            return InsertStatus::CopiedFallback;
        }
        if !copy_to_clipboard(text) {
            return InsertStatus::Failed;
        }
        macos_insert_status_after_paste(simulate_paste())
    }

    /// Copy text without attempting a synthetic paste. Used when the platform cannot
    /// prove the original input target is active enough to safely receive Ctrl/Cmd+V.
    pub fn copy_fallback(&self, text: &str) -> InsertStatus {
        if text.is_empty() {
            return InsertStatus::CopiedFallback;
        }
        if copy_to_clipboard(text) {
            InsertStatus::CopiedFallback
        } else {
            InsertStatus::Failed
        }
    }

    /// Correct a provisional paste only when the target itself confirms that
    /// the text immediately before the caret is exactly this session's paste.
    /// A final ASR rewrite must never blindly backspace into user content.
    #[cfg(target_os = "windows")]
    pub fn replace_verified_suffix(
        &self,
        expected: &str,
        replacement: &str,
        target_hwnd: usize,
        restore_clipboard_after_paste: bool,
        paste_shortcut: PasteShortcut,
    ) -> Result<InsertStatus, String> {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};
        use unicode_segmentation::UnicodeSegmentation;
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

        let count = expected.graphemes(true).count();
        if count == 0 || count > 8_192 || replacement.is_empty() {
            return Err("provisional suffix is outside the verified correction bounds".into());
        }
        if !clipboard_transport_is_reversible() {
            return Err("clipboard cannot be restored after verification".into());
        }
        let same_target = || unsafe { GetForegroundWindow().0 as usize == target_hwnd };
        if !same_target() {
            return Err("session target is no longer foreground".into());
        }
        // The target may be a terminal: Ctrl+C is a task interrupt there.
        // Require a read-only accessibility path before selecting anything.
        let selection_reader = crate::selection::windows_uia::WindowsSelectionReader::new(target_hwnd)?;
        let focus_before_select = focused_child_snapshot(target_hwnd);
        let mut keyboard = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        // Select from the caret to the editor start with one chord. Selecting
        // by counting Left events was not reliable in Chromium: even a single
        // SendInput batch selected only 109 of 110 provisional characters in
        // the captured session. Read the target once and replace only on an
        // exact match; ordinary user content preceding this session is safe.
        if let Err(err) = select_to_edit_start() {
            if same_target() {
                let _ = keyboard.key(Key::RightArrow, Direction::Click);
            }
            return Err(format!("could not select provisional suffix: {err}"));
        }
        if !same_target() {
            return Err("session target changed before suffix verification".into());
        }
        let focus_after_select = focused_child_snapshot(target_hwnd);
        if focus_after_select != focus_before_select {
            if same_target() {
                let _ = keyboard.key(Key::RightArrow, Direction::Click);
            }
            return Err(format!(
                "focused editor changed during suffix selection target_hwnd=0x{target_hwnd:x} before={focus_before_select:?} after={focus_after_select:?}"
            ));
        }
        let mut observed = match selection_reader.read_selected_text() {
            Ok(value) => value,
            Err(err) => {
                if same_target() {
                    let _ = keyboard.key(Key::RightArrow, Direction::Click);
                }
                return Err(format!("could not read selected provisional suffix: {err}"));
            }
        };
        // If the editor also contains earlier user text, leave that content
        // untouched. The old bounded suffix selection remains a fallback for
        // this case, with its own exact readback before any replacement.
        if observed != expected && observed.ends_with(expected) && count <= 160 && same_target() {
            let fallback = (|| -> Result<String, String> {
                keyboard
                    .key(Key::RightArrow, Direction::Click)
                    .map_err(|e| e.to_string())?;
                select_previous_graphemes(count)?;
                selection_reader.read_selected_text()
            })();
            match fallback {
                Ok(value) => observed = value,
                Err(err) => {
                    if same_target() {
                        let _ = keyboard.key(Key::RightArrow, Direction::Click);
                    }
                    return Err(format!("could not verify provisional suffix fallback: {err}"));
                }
            }
        }
        if observed != expected || !same_target() {
            if same_target() {
                let _ = keyboard.key(Key::RightArrow, Direction::Click);
            }
            return Err(format!(
                "selected text did not match provisional suffix (expected_chars={} observed_chars={} observed_prefix={} observed_suffix={})",
                expected.chars().count(), observed.chars().count(),
                expected.starts_with(&observed), expected.ends_with(&observed)
            ));
        }
        // The clipboard is touched only after the target has supplied an
        // exact readback. An unavailable provider cannot disturb user content.
        let mut clipboard = arboard::Clipboard::new().map_err(|e| {
            if same_target() {
                let _ = keyboard.key(Key::RightArrow, Direction::Click);
            }
            e.to_string()
        })?;
        let previous = snapshot_clipboard(&mut clipboard);
        clipboard.set_text(replacement.to_string()).map_err(|e| {
            if same_target() {
                let _ = keyboard.key(Key::RightArrow, Direction::Click);
            }
            restore_clipboard_snapshot(&mut clipboard, &previous);
            e.to_string()
        })?;
        if !same_target() {
            restore_clipboard_snapshot(&mut clipboard, &previous);
            return Err("session target changed before corrected paste".into());
        }
        // Keep the selection through paste: the host replaces exactly the
        // text that accessibility readback returned. No Delete or Backspace is sent.
        if let Err(err) = simulate_paste(paste_shortcut) {
            // A shortcut can fail while releasing its modifier, after the host
            // has already handled Ctrl+V. Never trigger a second full paste.
            log::warn!("[insertion] verified correction paste unconfirmed: {err}");
            if same_target() {
                let _ = keyboard.key(Key::RightArrow, Direction::Click);
            }
            return Ok(InsertStatus::SubmittedUnconfirmed);
        }
        if restore_clipboard_after_paste {
            schedule_clipboard_restore(ClipboardRestorePlan {
                inserted_text: replacement.to_string(),
                previous,
            });
        }
        Ok(InsertStatus::PasteSent)
    }
}

#[cfg(target_os = "windows")]
fn restore_clipboard_snapshot(clipboard: &mut arboard::Clipboard, snapshot: &ClipboardSnapshot) {
    let result = match snapshot {
        ClipboardSnapshot::Text(text) => clipboard.set_text(text.clone()),
        ClipboardSnapshot::Image(image) => clipboard.set_image(image.clone()),
        ClipboardSnapshot::Absent => clipboard.clear(),
    };
    if let Err(err) = result {
        log::warn!("[insertion] failed to restore clipboard after suffix verification: {err}");
    }
}

#[cfg(target_os = "windows")]
fn focused_child_snapshot(target_hwnd: usize) -> (usize, String) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };

    let root = HWND(target_hwnd as *mut c_void);
    let thread_id = unsafe { GetWindowThreadProcessId(root, None) };
    if thread_id == 0 {
        return (0, "unknown-thread".into());
    }
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(thread_id, &mut info) }.is_err() {
        return (0, "unknown-focus".into());
    }
    let focused = info.hwndFocus;
    if focused.0.is_null() {
        return (0, "no-focus".into());
    }
    let mut class_name = [0u16; 128];
    let len = unsafe { GetClassNameW(focused, &mut class_name) };
    let class_name = String::from_utf16_lossy(&class_name[..len.max(0) as usize]);
    (focused.0 as usize, class_name)
}

#[cfg(target_os = "windows")]
fn select_previous_graphemes(count: usize) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VK_LEFT, VK_SHIFT,
    };

    fn event(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let mut inputs = Vec::with_capacity(count * 2 + 2);
    inputs.push(event(VK_SHIFT, false));
    for _ in 0..count {
        inputs.push(event(VK_LEFT, false));
        inputs.push(event(VK_LEFT, true));
    }
    inputs.push(event(VK_SHIFT, true));
    let sent = unsafe { SendInput(&mut inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        // A short send can omit the final Shift key-up. Release it before
        // returning so the user's next keystroke is not modified.
        let mut release = [event(VK_SHIFT, true)];
        unsafe { SendInput(&mut release, std::mem::size_of::<INPUT>() as i32) };
        return Err(format!(
            "SendInput selected {sent}/{} key events",
            inputs.len()
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn select_to_edit_start() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP, VK_CONTROL, VK_HOME, VK_SHIFT,
    };

    fn event(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let mut inputs = [
        event(VK_CONTROL, false),
        event(VK_SHIFT, false),
        event(VK_HOME, false),
        event(VK_HOME, true),
        event(VK_SHIFT, true),
        event(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&mut inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        let mut release = [event(VK_SHIFT, true), event(VK_CONTROL, true)];
        unsafe { SendInput(&mut release, std::mem::size_of::<INPUT>() as i32) };
        return Err(format!("SendInput selected {sent}/{} key events", inputs.len()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_insert_status_after_paste(result: Result<(), String>) -> InsertStatus {
    match result {
        Ok(()) => insertion_success_status(),
        Err(err) => {
            log::warn!("[insertion] simulated paste failed: {}", err);
            InsertStatus::CopiedFallback
        }
    }
}

impl Default for TextInserter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
struct ClipboardRestorePlan {
    inserted_text: String,
    previous: ClipboardSnapshot,
}

/// What the user's clipboard held before a dictation borrowed it as paste
/// transport.  `Absent` covers both a truly empty clipboard and content this
/// crate cannot write back; restoring `Absent` clears the dictation text out,
/// so a user who never opted into clipboard retention finds an empty — not a
/// dictated — clipboard afterwards (2026-09-19: the old text-only restore
/// silently left the dictation text behind in exactly those cases).
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone)]
enum ClipboardSnapshot {
    Text(String),
    Image(arboard::ImageData<'static>),
    Absent,
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone)]
struct PendingClipboardRestore {
    latest_restore_id: u64,
    original: ClipboardSnapshot,
}

#[cfg(not(target_os = "macos"))]
static NEXT_CLIPBOARD_RESTORE_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(not(target_os = "macos"))]
static PENDING_CLIPBOARD_RESTORE: Lazy<Mutex<Option<PendingClipboardRestore>>> =
    Lazy::new(|| Mutex::new(None));

fn copy_to_clipboard(text: &str) -> bool {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(err) => {
            log::error!("[insertion] clipboard init failed: {}", err);
            return false;
        }
    };
    if let Err(err) = clipboard.set_text(text.to_string()) {
        log::error!("[insertion] clipboard set_text failed: {}", err);
        return false;
    }
    true
}

/// May the clipboard be borrowed as paste transport without destroying
/// content the restore path cannot write back?  Copied files and other
/// exotic formats have no arboard read/write round trip, so while such
/// content is held the Windows insert path declines the paste transport and
/// uses keystrokes for that one dictation instead (2026-09-19).
#[cfg(target_os = "windows")]
pub fn clipboard_transport_is_reversible() -> bool {
    use windows::Win32::System::DataExchange::CountClipboardFormats;
    if unsafe { CountClipboardFormats() } == 0 {
        // Empty clipboard: transport is reversible via clear-on-restore.
        return true;
    }
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        // Cannot inspect right now. The paste path itself fails safely on a
        // busy clipboard, so do not pre-empt it here.
        return true;
    };
    clipboard.get_text().is_ok() || clipboard.get_image().is_ok()
}

#[cfg(not(target_os = "windows"))]
pub fn clipboard_transport_is_reversible() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn snapshot_clipboard(clipboard: &mut arboard::Clipboard) -> ClipboardSnapshot {
    match clipboard.get_text() {
        Ok(text) => ClipboardSnapshot::Text(text),
        Err(_) => match clipboard.get_image() {
            Ok(image) => ClipboardSnapshot::Image(image),
            Err(_) => ClipboardSnapshot::Absent,
        },
    }
}

#[cfg(not(target_os = "macos"))]
fn copy_to_clipboard_with_restore_plan(text: &str) -> Result<ClipboardRestorePlan, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let previous = snapshot_clipboard(&mut clipboard);
    clipboard
        .set_text(text.to_string())
        .map_err(|e| e.to_string())?;
    Ok(ClipboardRestorePlan {
        inserted_text: text.to_string(),
        previous,
    })
}

#[cfg(not(target_os = "macos"))]
fn insert_with_clipboard_restore(
    text: &str,
    restore_clipboard_after_paste: bool,
    paste_shortcut: PasteShortcut,
) -> InsertStatus {
    let restore_plan = match copy_to_clipboard_with_restore_plan(text) {
        Ok(plan) => plan,
        Err(err) => {
            log::error!("[insertion] clipboard write failed: {}", err);
            return InsertStatus::Failed;
        }
    };

    if let Err(err) = simulate_paste(paste_shortcut) {
        log::warn!("[insertion] simulated paste failed: {}", err);
        return InsertStatus::CopiedFallback;
    }

    if restore_clipboard_after_paste {
        schedule_clipboard_restore(restore_plan);
    }
    // 关掉 → 听写文本留在剪贴板里，simulate_paste 没真正落地时用户能手动 Ctrl+V 找回。
    insertion_success_status()
}

#[cfg(not(target_os = "macos"))]
fn schedule_clipboard_restore(plan: ClipboardRestorePlan) {
    let (restore_id, original) = remember_pending_clipboard_restore(plan.previous.clone());
    std::thread::spawn(move || {
        restore_clipboard_after_delay(plan, original, restore_id, CLIPBOARD_RESTORE_DELAY)
    });
}

#[cfg(not(target_os = "macos"))]
fn remember_pending_clipboard_restore(previous: ClipboardSnapshot) -> (u64, ClipboardSnapshot) {
    let restore_id = NEXT_CLIPBOARD_RESTORE_ID.fetch_add(1, Ordering::SeqCst);
    let original = {
        let mut pending = PENDING_CLIPBOARD_RESTORE.lock();
        let original = pending
            .as_ref()
            .map(|batch| batch.original.clone())
            .unwrap_or(previous);
        *pending = Some(PendingClipboardRestore {
            latest_restore_id: restore_id,
            original: original.clone(),
        });
        original
    };
    (restore_id, original)
}

#[cfg(not(target_os = "macos"))]
enum ClipboardRestoreAction<'a> {
    PutText(&'a str),
    PutImage(&'a arboard::ImageData<'static>),
    Clear,
    Skip,
}

#[cfg(not(target_os = "macos"))]
fn clipboard_restore_action<'a>(
    current_text: Option<&str>,
    inserted_text: &str,
    snapshot: &'a ClipboardSnapshot,
) -> ClipboardRestoreAction<'a> {
    if !should_restore_clipboard(current_text, inserted_text) {
        return ClipboardRestoreAction::Skip;
    }
    match snapshot {
        ClipboardSnapshot::Text(text) => ClipboardRestoreAction::PutText(text),
        ClipboardSnapshot::Image(image) => ClipboardRestoreAction::PutImage(image),
        ClipboardSnapshot::Absent => ClipboardRestoreAction::Clear,
    }
}

#[cfg(not(target_os = "macos"))]
fn restore_clipboard_after_delay(
    plan: ClipboardRestorePlan,
    original: ClipboardSnapshot,
    restore_id: u64,
    delay: Duration,
) {
    std::thread::sleep(delay);

    if !is_latest_clipboard_restore(restore_id) {
        return;
    }

    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(err) => {
            log::warn!(
                "[insertion] clipboard re-open failed during restore: {}",
                err
            );
            clear_pending_clipboard_restore(restore_id);
            return;
        }
    };

    let current_text = match clipboard.get_text() {
        Ok(current) => Some(current),
        Err(err) => {
            log::warn!(
                "[insertion] clipboard get_text failed during restore: {}",
                err
            );
            None
        }
    };

    match clipboard_restore_action(current_text.as_deref(), &plan.inserted_text, &original) {
        ClipboardRestoreAction::PutText(text) => {
            if let Err(err) = clipboard.set_text(text.to_string()) {
                log::warn!("[insertion] clipboard restore failed: {}", err);
            }
        }
        ClipboardRestoreAction::PutImage(image) => {
            if let Err(err) = clipboard.set_image(image.clone()) {
                log::warn!("[insertion] clipboard image restore failed: {}", err);
            }
        }
        ClipboardRestoreAction::Clear => {
            if let Err(err) = clipboard.clear() {
                log::warn!("[insertion] clipboard clear failed: {}", err);
            }
        }
        ClipboardRestoreAction::Skip => {
            log::info!(
                "[insertion] skip clipboard restore: latest clipboard no longer matches inserted text"
            );
        }
    }

    clear_pending_clipboard_restore(restore_id);
}

#[cfg(not(target_os = "macos"))]
fn is_latest_clipboard_restore(restore_id: u64) -> bool {
    matches!(
        PENDING_CLIPBOARD_RESTORE.lock().as_ref(),
        Some(batch) if batch.latest_restore_id == restore_id
    )
}

#[cfg(not(target_os = "macos"))]
fn clear_pending_clipboard_restore(restore_id: u64) {
    let mut pending = PENDING_CLIPBOARD_RESTORE.lock();
    if matches!(pending.as_ref(), Some(batch) if batch.latest_restore_id == restore_id) {
        pending.take();
    }
}

#[cfg(not(target_os = "macos"))]
fn should_restore_clipboard(current_text: Option<&str>, inserted_text: &str) -> bool {
    matches!(current_text, Some(current) if current == inserted_text)
}

#[cfg(target_os = "macos")]
fn simulate_paste() -> Result<(), String> {
    if !matches!(
        crate::permissions::check_accessibility(),
        crate::permissions::PermissionStatus::Granted
    ) {
        return Err("accessibility permission is not granted".into());
    }
    macos::post_cmd_v()
}

/// 把用户配置的 PasteShortcut 拆成 `(modifiers, primary)`。modifier 顺序决定 enigo
/// 按下/释放顺序，跟物理键盘一致：先 Ctrl 再 Shift 再主键，释放反向。
#[cfg(not(target_os = "macos"))]
fn paste_keys(shortcut: PasteShortcut) -> (Vec<enigo::Key>, enigo::Key) {
    use enigo::Key;
    match shortcut {
        PasteShortcut::CtrlV => (vec![Key::Control], Key::Unicode('v')),
        PasteShortcut::CtrlShiftV => (vec![Key::Control, Key::Shift], Key::Unicode('v')),
        PasteShortcut::ShiftInsert => (vec![Key::Shift], Key::Insert),
    }
}

#[cfg(not(target_os = "macos"))]
fn simulate_paste(shortcut: PasteShortcut) -> Result<(), String> {
    use enigo::{Direction, Enigo, Keyboard, Settings};
    let (modifiers, primary) = paste_keys(shortcut);
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;

    // 跟原版 simulate_paste 保持同一行为：按下 modifier → 点击主键 → 反向释放 modifier。
    // 任何中途失败都尽量把已经按下的 modifier 反向释放回来，避免卡键。`pressed`
    // 记录已经成功按下的 modifier 数；用切片 `modifiers[..pressed]` 控制释放范围
    // —— 切片自带 DoubleEndedIterator，可以放心 `.rev()`。
    let mut pressed = 0usize;
    let mut first_err: Option<String> = None;

    for modifier in &modifiers {
        if let Err(e) = enigo.key(*modifier, Direction::Press) {
            first_err = Some(e.to_string());
            break;
        }
        pressed += 1;
    }

    if first_err.is_none() {
        if let Err(e) = enigo.key(primary, Direction::Click) {
            first_err = Some(e.to_string());
        }
    }

    for modifier in modifiers[..pressed].iter().rev() {
        if let Err(e) = enigo.key(*modifier, Direction::Release) {
            if first_err.is_none() {
                first_err = Some(e.to_string());
            }
        }
    }

    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(target_os = "macos")]
fn insertion_success_status() -> InsertStatus {
    InsertStatus::Inserted
}

#[cfg(not(target_os = "macos"))]
fn insertion_success_status() -> InsertStatus {
    // Windows/Linux 的 Ctrl+V 只能证明粘贴快捷键已发送，不能证明目标控件已接收。
    InsertStatus::PasteSent
}

#[cfg(target_os = "windows")]
mod windows_unicode {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY,
    };

    pub fn send_text(text: &str) -> Result<(), String> {
        for unit in text.encode_utf16() {
            send_utf16_unit(unit, false)?;
            send_utf16_unit(unit, true)?;
        }
        Ok(())
    }

    fn send_utf16_unit(unit: u16, key_up: bool) -> Result<(), String> {
        let flags = if key_up {
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
        } else {
            KEYEVENTF_UNICODE
        };
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYBD_EVENT_FLAGS(flags.0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        if sent == 1 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
}

// ─────────────────────────── macOS native CGEvent paste ───────────────────────────

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;

    #[repr(C)]
    struct OpaqueCGEvent(c_void);
    type CGEventRef = *mut OpaqueCGEvent;

    #[repr(C)]
    struct OpaqueCGEventSource(c_void);
    type CGEventSourceRef = *mut OpaqueCGEventSource;

    type CGEventTapLocation = u32;
    type CGEventSourceStateID = i32;
    type CGKeyCode = u16;
    type CGEventFlags = u64;

    const KCG_HID_EVENT_TAP: CGEventTapLocation = 0;
    const KCG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE: CGEventSourceStateID = 1;
    const KCG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x00100000;
    /// Virtual keycode for "V" on US/ANSI layouts (kVK_ANSI_V).
    const KEY_V: CGKeyCode = 9;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceCreate(state_id: CGEventSourceStateID) -> CGEventSourceRef;
        fn CGEventCreateKeyboardEvent(
            source: CGEventSourceRef,
            virtual_key: CGKeyCode,
            key_down: bool,
        ) -> CGEventRef;
        fn CGEventSetFlags(event: CGEventRef, flags: CGEventFlags);
        fn CGEventPost(tap: CGEventTapLocation, event: CGEventRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    /// 与 Swift `TextInserter.simulatePaste()` 同源:
    ///   下 V + 加 Cmd flag → post → 上 V + 加 Cmd flag → post
    /// 全部走 C 层 CGEvent，不会触发 enigo 那条 TSM 主线程断言路径。
    pub fn post_cmd_v() -> Result<(), String> {
        unsafe {
            let source = CGEventSourceCreate(KCG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE);
            // 即使 source 是空也能 post（Apple 文档允许 NULL source），所以不当致命错误。
            let down = CGEventCreateKeyboardEvent(source, KEY_V, true);
            let up = CGEventCreateKeyboardEvent(source, KEY_V, false);
            if down.is_null() || up.is_null() {
                if !source.is_null() {
                    CFRelease(source as *const c_void);
                }
                if !down.is_null() {
                    CFRelease(down as *const c_void);
                }
                if !up.is_null() {
                    CFRelease(up as *const c_void);
                }
                return Err("CGEventCreateKeyboardEvent returned null".into());
            }
            CGEventSetFlags(down, KCG_EVENT_FLAG_MASK_COMMAND);
            CGEventSetFlags(up, KCG_EVENT_FLAG_MASK_COMMAND);
            CGEventPost(KCG_HID_EVENT_TAP, down);
            CGEventPost(KCG_HID_EVENT_TAP, up);

            CFRelease(down as *const c_void);
            CFRelease(up as *const c_void);
            if !source.is_null() {
                CFRelease(source as *const c_void);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn automatic_final_correction_never_sends_terminal_interrupt_shortcut() {
        // A foreground AI terminal interprets Ctrl+C as cancel. Keep the
        // automatic correction path independent of clipboard copy shortcuts.
        let insertion_source = include_str!("insertion.rs");
        assert!(!insertion_source.contains(&["send_", "ctrl_c("].concat()));
        assert!(!insertion_source.contains(&["Key::Unicode(", "'c')"].concat()));
    }

    #[cfg(target_os = "windows")]
    use std::sync::{Arc, Mutex};
    #[cfg(target_os = "windows")]
    use std::thread;
    #[cfg(target_os = "windows")]
    use std::time::Duration;

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn restore_only_when_clipboard_still_holds_inserted_text() {
        assert!(should_restore_clipboard(
            Some("dictated text"),
            "dictated text"
        ));
        assert!(!should_restore_clipboard(
            Some("user changed clipboard"),
            "dictated text"
        ));
        assert!(!should_restore_clipboard(None, "dictated text"));
    }

    /// issue #360: 用户配置的快捷键必须真的映射到对应按键，否则 Settings UI
    /// 改了也没用。这里只检查 modifier 数量 + 主键，不依赖 enigo 内部 PartialEq。
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn paste_keys_match_configured_shortcut() {
        use enigo::Key;

        let (mods, primary) = paste_keys(PasteShortcut::CtrlV);
        assert_eq!(mods.len(), 1);
        assert!(matches!(mods[0], Key::Control));
        assert!(matches!(primary, Key::Unicode('v')));

        let (mods, primary) = paste_keys(PasteShortcut::CtrlShiftV);
        assert_eq!(mods.len(), 2);
        assert!(matches!(mods[0], Key::Control));
        assert!(matches!(mods[1], Key::Shift));
        assert!(matches!(primary, Key::Unicode('v')));

        let (mods, primary) = paste_keys(PasteShortcut::ShiftInsert);
        assert_eq!(mods.len(), 1);
        assert!(matches!(mods[0], Key::Shift));
        assert!(matches!(primary, Key::Insert));
    }

    #[test]
    fn empty_insertions_never_touch_clipboard_or_paste_path() {
        let inserter = TextInserter::new();

        assert_eq!(
            inserter.insert("", true, PasteShortcut::CtrlV),
            InsertStatus::CopiedFallback
        );
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(
                inserter.insert_via_clipboard_fallback("", true, PasteShortcut::CtrlV),
                InsertStatus::CopiedFallback
            );
        }
        assert_eq!(inserter.copy_fallback(""), InsertStatus::CopiedFallback);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn pending_clipboard_restore_keeps_first_original_until_latest_restore() {
        *PENDING_CLIPBOARD_RESTORE.lock() = None;

        let (first_id, first_original) = remember_pending_clipboard_restore(
            ClipboardSnapshot::Text("user clipboard".to_string()),
        );
        let (second_id, second_original) = remember_pending_clipboard_restore(
            ClipboardSnapshot::Text("first dictated text".to_string()),
        );

        assert_ne!(first_id, second_id);
        assert!(matches!(
            &first_original,
            ClipboardSnapshot::Text(text) if text == "user clipboard"
        ));
        assert!(matches!(
            &second_original,
            ClipboardSnapshot::Text(text) if text == "user clipboard"
        ));
        assert!(!is_latest_clipboard_restore(first_id));
        assert!(is_latest_clipboard_restore(second_id));

        clear_pending_clipboard_restore(first_id);
        assert!(is_latest_clipboard_restore(second_id));
        clear_pending_clipboard_restore(second_id);
        assert!(!is_latest_clipboard_restore(second_id));
    }

    /// 2026-09-19: the dictation text must never linger for a user who did
    /// not opt into clipboard retention. An empty (or unwritable) original
    /// clipboard restores by clearing, not by skipping.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn absent_clipboard_snapshot_restores_by_clearing() {
        let snapshot = ClipboardSnapshot::Absent;
        assert!(matches!(
            clipboard_restore_action(Some("dictated"), "dictated", &snapshot),
            ClipboardRestoreAction::Clear
        ));
        // A user copy made in between still wins over any restore.
        assert!(matches!(
            clipboard_restore_action(Some("user changed clipboard"), "dictated", &snapshot),
            ClipboardRestoreAction::Skip
        ));
        assert!(matches!(
            clipboard_restore_action(None, "dictated", &snapshot),
            ClipboardRestoreAction::Skip
        ));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn clipboard_restore_puts_back_text_and_image_snapshots() {
        let text_snapshot = ClipboardSnapshot::Text("user text".to_string());
        assert!(matches!(
            clipboard_restore_action(Some("dictated"), "dictated", &text_snapshot),
            ClipboardRestoreAction::PutText(text) if *text == *"user text"
        ));

        let image = arboard::ImageData {
            width: 1,
            height: 1,
            bytes: std::borrow::Cow::Owned(vec![0u8, 0, 0, 255]),
        };
        let image_snapshot = ClipboardSnapshot::Image(image);
        assert!(matches!(
            clipboard_restore_action(Some("dictated"), "dictated", &image_snapshot),
            ClipboardRestoreAction::PutImage(_)
        ));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn clipboard_restore_skips_when_clipboard_no_longer_matches_inserted_text() {
        assert!(should_restore_clipboard(
            Some("dictated text"),
            "dictated text"
        ));
        assert!(!should_restore_clipboard(
            Some("user edited clipboard"),
            "dictated text"
        ));
        assert!(!should_restore_clipboard(None, "dictated text"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_direct_write_or_paste_failure_keeps_copied_fallback_available() {
        assert_eq!(
            macos_insert_status_after_paste(Ok(())),
            InsertStatus::Inserted
        );
        assert_eq!(
            macos_insert_status_after_paste(Err("AX direct write unavailable".to_string())),
            InsertStatus::CopiedFallback
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn delayed_terminal_paste_must_see_dictated_text_before_clipboard_restore() {
        let inserted_text = "dictated text".to_string();
        let previous_text = "older clipboard".to_string();
        let clipboard = Arc::new(Mutex::new(inserted_text.clone()));
        let pasted = Arc::new(Mutex::new(None::<String>));

        let clipboard_for_paste = Arc::clone(&clipboard);
        let pasted_for_paste = Arc::clone(&pasted);
        let reader = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let seen = clipboard_for_paste.lock().unwrap().clone();
            *pasted_for_paste.lock().unwrap() = Some(seen);
        });

        thread::sleep(CLIPBOARD_RESTORE_DELAY);
        let current_text = Some(clipboard.lock().unwrap().clone());
        if should_restore_clipboard(current_text.as_deref(), &inserted_text) {
            *clipboard.lock().unwrap() = previous_text;
        }

        reader.join().unwrap();

        assert_eq!(
            pasted.lock().unwrap().as_deref(),
            Some(inserted_text.as_str())
        );
    }
}
