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

## Live text completion

A stable clause already pasted during recording is final for the editor.
Later ASR revisions may update the provisional preview, but finalization
only appends speech beyond the committed boundary. It never selects or
replaces earlier dictation, sends Ctrl+C for verification, or copies the
full revised transcript solely because the editor lacks UI Automation
selection support. An ambiguous tail is left uncommitted rather than
replaying an earlier clause.

During dictation, each paste and final submit targets the currently focused
window. Delivery must not call `SetForegroundWindow`, restore the window that
had focus when recording began, or infer a target from its title. If the user
switches apps during an optional TSF composition, cancel that composition and
continue with the current foreground editor. A delayed post-dictation shortcut
may fire only while the editor that received the final text still has focus.
