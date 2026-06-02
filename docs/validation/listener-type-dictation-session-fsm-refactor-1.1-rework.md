# listener-type-dictation-session-fsm-refactor 1.1 rework validation

Date: 2026-06-02
Agent: oai2

## Rework scope

- Backend capsule emission now uses explicit event session IDs for dictation snapshots instead of reading the current global dictation state inside `emit_capsule`.
- Non-session BLE/device/QA guidance keeps `sessionId = null`, and delayed idle emissions capture their originating session marker.
- Recorder, embedded BLE PCM, embedded BLE partial preview, cancel, ASR, polish, and done capsule emissions pass their captured dictation `SessionId`.
- Frontend ordering rejects non-session active/terminal snapshots while a real session is active, drops late active states for closed sessions, and keeps non-session guidance valid when no real session is active.

## Validation

- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib capsule` (5 passed)
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib embedded_streaming` (5 passed)
- PASS: `npm run test:capsule-ordering`
- PASS: `npm run test`
- PASS: `npm run build` (Vite chunk size warning only)
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check`
