# Voice Keyboard BLE Customer Recovery Hardening 1.1 Validation

Date: 2026-05-28
Agent: tai1

## Scope

Step 1.1 adds diagnostic capture and stable BLE failure classification for Listener-Type. It does not implement the full BLE ownership/recovery state machine; that remains in step 1.2.

## Implementation Evidence

- Added `BleFailureKind` and `classify_ble_failure` covering device missing, paired-but-disconnected, stale GATT service, CCCD protocol error, missing DIS firmware revision, background listener contention, OTA reboot window, and Windows Bluetooth service reset needed.
- Added `BleDiagnosticSnapshot` with Windows service selector/PnP entries for audio and OTA services, parsed Bluetooth address, configured address, service UUIDs, and live firmware OTA/DIS/battery/capability snapshot.
- Extended `exportDiagnosticPackage` schema to version 3 with `ble.failureTaxonomy`, `ble.diagnosticSnapshot`, `ble.backgroundListenerGeneration`, `ble.deviceAddress`, `ble.firmwareVersion`, `ble.batteryPercent`, and `ble.capabilities`.
- Added a read-only coordinator accessor for background listener generation.
- Documented support findings in `docs/features/ble-customer-recovery-diagnostics.md`.

## Recovery Script Findings

The existing local support prototype `tools/recover_listener_ble_ota.ps1` was reviewed from the main Listener-Type working tree because it is currently an untracked local support script there. The script captures Windows Bluetooth PnP state, optionally stops Listener Type, sends serial `~OTA:ABORT`, `~OTA:STATUS`, `~POWER:STATUS`, `~DIAGLOG:COUNT`, optionally restarts Windows Bluetooth, probes GATT maintain-connection by address, then runs Listener Type headless OTA preflight/transfer.

Product-side automatic recovery is appropriate for short retry, session reopen, background-listener coordination, address-based reopen, and OTA reboot-window waits. User/support action remains necessary for physical wake, Windows Bluetooth service reset, re-pair, and firmware DIS/readiness gaps.

## Real BLE Exception / Recovery Artifact

Windows PnP/GATT snapshot on this machine showed the Listener device present at `14C19F48FE72` while stale service instances still existed:

```text
Status=OK      FriendlyName=listener              InstanceId=BTHLE\DEV_14C19F48FE72\...
Status=OK      Service=710AF845-...-3BC3091A      InstanceId=...DEV_VID&0216C0..._14C19F48FE72...
Status=OK      Service=710AF845-...-3BC3092A      InstanceId=...DEV_VID&0216C0..._14C19F48FE72...
Status=OK      Service=0000180A DIS               InstanceId=...DEV_VID&0216C0..._14C19F48FE72...
Status=Unknown Service=710AF845-...-3BC3091A      InstanceId=..._14C19F48FE72...
Status=Unknown Service=710AF845-...-3BC3092A      InstanceId=..._14C19F48FE72...
Status=Unknown Service=0000180A DIS               InstanceId=..._14C19F48FE72...
```

This is the real stale/Unknown GATT evidence this step needs to preserve for support triage. The new product diagnostic snapshot records both OK and Unknown service entries with service UUID, instance id, parsed device address, plus live firmware/battery/capability fields when the app can read them.

The support bundle collector also ran successfully:

```text
tools/collect_ai_diagnostics.ps1
bundle: tests/artifacts/ai_diagnostics_ble_customer_recovery_1_1/20260528T022427Z/diagnostic_bundle.json
diagnostic_bundle sha256: f9ae6dc796afe83e5de003c2ce79a034e47ea7d1776dffd39de12263b803d60a
recent_warning_error_refs: 45
ble_audio_artifacts: 5
```

During validation, the first `npm run build` failed before Vite because `node_modules` was not installed in the new worktree:

```text
'tsc' is not recognized as an internal or external command
```

This was a local dependency/setup failure, not a BLE runtime failure. `npm ci --no-audit --no-fund` installed the ignored dependency tree, and `npm run build` then passed. Full Windows real-device recovery matrix remains step 1.5.

## Validation

- PASS: `cargo test --manifest-path src-tauri\Cargo.toml embedded_ble --lib` (24 passed)
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml diagnostic_ble_failure_taxonomy --lib` (1 passed)
- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- PASS: `npm ci --no-audit --no-fund`
- PASS: `npm run build` (Vite emitted the existing >500 kB chunk-size warning)
- PASS: `git diff --check`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml firmware_ota --lib` (13 passed)

## Notes

No BLE audio capture or OTA transfer was run for this step. The real-device matrix, timing, and user-action measurements are explicitly deferred to step 1.5.
