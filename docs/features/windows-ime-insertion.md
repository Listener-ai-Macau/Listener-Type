# Windows IME Insertion

## Status

- status: `implemented`
- scope: Windows TSF/IME insertion path plus normal insertion fallback

## Source Map

- IME project: `windows-ime/`
- IPC/session/profile/protocol: `src-tauri/src/windows_ime_ipc.rs`, `src-tauri/src/windows_ime_session.rs`, `src-tauri/src/windows_ime_profile.rs`, `src-tauri/src/windows_ime_protocol.rs`
- General insertion fallback: `src-tauri/src/insertion.rs`, `src-tauri/src/unicode_keystroke.rs`
- Settings/status UI: `src/pages/settings/PermissionsSection.tsx`, `src/lib/ipc.ts`

## Behavior

- Windows IME status is surfaced through the command layer.
- Dictation insertion can use the Windows IME path when available.
- Clipboard/direct insertion fallback remains required so text is not lost when IME insertion is unavailable.

## Verification

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows-ime-build.ps1
powershell -ExecutionPolicy Bypass -File scripts\windows-ime-install-smoke.ps1
```

## Known Limits

- Windows-only.
- Depends on TSF registration, target app compatibility, and Windows input method state.
