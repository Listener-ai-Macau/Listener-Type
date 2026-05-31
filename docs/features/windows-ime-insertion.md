# Windows IME Insertion

## Status

- status: `implemented`
- scope: Windows TSF/IME insertion path plus normal insertion fallback

## Source Map

- IME project: `windows-ime/`
- IPC/session/profile/protocol: `src-tauri/src/windows_ime_ipc.rs`, `src-tauri/src/windows_ime_session.rs`, `src-tauri/src/windows_ime_profile.rs`, `src-tauri/src/windows_ime_protocol.rs`
- General insertion fallback: `src-tauri/src/insertion.rs`, `src-tauri/src/unicode_keystroke.rs`
- Optional installer registration hooks: `src-tauri/nsis/listener-type-ime-hooks.nsh`
- Settings/status UI: `src/pages/settings/PermissionsSection.tsx`, `src/lib/ipc.ts`
- Platform documentation: `docs/platform/windows-ime.md`, `docs/quickstart/permissions.md`

## Behavior

- Windows IME status is surfaced through the command layer.
- Dictation insertion can use the Windows IME path when the TSF service is explicitly registered and the target app accepts it.
- Default product installers do not register the TSF IME. They only carry legacy cleanup hooks that unregister an older Listener Type TSF service if one is present. Optional hooks can still register and unregister both x64 and x86 TSF DLLs for dedicated TSF validation builds.
- The backend keeps profile/status/protocol/session code separate from the native TSF service.
- Clipboard/direct insertion fallback remains required so text is not lost when IME insertion is unavailable.

## Verification

```powershell
cargo test --manifest-path src-tauri\Cargo.toml --lib windows_ime_profile
powershell -ExecutionPolicy Bypass -File scripts\windows-ime-build.ps1
powershell -ExecutionPolicy Bypass -File scripts\windows-ime-register.ps1
powershell -ExecutionPolicy Bypass -File scripts\windows-ime-install-smoke.ps1
powershell -ExecutionPolicy Bypass -File scripts\windows-ime-unregister.ps1
```

## Known Limits

- Windows-only.
- Depends on TSF registration, target app compatibility, and Windows input method state.
