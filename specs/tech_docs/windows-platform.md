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
