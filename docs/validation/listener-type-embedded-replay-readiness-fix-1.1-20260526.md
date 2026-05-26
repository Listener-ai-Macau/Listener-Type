# Listener-Type Embedded Replay Readiness Fix 1.1 Validation

Date: 2026-05-26

## Script Issues Found And Fixed

- `run_ble_stream_smoke.ps1` waited for notify readiness, but serial replay could
  still toggle before firmware logged `stream_ready`. It now waits for
  `stream_ready`, records wait/retry counters, and retries once after a
  `BLE audio transport not ready` rejection.
- `run_ble_stream_smoke.ps1` calculated transcript accuracy but did not use it
  for final status, allowing a transcript mismatch to report PASS. Non-warning
  profiles now fail below threshold; warning-only profiles report WARNING.
- `run_ble_stream_smoke.ps1` and `run_embedded_audio_file_smoke.ps1` attempted
  `cargo build` in fresh worktrees without first checking Tauri `dist`, producing
  a late and unclear build error. Both scripts now fail early with an actionable
  message or accept `-ListenerExe`.
- `run_embedded_audio_file_smoke.ps1` recorded `missing_packets` without gating
  status. It now reports WARNING by default above `-MaxMissingPackets`, can fail
  with `-FailOnMissingPackets`, and gates transcript accuracy when `-Sentence` or
  `-ExpectedText` is supplied.

## Validation

- PowerShell parse:
  - `run_ble_stream_smoke.ps1`: PASS.
  - `run_embedded_audio_file_smoke.ps1`: PASS.
- `git diff --check`: PASS, with expected CRLF conversion warnings only.
- Missing-frontend precheck:
  - `run_ble_stream_smoke.ps1` without `-ListenerExe` and without `dist`: PASS,
    throws the expected actionable error before serial/hardware access.
  - `run_embedded_audio_file_smoke.ps1` without `-ListenerExe` and without
    `dist`: PASS, throws the same expected actionable error before launching the
    app.

## Hardware Status

A prior run using an external Listener-Type executable reached the serial
readiness path (`stream_ready=true`, no `transport_not_ready` rejection), but ASR
returned an empty transcript in that host/app environment. That run is useful
for script diagnosis but is not counted as the final product-chain PASS. The
full COM5 smoke should be rerun once a current Listener-Type exe/frontend is
available in the worktree or explicitly supplied with `-ListenerExe`.

## Hardware Rerun

- `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\embedded_audio_replay\run_ble_stream_smoke.ps1 -TriggerMode serial-toggle -Port COM5 -DeviceName listener -BluetoothAddress 14C19F48FE72 -TimeoutMs 90000 -VerifyHistory -ListenerExe C:\Users\Billy\Desktop\listener\Listener-Type\src-tauri\target\debug\listener-type.exe -Sentence "你好，开始测试。" -ExpectedText "你好，开始测试。" -PlaybackVolumePercent 90`: PASS.
- Report: `artifacts\embedded_stream_smoke\ble-stream-smoke.20260526-182150.json`.
- Serial log: `artifacts\embedded_stream_smoke\ble-stream-smoke-20260526-182137.serial.log`.
- Recording archive: `C:\Users\Billy\AppData\Roaming\Listener Type\recordings\87f2a0df-fe8a-45f7-9fde-444cf7a28340.wav`.
- Key results: `stream_ready=True`, `streaming_queued=True`, `transport_not_ready=False`, `record_start_rejected=False`, `recording_start_seen=True`, `recording_stop_seen=True`, `missing_packets=0`, `accuracy=1`, `history_session.id=87f2a0df-fe8a-45f7-9fde-444cf7a28340`.

## Rework: Foreground BLE Probe Recovery

User log diagnosis showed the remaining failure was not only a stale app build:

- Running app at the time of diagnosis was `C:\Users\Billy\Desktop\listener\Listener-Type\src-tauri\target\release\listener-type.exe`, so it did not contain this worktree's new fix.
- `422aa9d` on `main` skips a foreground BLE path probe only when a background listener cancel flag is present. That protects an active background capture, but it can still report success before the background listener has actually re-enabled CCCD notify after a foreground probe.
- Firmware/serial evidence from current matrix artifacts showed the failure shape: after capture cleanup the host disables CCCD, and firmware logs `notify=0` / `mtu_ready`; without a later `attr_handle=3 notify=1` subscription event, the next recording can be rejected as transport-not-ready.

Fix:

- Added a background Listener BLE `notify ready` state that flips only after the WinRT capture path successfully writes CCCD notify.
- Foreground `probe_embedded_audio_ble_subscription` now waits for the active background listener to be notify-ready instead of treating a merely armed cancel flag as ready.
- After a foreground probe that temporarily owns CCCD, the coordinator refreshes the background listener and waits for notify-ready before returning success. If CCCD is not restored, the probe returns an error instead of allowing the UI to show healthy while firmware is still `mtu_ready notify=0`.
- The serial replay helper now refuses to send `~VREC:TOGGLE` if `stream_ready` was not confirmed. After a transport-not-ready rejection, it waits for a fresh `stream_ready` line after the rejection before retrying once. `serial_report.stream_ready` now uses the target-state parser instead of a substring match.

Validation:

- PASS: `npm ci --prefer-offline --no-audit --no-fund`
- PASS: `npm run build`
- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml embedded_ble` (18 passed)
- PASS: `cargo check --manifest-path src-tauri\Cargo.toml`
- PASS: `npm run test -- --run`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: PowerShell parse for `tools\embedded_audio_replay\run_ble_stream_smoke.ps1`
- PASS: embedded Python block compile for `tools\embedded_audio_replay\run_ble_stream_smoke.ps1`
- PASS: `git diff --check` (CRLF warnings only)

Hardware note: no fresh hardware smoke was run after this rework because a release Listener-Type process from the main checkout was running. That process is useful evidence for the "old app build" question, but it is not the patched worktree binary.
