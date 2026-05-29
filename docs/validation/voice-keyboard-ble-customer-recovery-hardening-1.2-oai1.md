# voice-keyboard-ble-customer-recovery-hardening 1.2 validation

Date: 2026-05-28
Agent: oai1
Worktree: `Listener-Type-wt-oai1-voice-keyboard-ble-customer-recovery-hardening-1.2`

## Scope

- Merged the relevant cleaned stash changes for Listener-Type BLE ownership and recovery:
  - `src-tauri/src/commands.rs`
  - `src-tauri/src/coordinator.rs`
  - `src-tauri/src/embedded_ble.rs`
- Rebasing/merging was completed against `origin/main` at `54c1399` so 1.1 diagnostics and accepted OTA stabilization remain in the step branch.

## Behavior Covered

- Firmware OTA now acquires a coordinator-level BLE ownership flag before transfer and releases it before refreshing the background listener.
- Firmware OTA ownership is exclusive; a second concurrent OTA transfer is rejected instead of racing the same GATT path.
- Beginning OTA pauses the existing background listener, and later status/background refresh calls are skipped while OTA owns the BLE GATT path.
- Foreground BLE probes now coordinate with the background listener: if the background notify subscription is ready, the foreground probe reuses that state; if the background listener is expected but not active, it starts recovery instead of opening a competing GATT path.
- Type now exposes a callable `repair_embedded_ble_connection` action for a customer-facing repair button. It reuses a ready listener when available, otherwise releases the existing BLE session, restarts listener recovery, waits for notify readiness, refreshes firmware/DIS/OTA metadata, classifies the failure, and reports when Windows Bluetooth settings/re-pairing is required.
- Overview wires the repair action into the Listener BLE status panel so customer-facing UI has a concrete backend action rather than only a passive refresh.
- Audio and OTA target discovery can fall back from service selectors to device-address based reopen paths, including configured address and paired-device selectors.
- Bluetooth service discovery uses uncached then cached modes for stale Windows GATT cache recovery.
- HID/keyboard availability remains on the existing hotkey capability path; BLE audio/OTA recovery errors are tracked through embedded BLE runtime and wake recovery state.
- A CLI probe (`--probe-embedded-audio-ble-subscription`) records background/foreground coordination behavior for manual regression evidence.

## Validation

PASS:

- `npm ci`
- `npm run build`
- `cargo test --manifest-path src-tauri\Cargo.toml embedded_ble --lib`
- `cargo test --manifest-path src-tauri\Cargo.toml firmware_ota --lib`
- `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- `git diff --check`
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- `npm run test:firmware-ota`
- `cargo test --manifest-path src-tauri\Cargo.toml embedded_ble --lib`
- `cargo test --manifest-path src-tauri\Cargo.toml firmware_ota --lib`
- `npm run build`
- `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- `git diff --check`
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`

Notes:

- The first cargo run failed before test execution because the fresh worktree had no `dist`; `npm run build` generated it and both required cargo filters passed afterward.
- After merging `origin/main`, the required commands were rerun and passed again on the merged branch.
- No hardware resource locks were held during this automated validation.
- A real-device probe was run on 2026-05-28. The probe did not recover the Windows BLE notify path because the current host/device state repeatedly failed CCCD writes (`GattCommunicationStatus(1)` and `HRESULT(0x800704C7)`), but it did show the intended ownership behavior: the foreground probe delegated to / waited on the background listener rather than racing it with a separate GATT subscription.
- Remaining hardware/product matrix coverage belongs to 1.5; this step provides the shared state machine and callable repair action.
