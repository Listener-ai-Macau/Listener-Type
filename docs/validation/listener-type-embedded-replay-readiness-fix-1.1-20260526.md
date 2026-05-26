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
