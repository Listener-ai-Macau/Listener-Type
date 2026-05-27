# Listener-Type OTA Transfer Write Options 1.1 Codex Validation

Date: 2026-05-27

Worktree: `C:\Users\Billy\Desktop\listener\Listener-Type-wt-codex-listener-type-ota-transfer-write-options-1.1`

Branch: `ai/codex-listener-type-ota-transfer-write-options-1.1`

## Changes

- OTA transfer preflight now prepares one shared BLE target and reuses it for the transfer.
- OTA data writes prefer `WriteWithResponse` when the characteristic supports it and derive chunk size from `GattSession.MaxPduSize`.
- `WriteWithoutResponse` remains supported with a safe 20-byte payload cap.
- During OTA transfer, Listener-Type opens an audio notify keepalive subscription and keeps it alive until transfer cleanup. This prevents Windows from treating the BLE link as idle during the OTA data phase.

## Hardware Validation

Firmware package:

- Version: `14f779f-dirty`
- Size: `645600`
- SHA256: `cf673aa4eb80cb0b764b7eb668066401e44b03780bd0c1eb59fe790bccae5c5d`
- Package path: `C:\Users\Billy\Desktop\listener\voice-keyboard-firmware-wt-codex-voice-keyboard-ota-update-1.4b\.cache\ota_firmware_keepalive\listener-ota-14f779f-dirty-20260527-170631`

True BLE transfer:

- Baseline device version: `ota-smoke-0.0.1-dirty`
- Transfer result: PASS
- Transferred bytes: `645600`
- Transport: `listener_ble_ota`
- Confirmed version: `14f779f-dirty`
- Transfer log: `C:\Users\Billy\Desktop\listener\voice-keyboard-firmware-wt-codex-voice-keyboard-ota-update-1.4b\tests\artifacts\voice_keyboard_ota_1_4b_codex_20260527-135752\listener_type_transfer_keepalive_ota_smoke_to_14f779f_dirty.txt`
- Serial log: `C:\Users\Billy\Desktop\listener\voice-keyboard-firmware-wt-codex-voice-keyboard-ota-update-1.4b\tests\artifacts\voice_keyboard_ota_1_4b_codex_20260527-135752\serial_during_keepalive_transfer.log`

Serial evidence shows audio notify subscription enabled at `380014`, OTA begin at `380834`, OTA finish at `472884`, notify disabled at `472914`, and reboot at `473384`. No `disconnect; reason=546` occurred during the OTA begin-to-finish window.

## Commands

| Command | Result |
|---|---|
| `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check` | PASS |
| `cargo check --manifest-path src-tauri\Cargo.toml` | PASS |
| `cargo test --manifest-path src-tauri\Cargo.toml firmware_ota --lib` | PASS, 11 tests |
| `npm run test:firmware-ota` | PASS |
| `git diff --check` | PASS, only LF/CRLF warnings |
| `cargo run --manifest-path src-tauri\Cargo.toml -- --firmware-ota-transfer <manifest> <firmware>` | PASS |
