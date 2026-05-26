# Provider Credentials And Diagnostics

## Status

- status: `implemented`
- scope: provider selection, credential vault storage, provider validation, model listing, and privacy-safe diagnostic export

## Source Map

- Provider settings UI: `src/pages/settings/ProvidersSection.tsx`
- Advanced local-provider switch UI: `src/pages/settings/AdvancedSection.tsx`
- Frontend IPC wrappers: `src/lib/ipc.ts`, `src/lib/localAsr.ts`
- Command layer: `src-tauri/src/commands.rs`
- Credential vault and settings persistence: `src-tauri/src/persistence.rs`
- Runtime smoke helpers: `scripts/windows-real-asr-insertion-smoke.ps1`, `scripts/windows-runtime-smoke.ps1`, `tools/volcengine_asr_probe/`

## Behavior

- Active ASR and LLM providers are persisted in the credentials root, separate from regular UI preferences.
- Provider API keys, endpoints, models, proxy settings, and active provider IDs are owned by the OS credential vault when available.
- Vault payloads are chunked because Windows Credential Manager has a small per-entry blob limit.
- Legacy credential files or old keyring accounts are migrated into the current vault format when possible.
- Provider validation and model listing run through the active provider config rather than hardcoded defaults.
- Local ASR providers such as Qwen3-ASR and Foundry Local Whisper are treated as keyless for credential validation.
- Diagnostic export includes app/device/config status, recent errors, and redacted timeline data; it does not export audio, transcripts, inserted text, or credential values.

## Verification

```powershell
cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run
npm run build
```

For real-provider smoke, run the Windows real ASR scripts only on a machine with the intended credentials and target app installed.

## Known Limits

- Real ASR validation depends on provider accounts, network availability, and selected model access.
- OS credential-vault prompts and permissions vary by platform.
- Diagnostic export intentionally reports credential presence and provider IDs, not secret values.
