# Windows Platform Notes

Windows support spans packaging, global hotkeys, microphone privacy, clipboard behavior and TSF IME insertion.

## Source Areas

- `src-tauri/src/platform_hotkey.rs`
- `src-tauri/src/windows_ime.rs`
- `src-tauri/src/windows_ime_profile.rs`
- `windows-ime/**`
- `scripts/windows-*.ps1`
- `scripts/windows-*.test.mjs`

## Rules

- Keep Listener Type TSF CLSID/Profile GUID aligned across Rust, C++ and installer smoke scripts.
- Keep `ListenerTypeIme.dll` naming aligned across NSIS, WiX, Visual Studio project and smoke scripts.
- On non-Windows hosts, run JS static checks and record the missing runtime coverage.
- On Windows, verify install, registration, dictation insertion and uninstall cleanup.

## Final text correction

When a live clause was already pasted, final ASR may revise it. The Windows
insertion adapter may replace that clause only after UI Automation reads back
the exact selected text from a focused editable control. It must never send
Ctrl+C for automatic verification: terminals interpret that shortcut as task
interrupt. If safe readback is unavailable, leave the existing text in place
and copy the final transcript for recovery; do not paste the full transcript
again or report target-confirmed delivery.

Some Chromium contenteditable controls expose their full editor text through
UI Automation but report no selection for Ctrl+Shift+Home. If the complete
editor text exactly equals this session's early paste, the adapter may use
Ctrl+A and must then verify the selected text still matches exactly before
pasting the corrected final. Editors containing any unrelated text stay on the
bounded suffix route; unavailable selection still falls back to copying.

During dictation, each paste and final submit targets the currently focused
window. Delivery must not call `SetForegroundWindow`, restore the window that
had focus when recording began, or infer a target from its title. If the user
switches apps during an optional TSF composition, cancel that composition and
continue with the current foreground editor. A delayed post-dictation shortcut
may fire only while the editor that received the final text still has focus.
