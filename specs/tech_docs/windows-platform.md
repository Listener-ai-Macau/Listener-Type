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
