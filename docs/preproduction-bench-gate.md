# Listener Preproduction Bench Gate

The release gate still requires final physical acceptance, but the target state
is an unattended bench gate where Codex can produce machine-verdict evidence for
the same 16 user scenarios currently covered by
`scripts/windows-listener-preproduction-human-review.ps1`.

## Bench Entrypoint

```powershell
pwsh -NoProfile -File .\scripts\windows-listener-preproduction-bench-collect.ps1 -OutputDir .cache\validation\bench-collect
pwsh -NoProfile -File .\scripts\windows-listener-preproduction-bench-review.ps1 -WriteTemplate -OutputDir .cache\validation\bench-template
pwsh -NoProfile -File .\scripts\windows-listener-preproduction-bench-review.ps1 -CapabilityManifest <bench-capabilities.json>
```

The collector is read-only by default: it records Type runtime state, a desktop
screenshot, Windows BLE PnP/event state, optional live BLE/GATT probes, Type log
tail, and package hashes, then feeds the generated manifest into the bench
review. It does not pair or unpair Windows devices. If the physical bench,
Windows pair/unpair automation, audio fixture, LED optical capture, or second
BLE host are missing, review must remain `BENCH_REVIEW_NO_GO`.

Without a capability manifest, the bench review exits with
`BENCH_REVIEW_NO_GO` and writes the missing capabilities/evidence for every
step. This is intentional: a dry-run must not masquerade as physical acceptance.

## Required Bench Capabilities

- `windows_ble_automation`: enumerate paired/unpaired Listener devices, pair,
  unpair, capture Windows Bluetooth/DeviceSetup events, and verify current GATT
  discoverability. Windows GATT cache is system-wide, so evidence must include
  OS pairing state and uncached service discovery, not just Type runtime cache.
- `second_ble_host`: a second host or isolated BLE adapter for no-Type and
  computer-switch flows.
- `usb_power_relay` / `power_relay`: controlled unplug/replug, cold boot, and
  wired flash recovery.
- `physical_input_fixture`: physical KEY/EC11 actuation. Serial-generated
  gestures are useful diagnostics but do not prove switch bounce, timing, or
  tactile double-click behavior.
- `led_optical_capture`: camera or calibrated light sensors for status, EC11,
  key, and edge LEDs. `~LED:STATUS` alone proves logical render state, not the
  final physical light.
- `audio_fixture`: fixed playback/acoustic fixture for microphone capture, first
  response latency, and BLE audio transfer checks.
- `type_runtime`, `ota_package`, `wired_flash_port`, and `release_artifacts`:
  current Type tray runtime, current firmware OTA package, serial flash path, and
  publishable MSI/ZIP artifacts.

## Evidence Rule

Every bench step must provide a concrete file for each required evidence key.
Examples include Windows BLE state JSON, Type log delta, serial transcript,
optical LED capture summary, audio WAV/probe result, OTA log, flash log, and
root package hash listing. The bench script reports `BENCH_REVIEW_PASS` only
when all required capabilities and evidence files are present.

## External API Basis

Windows BLE automation should use WinRT enumeration/pairing APIs where possible:
`BluetoothLEDevice.GetDeviceSelectorFromPairingState`, connection-status and
device-name selectors, and uncached GATT calls. OS-level GATT cache behavior must
be treated as host-wide state; unpairing or Service Changed indications are the
real cache invalidation boundaries.
