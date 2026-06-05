# voice-keyboard-production-readiness/1.2 validation

Agent: oai3

Date: 2026-06-05

## Scope

This step keeps paired/disconnected BLE notification-wait failures in automatic
reconnect state, keeps those failures on the fast background retry path, and
adds a single-process multi-cycle reconnect validation mode to avoid mixing app
restart cleanup with BLE reconnect acceptance.

No UI surface or firmware code was changed.

## Commands

- `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- PowerShell parser check for `tools\verify_ble_reconnect_recovery.ps1`
- `git diff --check`
- `cargo test --manifest-path src-tauri\Cargo.toml -- --test-threads=1`
- `npm test`
- `npm run build`
- `aiw with-lock -Resource 'COM5,BLE-E99FCE38CC0D' -Run pwsh -NoProfile -Command "... restart_windows_bluetooth.ps1 ...; .\tools\verify_ble_reconnect_recovery.ps1 -Port COM5 -BluetoothAddress E99FCE38CC0D -OutDir 'tests\artifacts\voice-keyboard-production-readiness-1.2-oai3-rapid-cycles' -InitialReadyTimeoutSeconds 60 -ReconnectReadyTimeoutSeconds 5 -CycleCount 5 -SkipStreamSmoke"`
- `aiw with-lock -Resource 'COM5,BLE-E99FCE38CC0D' -Run pwsh -NoProfile -Command "... restart_windows_bluetooth.ps1 ...; .\tools\verify_ble_reconnect_recovery.ps1 -Port COM5 -BluetoothAddress E99FCE38CC0D -OutDir 'tests\artifacts\vkp12-stream' -InitialReadyTimeoutSeconds 60 -ReconnectReadyTimeoutSeconds 5 -PostReconnectStreamSmokeAttempts 3"`

## Results

- Rust format: PASS.
- PowerShell syntax: PASS.
- Diff whitespace: PASS.
- Full Rust tests: PASS, 502 passed, 0 failed.
- Frontend tests: PASS.
- Frontend build: PASS.
- Five rapid single-process BLE reconnect cycles: PASS.
  - Report: `tests\artifacts\voice-keyboard-production-readiness-1.2-oai3-rapid-cycles\20260605-133427\ble-reconnect-recovery.json`
  - Disconnect-to-ready ms by cycle: 2855, 2693, 2691, 2715, 2243.
  - All cycles were under the 5000 ms reconnect limit.
  - OTA identity before and after reconnect cycles: PASS.
  - Recovery capsule evidence: 5 reconnecting lines, 6 reconnected lines.
- Post-reconnect stream smoke: PASS.
  - Report: `tests\artifacts\vkp12-stream\20260605-133749\ble-reconnect-recovery.json`
  - Stream report: `tests\artifacts\vkp12-stream\20260605-133749\stream-smoke\attempt-1\ble-stream-smoke.20260605-133828.json`
  - Disconnect-to-ready: 2507 ms.
  - PCM bytes: 126080.
  - Missing packets: 0.
  - ASR transcript matched the expected Mandarin sentence with CER 0.

## Notes

- The plan validation command references `tools\verify_audio_ble_product_matrix.py --cases A3,A4` from this repo, but that helper is not present here and the firmware repo copy currently advertises A1/A2. I recorded feedback `20260605T050951207Z-fddb82b5417b` and validated the reconnect behavior with the Listener-Type BLE reconnect recovery tooling instead.
- Re-running the original single-cycle reconnect script in a loop force-killed `listener-type` between cycles and produced pre-disconnect WinRT/GATT false failures; feedback `20260605T053352217Z-195913d23a28` records that validation workflow issue.
- A long nested artifact path made Windows PowerShell stream smoke fail while creating `*.start.signal`; feedback `20260605T054009855Z-237f75d36d45` records that path-length validation issue. The short `tests\artifacts\vkp12-stream` rerun passed.
