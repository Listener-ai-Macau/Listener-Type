# voice-keyboard-production-readiness/2.1 validation

## Scope

- Added a Windows first-run Listener BLE pairing wizard in the existing floating shell prompt.
- Added a scriptable first-run pairing state model covering discovery, Windows pairing, connection, service discovery, and audio subscription.
- Kept clean Windows install and physical pairing acceptance as later human evidence for plan steps 3.5/5.1.

## Evidence

- `npm test`
- `npm run build`
- `cargo test --manifest-path src-tauri\Cargo.toml`
- `git diff --check`
- Screenshot: `tests/artifacts/voice-keyboard-production-readiness-2.1/first-run-pairing-wizard-initial.png`

## Scripted coverage

- Initial wizard state exposes target discovery progress and Windows Bluetooth Add Device entry.
- Checking state advances through service discovery and audio subscription progress.
- Missing pairing states show Windows Bluetooth pairing recovery.
- Paired-but-unavailable states expose retry plus recovery actions.
- Raw BLE/GATT/CCCD/COM/HRESULT diagnostic strings are replaced by product-facing guidance.
