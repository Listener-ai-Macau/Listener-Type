# listener-type-dictation-session-fsm-refactor/1.3 validation

Agent: oai2
Date: 2026-06-04
Repo: Listener-Type
Branch: ai/oai2-listener-type-dictation-session-fsm-refactor-1.3

## Scope

- Added a bounded Listener BLE session actor command log with monotonic sequence numbers for BLE packets, ASR partial/final callbacks, stop, cancel, timeout, notify ready/cleanup, and actor restart markers.
- Kept completed background BLE sessions listening through ASR/polish failure handling instead of cancelling or reopening the notify subscription.
- Reused an already-ready background notify subscription when `start_dictation` is invoked for the Listener BLE source.
- Added replay/unit coverage for rapid repeated short sessions, cancel during tail drain command ordering, notify cleanup delay, ASR empty/final command ordering, actor restart, and ready listener reuse.

## Validation

Prerequisite:

```powershell
npm ci
```

Result: PASS, restored ignored frontend dependencies from `package-lock.json`.

Commands:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --lib dictation
cargo test --manifest-path src-tauri/Cargo.toml --lib embedded_ble
cargo test --manifest-path src-tauri/Cargo.toml --lib embedded_streaming
pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check
git diff --check
```

Results:

- PASS: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- PASS: `npm run build`
- PASS: `cargo test --manifest-path src-tauri/Cargo.toml --lib dictation` (59 passed)
- PASS: `cargo test --manifest-path src-tauri/Cargo.toml --lib embedded_ble` (36 passed)
- PASS: `cargo test --manifest-path src-tauri/Cargo.toml --lib embedded_streaming` (5 passed)
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check` (only Git CRLF conversion warnings)

## Notes

No real-device BLE matrix was run in this step. The plan's real-device rapid recording/cancel matrix remains step 1.4.
