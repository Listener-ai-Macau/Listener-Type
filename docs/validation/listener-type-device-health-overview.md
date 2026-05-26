# Listener Type Device Health Overview Validation

Date: 2026-05-25
Branch: ai/oai-listener-type-device-health-overview-1.1

## Scope

- Added a non-blocking Listener BLE health snapshot on Overview.
- Derived health from the selected input source, platform, runtime BLE background-listener state, recent history, and embedded BLE session stats.
- Kept healthy UI terse: a green health pill only.
- Reserved setup/action text for disconnected, unsupported, degraded, unknown, and error states.

## Commands

- `npm run test:device-health` - PASS
- `npm test` - PASS
- `npm run check:dark-mode` - PASS
- `npm run build` - PASS
- `npm run verify` - PASS
- `cargo test --manifest-path src-tauri\Cargo.toml --lib --quiet` - PASS, 388 tests
- `git diff --check` - PASS, Git reported only existing LF-to-CRLF working-copy warnings

## Rework Validation - 2026-05-26

Reviewer rework addressed: setup/listener failures before a successful BLE dictation now flow into Overview through `get_embedded_ble_runtime_status`.

### Commands

- `npm test` - PASS
- `npm run build` - PASS
- `npm run verify` - PASS
- `cargo test --manifest-path src-tauri\Cargo.toml --lib --quiet` - PASS, 391 tests
- `cargo fmt --manifest-path src-tauri\Cargo.toml --check` - PASS
- `pwsh -NoProfile -File ..\ai-collaboration-workflow\scripts\aiw.ps1 validate -Plan listener-type-device-health-overview` - PASS
- `git diff --check` - PASS, Git reported only LF-to-CRLF working-copy warnings

### Rework Notes

- The backend records the last non-idle background Listener BLE listener error and exposes it as `backgroundListenerLastError`.
- Idle background capture timeouts clear the error instead of surfacing as a health failure.
- Overview passes `backgroundListenerLastError` into `summarizeListenerDeviceHealth` without running a foreground BLE probe.
- Device health now classifies pre-session CCCD/notify failures as `bleCccdTimeout` and subscription timeout failures as `bleSubscriptionTimeout`, even when no `DictationSession` exists yet.

## Notes

- Overview rendering does not call `probe_embedded_audio_ble_subscription` or perform a foreground BLE capture.
- The runtime status command reads environment/runtime state only; it does not connect to the device.
- Healthy means recent Listener BLE history contains complete audio with normal transport stats.
- Empty transcript with complete audio is treated as ASR failure, not device failure.
