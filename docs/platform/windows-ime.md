# Windows IME

Listener Type uses a native TSF text service for Windows insertion. The current
MSI bundles the x64 and x86 DLLs as resources, clears stale registration, and
registers them during installation. It unregisters them on removal. The NSIS
installer still uses legacy cleanup hooks and does not register TSF.

## Identity

| Item | Value |
| --- | --- |
| Text service CLSID | `{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}` |
| Profile GUID | `{19F96D43-A5EB-46C9-8A73-9FCA5A0630C8}` |
| Language ID | `0x0804` |
| DLL | `ListenerTypeIme.dll` |
| Display name | `Listener Type Voice Input` |

## Source Map

- Native TSF service: `windows-ime/src/text_service.cpp`, `windows-ime/src/registry.cpp`, `windows-ime/src/ipc_client.cpp`.
- GUID definitions: `windows-ime/src/guids.h`, `src-tauri/src/windows_ime_profile.rs`.
- Backend bridge: `src-tauri/src/windows_ime.rs`, `src-tauri/src/windows_ime_profile.rs`.
- Legacy cleanup hooks used by NSIS: `src-tauri/nsis/listener-type-ime-cleanup-hooks.nsh`.
- MSI cleanup and registration: `src-tauri/wix/listener-type-ime-cleanup.wxs`.
- Optional TSF packaging hooks: `src-tauri/nsis/listener-type-ime-hooks.nsh`, `src-tauri/wix/listener-type-ime.wxs`.
- Smoke scripts: `scripts/windows-ime-install-smoke.ps1`, `scripts/windows-real-asr-insertion-smoke.ps1`.

## Verification

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib windows_ime_profile
node scripts/windows-package-msvc.test.mjs
```

For manual TSF validation, run the insertion smoke against the installed MSI.
For standalone IME development, register with `scripts/windows-ime-register.ps1`
and unregister with `scripts/windows-ime-unregister.ps1` afterward. An MSI file
by itself does not prove that registration succeeded on a particular machine.
