# voice-keyboard-low-power-wake-policy 1.2 validation

Date: 2026-05-27
Agent: tai
Repo: Listener-Type
Branch: ai/tai-voice-keyboard-low-power-wake-policy-1.2

## Scope

Implemented Listener desktop idle wake and reconnect UX for the accepted firmware 1.1 wake policy.

- `start_dictation` on Embedded BLE now enters bounded wake recovery instead of surfacing raw `transport not ready` style errors.
- Background and foreground BLE paths update a structured wake-recovery snapshot with reconnect attempts, recent disconnect reason, notify subscription state, and last attempt/ready timestamps.
- Recovery failures are mapped to user-actionable guidance: press KEY4/wake key, reconnect, or export diagnostics.
- Diagnostic package schema is now `2` and includes BLE wake-recovery state plus the current V1 firmware wake policy snapshot while preserving privacy exclusions for API keys and raw transcript text.
- Overview device health and BLE panel consume readiness and wake-recovery state, show wake guidance, and expose diagnostic export on BLE error states.
- TypeScript mocks, health tests, probe messages, and localized strings were updated for the new status surface.

## Acceptance Mapping

- Idle, low-power, or host-disconnect start path: covered by coordinator wake-recovery state, bounded wait, capsule guidance, and frontend health messages.
- BLE reconnect path: covered by listener refresh, readiness wait, notify-ready tracking, reconnect attempt accounting, and tests around wake recovery state.
- Offline-state wake handling: represented in the firmware wake policy snapshot as `key4_only`, `KEY4/GPIO21`, `GPIO35`, and `voiceKeyDeepSleepWake=false`; user guidance tells users to press KEY4/wake key.
- Diagnostics: exported BLE diagnostics include recent disconnect reason, reconnect attempts, notify subscription state, background listener readiness, and wake policy snapshot. The diagnostic package test checks schema/privacy fields.

## Validation

Passed:

- `npm ci`
- `npm run build`
- `npm run test`
- `cargo fmt --manifest-path src-tauri\Cargo.toml --check`
- `cargo test --manifest-path src-tauri\Cargo.toml --lib`
- `cargo test --manifest-path src-tauri\Cargo.toml`
- `cargo test --manifest-path tools\embedded_audio_replay\Cargo.toml`
- `cargo test --manifest-path tools\firmware_ota_headless\Cargo.toml`
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- `git diff --check`
- `pwsh -NoProfile -File ..\ai-collaboration-workflow\scripts\aiw.ps1 validate -Plan voice-keyboard-low-power-wake-policy`

## Hardware Boundary

Manual/headless real-device idle-disconnect reconnect smoke was not run in this step because the shared hardware locks were already active:

- `COM5` owner: `codex`
- `BLE-14C19F48FE72` owner: `codex`

The real 15-minute idle, KEY4 wake, BLE reconnect, voice recording start/stop/cancel, and diagnostic export product validation remains owned by plan step 1.3.
