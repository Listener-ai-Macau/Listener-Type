# listener-type-provider-model-management 1.2 validation

Agent: oai2
Date: 2026-06-01
Worktree: `C:\Users\Billy\Desktop\listener\Listener-Type-wt-oai2-listener-type-provider-model-management-1.2`

## Acceptance coverage

- Available local model listing and download selection remains driven by `listLocalAsrModels` and the Foundry catalog in `LocalAsr`.
- Download progress, cancellation, and resume paths remain backed by the existing Tauri commands and progress events.
- Qwen rows now show local disk usage from `downloadedBytes` during partial or completed downloads.
- Cached Foundry selected models now label the cached size as disk usage.
- Deleting a complete or partial Qwen local model now requires confirmation before calling `deleteLocalAsrModel`.
- Engine load/release status continues to use the real Tauri `getLocalAsrEngineStatus`, `preloadLocalAsr`, and `releaseLocalAsrEngine` commands; no mock path was added.

## Validation commands

- `npm ci` - PASS
- `npx tsc --noEmit` - PASS
- `npm run build` - PASS, with the existing Vite chunk-size warning.
- `git diff --check` - PASS
- `npm run verify` - PASS
- `cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run` - PASS on rerun after the first 120s timeout.

## Notes

- No large ASR model download was performed in this validation pass; model lifecycle behavior was verified by code path inspection plus TypeScript, Vite, frontend verify, and Rust compile checks.
