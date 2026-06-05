# voice-keyboard-extreme-stability-hardening/1.4 validation

Agent: oai2
Date: 2026-06-05
Repo: Listener-Type
Branch: ai/oai2-voice-keyboard-extreme-stability-hardening-1.4

## Scope

Listener-Type BLE/capsule recovery hardening for the desktop side of the voice-keyboard extreme stability plan.

Implemented:

- Capsule event ordering now tracks the terminal state per `sessionId`; late Recording/Transcribing/Polishing events remain blocked, late Error snapshots cannot overwrite Done, Cancelled, or Idle, and late Done/Cancelled terminal snapshots cannot revive an already closed session.
- Actionable dictation Error capsule paths use a separate 6s hide delay while Done/Cancelled keep the existing fast auto-hide behavior for hardware BLE daily use.
- Diagnostic export now includes `ble.sessionActorHistory` and mirrors it in `desktop/ble_connection_history.json`, exposing ordered BLE actor events for notify readiness, BLE packets, ASR partial/final, stop, cancel, timeout, and actor restart without transcript text.
- `docs/features/ble-customer-recovery-diagnostics.md` documents command-source convergence for device key, app/tray/IPC, CLI toggle, and BLE packet/control paths.

## Targeted Checks Before Full Validation

- PASS: `npm run test:capsule-ordering`
- PASS: `npm run build` (generated `dist/`; existing Vite chunk-size warning only)
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib capsule`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib diagnostic_package_includes_ble_wake_recovery_without_sensitive_text`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib session_actor_diagnostics_expose_ordered_safe_event_context`

Note: initial Rust test attempts failed before compiling product code because Tauri `frontendDist` required `dist/`. Running `npm run build` generated the required frontend bundle; subsequent Rust tests passed.

## Full Validation Commands

- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib embedded_ble` (41 passed)
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib embedded_streaming` (5 passed)
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib dictation` (62 passed)
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib capsule` (7 passed)
- PASS: `npm run build` (existing Vite chunk-size warning only)
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check` (Git printed CRLF normalization warnings only)
