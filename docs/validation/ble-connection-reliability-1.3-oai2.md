# ble-connection-reliability 1.3 validation

Assignee: oai2
Date: 2026-06-01

## Implementation summary

- Added WinRT BLE connection and GATT session status callbacks to the embedded BLE notify loop so out-of-range disconnects wake the loop immediately instead of waiting on a silent notification stream.
- Classified embedded BLE link-loss errors as automatically recoverable and moved the fast reconnect path to a 500 ms retry, with transient retry delays capped at 5 seconds.
- Emitted short UI capsule reconnecting/reconnected messages from the background listener and refreshed the Overview embedded BLE runtime status while the embedded BLE source is active.
- Added `tools/run_ble_stream_smoke.ps1` as the plan-level wrapper for the existing `tools/embedded_audio_replay/run_ble_stream_smoke.ps1` smoke script.

## Automated checks

- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml`
- PASS: `npm ci`
- PASS: `npm run build` (Vite emitted the existing large chunk warning)
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml embedded_ble --lib`
- PASS: `npm run test`
- PASS: `git diff --check` (CRLF warnings only)
- PASS: `npm run verify`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: PowerShell parser check for `tools/run_ble_stream_smoke.ps1`
- PASS: PowerShell parser check for `tools/embedded_audio_replay/run_ble_stream_smoke.ps1`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib` (425 tests)

## Hardware smoke

Required plan command:

```powershell
pwsh -NoProfile -File .\tools\run_ble_stream_smoke.ps1
```

### PASS run

Hardware lock resources:

- `COM5`
- `BLE-E80B41BCC9A1`

Command:

```powershell
$env:AI_AGENT_ID='oai2'; pwsh -NoProfile -File 'C:\Users\Billy\Desktop\listener\ai-collaboration-workflow\scripts\aiw.ps1' with-lock -Resource COM5,BLE-E80B41BCC9A1 -Run pwsh -NoProfile -File .\tools\run_ble_stream_smoke.ps1 -Port COM5 -BluetoothAddress E80B41BCC9A1 -TimeoutMs 90000 -NotifyReadyTimeoutSeconds 35 -VerifyHistory -AudioProfile punctuation -Sentence '你好，开始测试。' -ExpectedText '你好，开始测试。' -PlaybackVolumePercent 80
```

Result artifacts:

- `artifacts\embedded_stream_smoke\ble-stream-smoke.20260601-105507.json`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260601-105446.serial.log`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260601-105446.trace.log`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260601-105446.wav`

Summary:

- status: PASS
- trigger: `serial-toggle`
- BLE ensure status: `Connected`, GATT `Success`, services `8`, sessions `8`
- notify enabled: true
- stream ready: true
- `transport_not_ready`: false
- recording start/stop confirmed: true/true
- PCM bytes: 125440
- missing packets: 0
- transcript/final text: `你好，开始测试。`
- history session: `e6d9da27-7e0b-4281-a2ba-e649d9f290d7`
- embedded stats: received/reconstructed PCM 125440/125440, missing packets 0, terminal received true
- recording archive: `%APPDATA%\Listener Type\recordings\e6d9da27-7e0b-4281-a2ba-e649d9f290d7.wav`

### Earlier blocked attempt

An earlier smoke attempt used stale resources and was blocked before the working hardware address was visible:

```powershell
$env:AI_AGENT_ID='oai2'; pwsh -NoProfile -File 'C:\Users\Billy\Desktop\listener\ai-collaboration-workflow\scripts\aiw.ps1' with-lock -Resource COM3,BLE-DCB4D91112CE -Run pwsh -NoProfile -File .\tools\run_ble_stream_smoke.ps1
```

Result: BLOCKED by unavailable local serial hardware. The workflow lock was acquired and released, and the smoke script built the debug app, but serial reset failed because `COM3` was not present:

```text
serial.serialutil.SerialException: could not open port 'COM3': FileNotFoundError(2, 'The system cannot find the file specified.', None, 2)
serial reset failed on COM3
```

Additional check:

```powershell
Get-CimInstance Win32_SerialPort
```

No serial ports were visible on this machine at validation time.

Failed smoke artifacts were generated under ignored output paths:

- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260601-104501.trace.log`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260601-104501.wav`
