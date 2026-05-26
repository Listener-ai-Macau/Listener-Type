# Listener-Type OTA headless validation 1.1

Date: 2026-05-27T00:35:40+08:00

Worktree: `C:\Users\Billy\Desktop\listener\Listener-Type-wt-tai-listener-type-ota-headless-validation-1.1`

Branch: `ai/tai-listener-type-ota-headless-validation-1.1`

## Summary

Implemented a shared Rust firmware OTA package validator and headless release-gate runner for Listener-Type. The app CLI now accepts:

- `--firmware-ota-check <ota_manifest.json> <firmware_ota.bin>`
- `--firmware-ota-preflight <ota_manifest.json> <firmware_ota.bin>`
- `--firmware-ota-transfer <ota_manifest.json> <firmware_ota.bin>`

The firmware OTA CLI path runs before Tauri UI initialization and exits with a process status code, so `listener-type --firmware-ota-*` can be used as a headless release-gate command without opening the app UI.

## Package Evidence

Validated OTA package:

- Path: `C:\Users\Billy\Desktop\listener\voice-keyboard-firmware\.cache\ota_firmware\listener-ota-3afb9d9-20260526-230523`
- Firmware version: `3afb9d9`
- Size: `640080`
- SHA256: `52842950fe71db0541d64782df6b5a85b47dea12e4c55abdb9c376cb325567d7`
- JSON report: `tests\artifacts\ota_headless_validation\package-report.json`

## Validation

| Command | Result |
|---|---|
| `npm ci` | PASS, installed local worktree dependencies. |
| `npm run build` | PASS, `tsc && vite build`; generated ignored `dist/` needed by Tauri macros. |
| `npm run test:firmware-ota` | PASS. |
| `cargo test --manifest-path tools\firmware_ota_headless\Cargo.toml` | PASS, 7 tests. |
| `cargo test --manifest-path src-tauri\Cargo.toml firmware_ota --lib` | PASS, 11 tests. |
| `cargo check --manifest-path src-tauri\Cargo.toml` | PASS. |
| `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check` | PASS. |
| `git diff --check` | PASS; only CRLF normalization warnings printed. |
| `cargo run --manifest-path tools\firmware_ota_headless\Cargo.toml -- --manifest <package>\ota_manifest.json --firmware <package>\firmware_ota.bin --desktop-version 1.3.3 --hardware keyboard-v1 --json-out tests\artifacts\ota_headless_validation\package-report.json` | PASS, package/hash/schema/GATT boundary valid. |
| `cargo run --manifest-path src-tauri\Cargo.toml -- --firmware-ota-check <package>\ota_manifest.json <package>\firmware_ota.bin` | PASS, app CLI ran headless and printed `firmware_ota_result_json` without UI initialization. |
| Helper command with `--desktop-version 0.9.0` | PASS expected failure, exit code 1 with `Listener Type 0.9.0 is older than required 1.3.3.` |
| Helper command with `--preflight` | PASS expected failure, exit code 1 with standalone no-BLE-backend blocker. |

## Hardware Scope

No real BLE transfer, flash, serial capture, or device preflight was run. Hardware resources `COM5` and `BLE-14C19F48FE72` were already locked by another workflow task, so this step validated package logic, command-line headless execution, negative preflight behavior, and the compiled transfer code path only.

The transfer implementation calls the existing Windows BLE OTA backend (`embedded_ble::transfer_firmware_ota`) and reports `bytesTransferred`, `transport`, and `confirmedVersion` through the shared headless report/IPC result. A real transfer still needs the hardware lock to be released before release-gate execution.
