# voice-keyboard-ble-customer-recovery-hardening 1.3 validation

Assignee: tai1; rework handoff: oai2
Date: 2026-05-29
Repo: Listener-Type
Branch: ai/tai1-voice-keyboard-ble-customer-recovery-hardening-1.3

## Summary

Implemented the customer-facing Listener BLE recovery UI layer for Overview:

- Added `src/lib/bleRecoveryUi.ts` to map probe/runtime/device-health/repair outputs to customer-actionable states.
- Added tests for ready, reconnecting, needs wake key, needs Bluetooth, needs re-pair, diagnostics available, and OTA reconnecting.
- Updated Overview to avoid showing raw repair messages from GATT/CCCD/HRESULT/COM failures as the primary UI message.
- Updated `EmbeddedBleStatusPanel` to render mapped labels/messages/actions, re-pair steps, Repair connection, Open Windows Bluetooth, Use microphone, and diagnostic export status.
- Added localized recovery labels/messages for zh-CN, zh-TW, en, ja, and ko.

## Manual UI Copy Review

Reviewed mapped UI states against the 1.3 acceptance criteria:

| Scenario | UI state | Primary customer action |
|---|---|---|
| No device / no BLE evidence | `needsWakeKey` | Press KEY4/wake key, retry; Open Windows Bluetooth if repair result requests it |
| Sleeping device | `needsWakeKey` | Press KEY4/wake key, then refresh or repair |
| Stale Windows pairing / GATT cache | `needsRePair` | Remove Listener in Windows Bluetooth, pair again, then refresh |
| DIS firmware revision missing | `diagnosticsAvailable` | Export diagnostics and contact support |
| Background listener recovering | `reconnecting` | Wait for automatic recovery, then repair if it does not recover |
| OTA reconnecting/reboot window | `otaReconnecting` | Wait for device return, then refresh |
| Bluetooth service/access problem | `needsBluetooth` | Turn on/check Windows Bluetooth, reconnect, retry |

The primary status text no longer uses raw GATT, CCCD, HRESULT, WinRT, or COM-port evidence. That evidence remains available through diagnostic export.

## Diagnostic Export Review

The existing customer-facing diagnostic export is reachable from the BLE recovery panel without SDK, serial monitor, or COM-port instructions. The exported package already includes BLE triage fields:

- `ble.diagnosticSnapshot` with platform, configured device address, audio/OTA service snapshots, DIS service UUID, firmware snapshot, and BLE errors.
- `ble.failureTaxonomy` with classified BLE failures.
- `ble.backgroundListenerActive`, `ble.backgroundListenerReady`, `ble.backgroundListenerGeneration`.
- `ble.wakeRecovery`, `ble.recentDisconnectReason`, `ble.reconnectAttempts`, `ble.notifySubscriptionState`.
- `ble.deviceAddress`, `ble.firmwareVersion`, `ble.batteryPercent`, `ble.capabilities`.
- Privacy flags confirming audio, transcripts, final text, and credential values are excluded.

## Validation Commands

- PASS: `npm test`
- PASS: `npm run build`
- PASS: `git diff --check`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`

Notes:

- `npm run build` completed with the existing Vite chunk-size warning for the main bundle.
- `git diff --check` completed cleanly; Git printed only CRLF normalization warnings.
- A `node_modules` junction was created in this worktree pointing to the main Listener-Type dependency directory for validation only.

## OAI2 Rework After Review

Reviewer feedback on 2026-05-29 found that stale runtime BLE errors could override a successful foreground probe or a recovered repair result, leaving the customer UI on `needsRePair` after recovery had already succeeded.

Rework completed:

- `selectBleRecoveryUiState` now treats `probeStatus === 'ok'` and `lastRepairResult.recovered` as authoritative ready states before considering stale runtime errors.
- `Overview` updates local BLE runtime state from a repair result immediately, so the panel does not wait for an asynchronous refresh before clearing stale guidance.
- Added regression coverage for probe-ok plus stale runtime and repair-recovered plus stale runtime.

Rework validation:

- PASS: `npm test`
- PASS: `npm run build`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check`

Notes:

- `npm run build` still reports the existing Vite chunk-size warning for the main bundle.
- `git diff --check` completed cleanly; Git printed only CRLF normalization warnings.
