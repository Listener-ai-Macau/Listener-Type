# Listener-Type BLE Probe Listener Guard 1.1 Validation

Date: 2026-05-26
Agent: oai

## Issue

Desktop foreground BLE path probe cancelled the active background Listener BLE notify capture before running its own CCCD reset/enable/disable cycle. If the hardware EC11 button started a firmware audio session during that window, the probe's CCCD disable could cause firmware to abort the session.

## Fix

- `probe_embedded_audio_ble_subscription` now treats an armed, non-cancelled background Listener BLE capture as an already-valid notify subscription and returns `Ok(())` without pausing the listener or touching CCCD.
- Foreground one-shot capture, foreground streaming capture, OTA pause, refresh, and shutdown still use the existing explicit background-listener pause/cancel path.
- Added a targeted async regression test proving foreground probe preserves the active background cancel flag instead of cancelling it.

## Validation

- PASS: `npm ci --prefer-offline --no-audit --no-fund`
- PASS: `npm run build`
- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml embedded_ble` (17 passed)
- PASS: `cargo check --manifest-path src-tauri\Cargo.toml`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check`

## Notes

No real BLE hardware capture was run for this step. The regression is covered at coordinator level so it does not require device access: when the background listener cancel flag is armed, foreground probe exits before any WinRT BLE probe path can reset or disable CCCD.
