# Voice Keyboard OTA 1.2 Validation

Plan: `voice-keyboard-ota-update`
Step: `1.2 Desktop OTA package validation and BLE transfer UX`
Repo: `Listener-Type`
Branch: `ai/oai-voice-keyboard-ota-update-1.2`

## Automated Checks

- `npm test`: PASS.
  - Existing capsule preview/layout/device-health tests passed.
  - New `test:firmware-ota` validates manifest parsing, SHA256/size errors,
    hardware/min-desktop blockers, preflight blockers, state transitions, and
    BLE audio/HID boundary constants.
- `npm run build`: PASS.
  - `tsc` and `vite build` passed.
  - Vite reported the existing large chunk warning.
- `npm run verify`: PASS.
  - `tsc --noEmit`, brand check, dark-mode check, unit tests, and Vite build.
- `cargo check` in `src-tauri`: PASS.
- `cargo test firmware_ota` in `src-tauri`: PASS.
  - Covers DIS firmware revision normalization and empty-version handling used
    by the Tauri OTA confirmation path.
- `git diff --check`: PASS.
  - Git printed line-ending normalization warnings only.
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`: PASS.

## Rework Evidence

- Rejected hardcoded preflight state was removed.
  - `FirmwareOtaPanel` now calls `get_firmware_ota_preflight_snapshot` before
    showing readiness and again immediately before transfer.
  - The UI no longer calls `cancelDictation()` before OTA. Active recording
    stays active and blocks the update through preflight and the Rust transfer
    command.
  - Device readiness comes from the Tauri BLE/DIS snapshot instead of fixed
    hardware/capability/power values.
- Firmware manifest parsing now supports both the original desktop schema v1
  and the firmware package schema v2 emitted by `tools/package_ota_firmware.ps1`.
  Schema v2 normalizes `firmware`, `requirements`, `ble_identity`, `rollback`,
  and `recovery` into the desktop transfer contract.
- `test:firmware-ota` includes a schema v2 sample package and negative cases for
  missing `ble_identity`, incomplete rollback metadata, incomplete recovery
  metadata, plus the unknown-device-status OTA blocker.
- Real Tauri success path was reworked after review:
  - `transfer_firmware_ota_ble` now polls the Listener BLE/DIS firmware
    revision after a successful transfer and returns `confirmedVersion`.
  - The desktop UI only emits `versionNotConfirmed` after the backend
    confirmation window returns no firmware version or a different version.
  - Version matching accepts a DIS `v` prefix but does not accept dirty/dev
    suffixes as the release version.

## Acceptance Self-Review

- Package input:
  - UI selects `ota_manifest.json` and `firmware_ota.bin`.
  - `src/lib/firmwareOta.ts` validates schema v1 and firmware tooling schema v2,
    package type, protocol, firmware capability, hardware revision, min desktop
    version, file size, and SHA256.
- User-visible state:
  - UI exposes idle/checking/ready/transferring/rebooting/verifying/success/
    failed/rolledBack labels.
- Preflight:
  - Library checks connected, recording active, transfer active, min desktop
    version, unknown hardware status, hardware mismatch, missing capability,
    same/non-newer version, low battery, and unknown power.
  - Frontend reads the Tauri preflight snapshot and blocks active recording
    instead of silently cancelling it.
  - Rust transfer command also rejects non-idle dictation phase before BLE I/O.
- BLE boundary:
  - OTA uses a dedicated `listener_ble_ota` GATT service and does not reuse BLE
    audio notifications or HID reports.
  - The transfer command pauses the background embedded BLE listener for the OTA
    write and refreshes it afterwards.
- Failure recovery:
  - Manifest/hash mismatch, preflight/device rejection, BLE disconnect, and
    version-not-confirmed paths show retry/export diagnostics actions.
  - The real Tauri transfer path can now reach `success` when the backend
    confirms the device firmware version after reboot.

## Manual UI Copy Review

- The panel copy keeps firmware update text user-facing: choose package, update,
  failed, rolled back, retry, export diagnostics.
- Internal details such as partitions and otadata are not shown in the main
  panel. The only protocol detail shown explains that OTA uses a separate BLE
  channel, not BLE audio/HID.

## Deferred Hardware Evidence

This step implements desktop validation, UX state handling, and an actual
Windows GATT write path. Real device upgrade/rollback confirmation is covered
by plan step `1.4`, which requires a hardware lock and records the end-to-end
version and rollback evidence.
