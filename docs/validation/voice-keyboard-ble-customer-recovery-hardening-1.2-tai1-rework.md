# voice-keyboard-ble-customer-recovery-hardening 1.2 tai1 rework

Date: 2026-05-29
Agent: tai1
Worktree: `Listener-Type-wt-tai1-voice-keyboard-ble-customer-recovery-hardening-1.2`

## Scope

- Cherry-picked the previous `oai1` BLE repair action implementation into a fresh tai1 step branch.
- Reworked the failed repair-result mapping called out by review:
  - `StaleGattService` remains classified as automatic recovery during ordinary transient detection.
  - After the customer-facing repair action has already failed, stale GATT/bond-cache cases now return `userActionRequired=true` and `openBluetoothSettings=true`.
  - Transient paired/disconnected repair failures remain automatic and do not force Bluetooth settings.
- Added Rust unit coverage for both stale GATT final repair failure and transient disconnect repair failure.

## Validation

PASS:

- `npm run build`
- `$env:FOUNDRY_NATIVE_OVERRIDE_DIR = Join-Path $env:APPDATA 'Listener Type\models\foundry-local\runtime'; cargo test --manifest-path src-tauri\Cargo.toml repair_failure --lib`
- `$env:FOUNDRY_NATIVE_OVERRIDE_DIR = Join-Path $env:APPDATA 'Listener Type\models\foundry-local\runtime'; cargo test --manifest-path src-tauri\Cargo.toml embedded_ble --lib`
- `$env:FOUNDRY_NATIVE_OVERRIDE_DIR = Join-Path $env:APPDATA 'Listener Type\models\foundry-local\runtime'; cargo test --manifest-path src-tauri\Cargo.toml firmware_ota --lib`
- `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- `git diff --check`
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- `npm run test:firmware-ota`
- `npm test`

Notes:

- The first narrow cargo run without `FOUNDRY_NATIVE_OVERRIDE_DIR` failed before compiling this crate because `foundry-local-sdk` tried to download a missing native NuGet package and the host network failed. The local prepared Foundry runtime under `%APPDATA%\Listener Type\models\foundry-local\runtime` was then used for all cargo validation.
- The fresh worktree initially had no frontend `dist`; `npm run build` generated it before cargo tests that invoke Tauri context generation.
- `node_modules` is a local junction to the main Listener-Type dependency install and is ignored by git.
