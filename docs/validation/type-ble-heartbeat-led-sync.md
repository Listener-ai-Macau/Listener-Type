# Type BLE Heartbeat LED Sync

## Scope

Listener-Type now marks the desktop process as alive over the existing BLE audio
control characteristic while the background continuous notify listener is ready.

## Contract

- `TYPE:READY` is written once after the background listener enables notify.
- `TYPE:HB` is written every 4 seconds while that listener stays alive.
- `TYPE:BYE` is best-effort before the listener disables notify.
- Firmware demotes `STATUS_LED_BLE_TYPE_READY` to generic connected when no
  Type heartbeat is received for 12 seconds.
- Type heartbeat writes are intentionally not user activity on firmware, so they
  do not reset low-power idle timers.

## Validation

- `rustfmt --edition 2021 --check src-tauri/src/embedded_ble.rs`
- `cargo check --manifest-path src-tauri/Cargo.toml --lib`
- Firmware static checks cover the reciprocal parser, timeout, and LED demotion
  path.
