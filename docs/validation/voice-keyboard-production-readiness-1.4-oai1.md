# voice-keyboard-production-readiness/1.4 validation

Agent: oai1

Date: 2026-06-07

Worktree: `Listener-Type-wt-oai1-voice-keyboard-production-readiness-1.4`

Branch: `ai/oai1-voice-keyboard-production-readiness-1.4`

## Scope

This step closes the desktop capsule cancel path for Listener BLE recording. Product code already routes `cancel_dictation` through the embedded BLE cancel flag; this change adds executable validation coverage that clicks the actual capsule cancel button and proves the BLE capture returns `Ok` without inserting text or surfacing an internal error.

No BLE bidirectional control protocol was added. Serial is used only to start the firmware recording and to clean up the firmware recording after the desktop-side cancel has already completed.

## Evidence

- PASS: `npm ci`
  - Result: dependencies installed, 0 vulnerabilities.
- PASS: `npm run build`
  - Result: `tsc && vite build` completed; Vite emitted only the existing large chunk warning.
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml`
  - Result: 502 tests passed.
- PASS: `cargo build --manifest-path src-tauri\Cargo.toml`
  - Result: debug `listener-type.exe` built outside the hardware lock.
- PASS: PowerShell parser check for:
  - `tools\embedded_audio_replay\run_ble_stream_smoke.ps1`
  - `scripts\validation\run_ble_recording_cancel_matrix.ps1`
- PASS: `pwsh -NoProfile -File .\scripts\validation\run_ble_recording_cancel_matrix.ps1 -DryRun`
  - Result: wrapper and smoke script parse; command plan records desktop-cancel trigger and COMx runtime resolution.
- PASS: `aiw with-lock -Resource COMx,BLE-current-listener -Run pwsh -NoProfile -File .\scripts\validation\run_ble_recording_cancel_matrix.ps1`
  - Resolved COMx to `COM6`; target BLE address resolved by `ensure_ble_hid_connection` to `A4CB8FF459A6`.
  - Firmware recording start confirmed.
  - Capsule cancel button clicked at `(601, 798)` on `Listener Type Capsule` window rect `568,756,304,84`.
  - CLI completed with `pcm_bytes=110880`, `missing_packets=0`, and no expected stream failure.
  - No transcript, no history session, and no inserted text were produced.
  - Firmware cleanup cancel confirmed after desktop completion.
  - Screenshot evidence was captured before the click. The capsule window-region capture is white because this transparent layered WebView is captured poorly by GDI, so the report also includes a nonblank full-screen context screenshot.

## Artifacts

- Matrix summary: `artifacts\voice-keyboard-production-readiness-1.4-ble-cancel\ble-recording-cancel-matrix.20260607-105037.json`
- BLE smoke report: `artifacts\voice-keyboard-production-readiness-1.4-ble-cancel\ble-stream-smoke.20260607-105054.json`
- Serial log: `artifacts\voice-keyboard-production-readiness-1.4-ble-cancel\ble-stream-smoke-20260607-105038.serial.log`
- Trace log: `artifacts\voice-keyboard-production-readiness-1.4-ble-cancel\ble-stream-smoke-20260607-105038.trace.log`
- Capsule screenshot: `artifacts\voice-keyboard-production-readiness-1.4-ble-cancel\ble-stream-smoke-20260607-105038.desktop-cancel.png`
- Full-screen context screenshot: `artifacts\voice-keyboard-production-readiness-1.4-ble-cancel\ble-stream-smoke-20260607-105038.desktop-cancel.screen.png`

## Notes

- The status validation command `Set-Location ...\Listener-Type; cargo test` is a historical shorthand that is not directly runnable in this repo layout because the Rust crate lives under `src-tauri`. Equivalent full validation was run in the claimed worktree with `cargo test --manifest-path src-tauri\Cargo.toml`.
- The status `with-lock` sample uses repeated `-Resource` and a quoted `-Run` command string; current `with-lock` requires one `-Resource COMx,BLE-current-listener` value and tokenized `-Run pwsh ...` arguments. The corrected equivalent command above was used for the successful hardware run.
