# listener-type-dictation-session-fsm-refactor/1.2 rework validation

Date: 2026-06-04
Agent: tai1
Worktree: `Listener-Type-wt-tai1-listener-type-dictation-session-fsm-refactor-1.2`

## Rework scope

- Added coordinator-level cleanup for cancelled `Processing` sessions when late ASR pipeline errors or timeouts are ignored by the FSM because `cancelled=true`.
- The cleanup restores the prepared IME session, clears embedded audio stats, clears `focus_target`, and returns the session to `Idle` without scheduling the Error capsule finish.
- Kept non-cancelled pipeline error and timeout behavior unchanged.
- Added coordinator-level regressions for cancelled ASR pipeline error and timeout finish paths.
- Rebuilt `ai/tai1-listener-type-dictation-session-fsm-refactor-1.2` directly on `origin/main` (`ffe4e5a`) so review scope excludes unrelated hotkey and `embedded_ble.rs` changes.
- Final `origin/main...HEAD` scope is limited to `docs/validation/listener-type-dictation-session-fsm-refactor-1.2-rework-tai1.md`, `src-tauri/src/coordinator.rs`, `src-tauri/src/coordinator/dictation.rs`, `src-tauri/src/coordinator/resources.rs`, and `src-tauri/src/coordinator_state.rs`.

## Validation

- PASS: `npm run build`
  - Vite build completed; existing chunk-size warning only.
- PASS: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- PASS: `cargo test --manifest-path src-tauri/Cargo.toml --lib dictation`
  - 53 passed; includes `finish_pipeline_error_after_processing_cancel_cleans_without_error_finish` and `finish_timeout_after_processing_cancel_cleans_without_error_finish`
- PASS: `cargo test --manifest-path src-tauri/Cargo.toml --lib embedded_streaming`
  - 5 passed

## Final checks

- PASS: `git diff --check`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git merge-tree --write-tree origin/main HEAD`
  - Result tree: `53cd369e2d54afc8d7eda89266a1e78131c0de2d`
