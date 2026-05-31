# listener-type-ble-idle-disconnect-ux 1.1 validation

Assignee: tai1
Date: 2026-05-31

## Automated checks

- PASS: `npm run test:device-health`
- PASS: `npm run test:ble-recovery-ui`
- PASS: `node --experimental-strip-types src/lib/providerSetup.test.ts`
- PASS: PowerShell parser check for `tools/embedded_audio_replay/run_ble_stream_smoke.ps1`
- PASS: `npm run build` (`vite` emitted the existing large chunk warning)
- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib embedded_ble`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib coordinator::tests::embedded_ble`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib repair_failure`
- PASS: `git diff --check` (CRLF warnings only)

## Real-device smoke

Hardware lock resources:

- `COM5`
- `BLE-14C19F48FE72`

Both hardware runs started from `ensure_ble_hid_connection: status=Disconnected gatt=Unreachable services=0 sessions=0`, then Listener-Type registered notify and recovered the BLE audio path without `transport_not_ready`.

### Voice recording after reconnect

Command:

```powershell
pwsh -NoProfile -File ..\ai-collaboration-workflow\scripts\aiw.ps1 with-lock -Resource COM5,BLE-14C19F48FE72 -Run powershell -NoProfile -ExecutionPolicy Bypass -File tools\embedded_audio_replay\run_ble_stream_smoke.ps1 -TriggerMode serial-toggle -Port COM5 -DeviceName listener -BluetoothAddress 14C19F48FE72 -TimeoutMs 90000 -VerifyHistory -ListenerExe src-tauri\target\debug\listener-type.exe -Sentence "你好，开始测试。" -ExpectedText "你好，开始测试。" -PlaybackVolumePercent 80 -NoResetBeforeCapture -AudioProfile punctuation
```

Result artifact:

- `artifacts\embedded_stream_smoke\ble-stream-smoke.20260531-121238.json`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260531-121217.serial.log`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260531-121217.wav`

Summary:

- status: PASS
- trigger: `serial-toggle`
- notify enabled: true
- stream ready: true
- `transport_not_ready`: false
- recording start/stop confirmed: true/true
- PCM bytes: 122240
- missing packets: 0
- transcript/final text: `你好，开始测试。`
- history session: `75303353-cdbc-41d4-a226-10c1718b8e00`
- recording archive: `%APPDATA%\Listener Type\recordings\75303353-cdbc-41d4-a226-10c1718b8e00.wav`

### Cancel after reconnect

Command:

```powershell
pwsh -NoProfile -File ..\ai-collaboration-workflow\scripts\aiw.ps1 with-lock -Resource COM5,BLE-14C19F48FE72 -Run powershell -NoProfile -ExecutionPolicy Bypass -File tools\embedded_audio_replay\run_ble_stream_smoke.ps1 -TriggerMode serial-cancel -Port COM5 -DeviceName listener -BluetoothAddress 14C19F48FE72 -TimeoutMs 90000 -VerifyHistory -ListenerExe src-tauri\target\debug\listener-type.exe -Sentence "你好，取消测试。" -ExpectedText "你好，取消测试。" -PlaybackVolumePercent 80 -NoResetBeforeCapture -AudioProfile punctuation -ExpectNoText
```

Result artifact:

- `artifacts\embedded_stream_smoke\ble-stream-smoke.20260531-122122.json`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260531-122102.serial.log`
- `artifacts\embedded_stream_smoke\ble-stream-smoke-20260531-122102.wav`

Summary:

- status: PASS
- trigger: `serial-cancel`
- notify enabled: true
- stream ready: true
- `transport_not_ready`: false
- recording start confirmed: true
- cancel completed: true
- expected stream failure: `嵌入式音频会话已取消`
- PCM bytes: 0
- missing packets: 0
- transcript/final text: empty
- history session: none
- recording archive: none

## Diagnostic export

PASS: `pwsh -NoProfile -File .\tools\collect_ai_diagnostics.ps1 -OutputDir .\tests\artifacts\ai_diagnostics_ble_idle_1_1`

Artifacts:

- `tests\artifacts\ai_diagnostics_ble_idle_1_1\20260531T041346Z\manifest.json`
- `tests\artifacts\ai_diagnostics_ble_idle_1_1\20260531T041346Z\diagnostic_bundle.json`

## Notes

- `artifacts\`, `dist\`, `node_modules\`, `src-tauri\target\`, and `tests\artifacts\` are ignored build/test outputs.
- One earlier `serial-toggle` smoke produced a real BLE/audio/history pass but failed the strict transcript threshold due ASR mismatch; the later `punctuation` profile run passed with exact transcript.
- The smoke script was adjusted so `-ExpectNoText` cancel verification does not treat old history rows or pre-cancel ASR log fragments as new cancelled-session output.
