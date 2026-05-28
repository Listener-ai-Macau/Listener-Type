# BLE Customer Recovery Diagnostics

Listener Type exports BLE recovery evidence through **Settings -> About -> Export diagnostic package**. The package is metadata-only: it does not include audio recordings, transcripts, inserted text, API keys, access tokens, or OAuth tokens.

## Failure Taxonomy

The diagnostic package classifies BLE errors into stable support categories:

| Category | Meaning | Automatic recovery | User/support action |
|---|---|---|---|
| `deviceMissing` | No Listener BLE service or paired Listener candidate was found. | No | Wake the device, confirm Windows pairing, then re-pair if needed. |
| `pairedButDisconnected` | Windows knows the device, but the GATT path is unreachable, disconnected, or timing out. | Yes | Wait for reconnect; press the wake key if it stays disconnected. |
| `staleGattService` | Windows returned a stale or inconsistent GATT/service cache. | Yes | Retry after Listener Type refreshes the path; re-pair if stale services persist. |
| `cccdProtocolError` | Notify subscription or CCCD write failed. | Yes | Reopen notify subscription; restart Type if repeated. |
| `missingDisFirmwareRevision` | DIS firmware revision is unavailable from the live snapshot. | No | Treat as firmware readiness evidence; do not approve release until DIS is stable. |
| `backgroundListenerContention` | Foreground status/probe, background audio listener, OTA, or diagnostics are competing for BLE ownership. | Yes | Pause the competing operation and retry through the shared listener path. |
| `otaRebootWindow` | Device is rebooting or rediscovering after OTA while version confirmation is still pending. | Yes | Wait for the reboot window, then refresh device status. |
| `windowsBluetoothServiceResetNeeded` | Windows Bluetooth radio/service state appears stuck. | No | Toggle Windows Bluetooth or restart the Bluetooth Support Service, then retry. |

## Diagnostic Fields

`exportDiagnosticPackage` writes these BLE support fields:

- `ble.backgroundListenerGeneration`: current background listener generation.
- `ble.deviceAddress`: configured or discovered Listener Bluetooth address when available.
- `ble.firmwareVersion`, `ble.batteryPercent`, `ble.capabilities`: readable firmware fields from the live OTA/DIS/battery snapshot.
- `ble.diagnosticSnapshot.audioServices` and `ble.diagnosticSnapshot.otaServices`: Windows PnP/GATT service selector entries with service UUID, device name, instance id, and parsed Bluetooth address.
- `ble.diagnosticSnapshot.errors`: snapshot collection failures or live firmware snapshot details.
- `ble.failureTaxonomy`: deduplicated classifications from listener errors, wake recovery state, recent log errors, and snapshot errors.

## Support Script Findings

The development recovery script prototype (`tools/recover_listener_ble_ota.ps1` in the local support workspace) follows this sequence:

1. Capture Windows Bluetooth PnP entries for Listener, DIS, battery, HID, audio, and OTA UUIDs.
2. Stop Listener Type unless the operator explicitly keeps it running.
3. Send serial recovery/status commands: `~OTA:ABORT`, `~OTA:STATUS`, `~POWER:STATUS`, and `~DIAGLOG:COUNT`.
4. Optionally restart Windows Bluetooth using the firmware repo support script.
5. Probe/maintain the Listener GATT connection by Bluetooth address.
6. Run Listener Type headless OTA preflight, and optionally OTA transfer.

The product should automate short retry, session reopen, background-listener coordination, address-based reopen, and OTA reboot-window waits. User action is still required for device wake, Windows Bluetooth service reset, re-pair, and firmware readiness gaps such as missing DIS firmware revision.
