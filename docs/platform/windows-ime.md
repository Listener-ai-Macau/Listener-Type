# Windows IME

Listener Type ships a native TSF text service for reliable Windows insertion.

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
- Installer hooks: `src-tauri/nsis/listener-type-ime-hooks.nsh`, `src-tauri/wix/listener-type-ime.wxs`.
- Smoke scripts: `scripts/windows-ime-install-smoke.ps1`, `scripts/windows-real-asr-insertion-smoke.ps1`.

## Verification

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib windows_ime_profile
node scripts/windows-package-msvc.test.mjs
```

On Windows, install the generated NSIS/MSI package and run `scripts/windows-ime-install-smoke.ps1`.
