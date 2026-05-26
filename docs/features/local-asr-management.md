# Local ASR Management

## Status

- status: `implemented`
- scope: local ASR model/runtime management for Qwen3-ASR and Windows Foundry Local Whisper

## Source Map

- UI: `src/pages/LocalAsr.tsx`, `src/pages/settings/AdvancedSection.tsx`
- IPC wrappers and types: `src/lib/localAsr.ts`, `src/lib/ipc.ts`, `src/lib/types.ts`
- Command layer: `src-tauri/src/commands.rs`
- Runtime/model code: `src-tauri/src/asr/local/`
- macOS Qwen3-ASR vendor bridge: `src-tauri/vendor/qwen-asr/`, `src-tauri/build.rs`
- Windows Foundry native/runtime preparation: `src-tauri/src/asr/local/foundry_native.rs`, `src-tauri/src/asr/local/foundry_runtime.rs`
- Probe and prepare tools: `tools/foundry_asr_probe/`, `tools/foundry_runtime_prepare/`

## Behavior

- Users manage local ASR models from Settings -> Advanced.
- Switching to a local ASR provider releases inactive engines and avoids external credential validation for local providers.
- Foundry Local runtime preparation is explicit and cancellable.
- Foundry Local Whisper is the Windows local-ASR path and includes runtime component preparation, model catalog/status, model prepare, release, and probe tooling.
- Qwen3-ASR is the macOS local-ASR path and uses the vendored native bridge plus local model registry/status commands.
- Qwen3-ASR model download and local model status are tracked separately from cloud ASR credentials.

## Verification

```powershell
cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run
npm run build
```

## Known Limits

- Foundry Local is Windows-only.
- Runtime downloads and model availability depend on upstream package/model sources.
- Direct Rust test execution may be blocked by local Windows runtime/DLL state; `--no-run` is the current low-risk compile gate.
