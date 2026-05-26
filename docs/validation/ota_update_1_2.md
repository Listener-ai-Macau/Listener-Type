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
- `git diff --check`: PASS.
  - Git printed line-ending normalization warnings only.

## Acceptance Self-Review

- Package input:
  - UI selects `ota_manifest.json` and `firmware_ota.bin`.
  - `src/lib/firmwareOta.ts` validates schema v1, package type, protocol,
    firmware capability, hardware revision, min desktop version, file size,
    and SHA256.
- User-visible state:
  - UI exposes idle/checking/ready/transferring/rebooting/verifying/success/
    failed/rolledBack labels.
- Preflight:
  - Library checks connected, recording active, transfer active, min desktop
    version, hardware mismatch, missing capability, same/non-newer version,
    low battery, and unknown power.
  - Rust transfer command also rejects non-idle dictation phase before BLE I/O.
- BLE boundary:
  - OTA uses a dedicated `listener_ble_ota` GATT service and does not reuse BLE
    audio notifications or HID reports.
  - The transfer command pauses the background embedded BLE listener for the OTA
    write and refreshes it afterwards.
- Failure recovery:
  - Manifest/hash mismatch, preflight/device rejection, BLE disconnect, and
    version-not-confirmed paths show retry/export diagnostics actions.

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
