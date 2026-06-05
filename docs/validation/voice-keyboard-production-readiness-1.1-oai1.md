# voice-keyboard-production-readiness/1.1 validation

Agent: oai1

Date: 2026-06-05

## Scope

Capsule Recording window operations were throttled without throttling `capsule:state`
events. This keeps audio level events flowing to the frontend while reducing repeated
native window show/position work during long BLE recording.

## Commands

- `npm ci`
- `npm run build`
- `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- `CARGO_TARGET_DIR=target-oai1-validation cargo test --manifest-path src-tauri\Cargo.toml capsule_ --lib`
- `CARGO_TARGET_DIR=target-oai1-validation cargo test --manifest-path src-tauri\Cargo.toml -- --test-threads=1`
- `aiw with-lock -Resource 'COM5,BLE-E99FCE38CC0D' -Run powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools\run_ble_stream_smoke.ps1 -Port COM5 -BluetoothAddress E99FCE38CC0D -DeviceName listener -SilentAudio -ExpectNoText -SilentAudioMs 30000 -PostPlaybackRecordMs 0 -TimeoutMs 90000 -RecordingStartTimeoutMs 5000 -NoNotificationTimeoutSeconds 12 -MaxMissingPackets 0 -FailOnMissingPackets -OutDir artifacts\voice-keyboard-production-readiness-1.1-oai1-ble-smoke-locked -ListenerExe target-oai1-validation\debug\listener-type.exe`

## Results

- Rust focused capsule tests: PASS, 7 passed.
- Full Rust tests: PASS, 501 passed, 0 failed.
- 30s BLE smoke under `COM5` and `BLE-E99FCE38CC0D` locks: PASS.
- BLE smoke report: `artifacts\voice-keyboard-production-readiness-1.1-oai1-ble-smoke-locked\ble-stream-smoke.20260605-124529.json`.
- BLE smoke recording window: 30.145s playback, 30.300s recording window.
- BLE missing packets: 0.
- Capsule visible during recording: yes.
- `[capsule] show request state=Recording` during `2026-06-05T04:44:47Z` to `2026-06-05T04:45:18Z`: 31 lines over 31s, 1.0/s.
