# listener-type-dictation-session-fsm-refactor/1.2 rework validation

Date: 2026-06-03
Agent: tai1
Worktree: `Listener-Type-wt-tai1-listener-type-dictation-session-fsm-refactor-1.2`

## Rework scope

- Reapplied the submitted central dictation FSM implementation onto the current Listener-Type main worktree.
- Fixed cancelled Processing sessions so late `PipelineError` and `Timeout` FSM events are ignored instead of publishing an Error capsule after user cancel.
- Fixed recorder runtime aborts so the abort path publishes an Error capsule through an explicit FSM event even after the abort state has set `cancelled=true`.
- Added pure FSM regression tests for cancelled pipeline/timeout and recorder abort error publication.

## Validation

- PASS: `npm ci`
- PASS: `npm run build`
- PASS: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- PASS: `cargo test --manifest-path src-tauri/Cargo.toml --lib dictation`
  - 52 passed; includes `dictation_fsm_ignores_pipeline_error_and_timeout_after_cancel` and `dictation_fsm_allows_recorder_abort_error_after_cancelled_abort_state`
- PASS: `cargo test --manifest-path src-tauri/Cargo.toml --lib embedded_streaming`
  - 5 passed

## Final checks

- PASS: `git diff --check`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
