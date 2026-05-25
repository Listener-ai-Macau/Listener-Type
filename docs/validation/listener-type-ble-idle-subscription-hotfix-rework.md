# Listener BLE Idle Subscription Hotfix Rework Validation

Date: 2026-05-25
Owner: oai
Branch: ai/oai-listener-type-ble-idle-subscription-hotfix-1.1

## Hardware

- Device: Listener BLE, address `14C19F48FE72`
- Serial: `COM5`
- Firmware live serial evidence: device reported `audio notify subscribed`, MTU `517`, `stream_ready`, and no transport-not-ready error during validated triggers.

## Results

- Foreground BLE stream smoke: PASS
  - Artifact: `artifacts/embedded_stream_smoke/ble-idle-rework-after-gatt-ready-v2/ble-stream-smoke.20260525-195051.json`
  - Result: transcript matched expected text, accuracy `1`, missing packets `0`, history session recorded.
- Background idle route: PASS
  - Artifact dir: `artifacts/embedded_stream_smoke/ble-background-idle-route-step/20260525-200315`
  - App log: `capture #1` kept the BLE notify session for `109260 ms`, then showed Recording capsule and `embedded audio streaming dictation started`.
  - Serial summary: `recording_start_seen=true`, `stream_ready=true`, `notify_enabled=true`, `transport_not_ready=false`.
- Earlier failed probe diagnosis:
  - Desktop log showed Windows GATT/CCCD timing failures before the fix.
  - Foreground smoke initially failed only because the script looked for the old exact log string. The app had already logged `notify CCCD enabled`; the smoke readiness check now waits for that line.

## Automated Checks

- `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check`: PASS
- `git diff --check`: PASS, CRLF warnings only
- `npm run check:dark-mode`: PASS
- `npm test`: PASS
- `npm run verify`: PASS
- `npm run build`: PASS
- `cargo check --manifest-path src-tauri\Cargo.toml`: PASS
- `cargo test --manifest-path src-tauri\Cargo.toml --lib --quiet`: PASS, 388 passed
- `cargo build --manifest-path src-tauri\Cargo.toml`: PASS
