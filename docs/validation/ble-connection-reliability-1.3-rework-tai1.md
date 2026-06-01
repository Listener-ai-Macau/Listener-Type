# ble-connection-reliability 1.3 rework validation

Assignee: tai1
Date: 2026-06-01

## Reviewer gap closure

Reviewer requested evidence for a real BLE disconnect/reconnect cycle or a clearly justified deterministic equivalent, 5s reconnect timing, post-reconnect audio streaming, post-reconnect OTA identity, and UI capsule reconnect prompt evidence.

This rework adds `tools/verify_ble_reconnect_recovery.ps1`, which starts the real Listener-Type GUI background BLE listener against the paired `BLE-E80B41BCC9A1` hardware, injects a validation-only `Disconnected/transport_not_ready` signal into the same WinRT notify wait branch used by real device and GATT status disconnect handlers, and then verifies real hardware OTA identity plus BLE audio streaming after recovery.

The deterministic disconnect is intentionally limited to validation by `LISTENER_TYPE_BLE_VALIDATION_DISCONNECT_SIGNAL_FILE`. Normal users do not set this environment variable, and the production recovery path still uses the existing WinRT device/GATT status handlers.

## PASS run

Hardware lock resources:

- `COM5`
- `BLE-E80B41BCC9A1`

Command:

```powershell
$env:AI_AGENT_ID='tai1'; pwsh -NoProfile -File 'C:\Users\Billy\Desktop\listener\ai-collaboration-workflow\scripts\aiw.ps1' with-lock -Resource COM5,BLE-E80B41BCC9A1 -Run pwsh -NoProfile -File .\tools\verify_ble_reconnect_recovery.ps1 -Port COM5 -BluetoothAddress E80B41BCC9A1
```

Result artifact:

- `tests\artifacts\ble_reconnect_recovery\20260601-121243\ble-reconnect-recovery.json`
- `tests\artifacts\ble_reconnect_recovery\20260601-121243\ble-reconnect-recovery.md`
- `tests\artifacts\ble_reconnect_recovery\20260601-121243\listener-type.reconnect.log`
- `tests\artifacts\ble_reconnect_recovery\20260601-121243\stream-smoke\attempt-2\ble-stream-smoke.20260601-121332.json`

Summary:

- Reconnect report status: `PASS`
- Deterministic disconnect method: `validation-injected`
- Disconnect observed: `2026-06-01T04:12:50.2509849Z`
- Ready observed: `2026-06-01T04:12:52.0035935Z`
- Disconnect-to-ready: `1753 ms` against the `5000 ms` limit
- OTA identity before reconnect: `PASS`, address `E80B41BCC9A1`, connection `Connected`, primed cached sessions `8`
- OTA identity after reconnect: `PASS`, address `E80B41BCC9A1`, connection `Connected`, primed cached sessions `8`
- Post-reconnect audio stream: `PASS`
- Post-reconnect stream transcript/final text: `你好，开始测试。`
- Post-reconnect stream PCM bytes: `125440`
- Post-reconnect missing packets: `0`
- History embedded audio stats: reconstructed PCM `125440`, missing packet count `0`, terminal received `true`

UI capsule log evidence from the same run:

```text
2026-06-01T04:12:50.3372185Z [INFO] [embedded-ble] recovery capsule state=reconnecting emitted=true idle_after_ms=1800 message="Listener BLE 正在自动重连，回到范围后会恢复语音键。"
2026-06-01T04:12:51.9712504Z [INFO] [embedded-ble] recovery capsule state=reconnected emitted=true idle_after_ms=1600 message="Listener BLE 已自动重连，语音键可用。"
2026-06-01T04:12:52.040855Z [INFO] [capsule] show request state=Recording shown_no_activate=true
```

## Automated checks

- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `npm run test`
- PASS: `npm run build` (Vite reported the existing large chunk warning)
- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml embedded_ble --lib`
- PASS: PowerShell parser check for `tools\verify_ble_reconnect_recovery.ps1`
- PASS: `git diff --check`
