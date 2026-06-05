# voice-keyboard-production-readiness/1.5 validation

Agent: tai1
Date: 2026-06-05
Worktree: `Listener-Type-wt-tai1-voice-keyboard-production-readiness-1.5`
Branch: `ai/tai1-voice-keyboard-production-readiness-1.5`

## Acceptance Evidence

- PASS: Valid-key polish path remains wired through `validate_provider_credentials("llm")` and the normal OpenAI-compatible polish path. The existing DeepSeek preset uses `https://api.deepseek.com/v1`, model `deepseek-v4-flash`, provider-default direct proxy, and the focused Rust checks below prove DeepSeek request-body/proxy policy behavior. No live external DeepSeek credential was used in this AI session.
- PASS: Invalid key and provider rejection paths are mapped to action copy instead of raw `401`/`403`: backend returns `providerHttpStatus:<status>` for failed LLM validation, and `src/lib/providerSetup.test.ts` asserts `providerHttpStatus:401 -> apiKeyRejected`.
- PASS: Offline/network failure paths are mapped to short user action copy: backend network and timeout codes classify to `network`/`timeout`, and `ProvidersSection` renders localized guidance instead of internal exceptions.
- PASS: No-device path has an executable recovery route: embedded BLE probe errors classify not-found service/device failures as `noDevice`, and UI guidance routes users to wake/retry/reconnect/diagnostics or microphone recording.
- PASS: User can enter local/test-audio fallback paths before cloud credentials are configured: provider prompt includes `testAudio`, Overview quick start exposes configure API key, local recognition, and microphone recording actions. The provider setup regression test is now part of `npm test` so these recovery classifications stay covered by the standard frontend gate.

## Commands

- PASS: `npm run test:provider-setup`
  - Result: `providerSetup: all assertions passed`
- PASS: `npm test`
  - Result: all frontend unit script checks passed, including `test:provider-setup`.
- PASS: `npm run build`
  - Result: `tsc && vite build` completed; Vite emitted only the existing large chunk warning.
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib provider_default_proxy_policy_splits_domestic_and_overseas_vendors -- --nocapture`
  - Result: 1 passed, 0 failed.
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib openai_chat_body_adds_deepseek_thinking_toggle_by_channel -- --nocapture`
  - Result: 1 passed, 0 failed.
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml`
  - Result: 499 passed, 0 failed; the cache coalescing test initially exposed global test-cache interference under parallel Rust tests, then passed after serializing the affected cache tests.
- PASS: `pwsh -NoProfile -File ..\ai-collaboration-workflow\scripts\aiw.ps1 validate -Plan voice-keyboard-production-readiness`
  - Result: plan validation passed with one unrelated warning: `voice-keyboard-production-readiness/1.4` is blocked.
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
  - Result: Listener-Type repo feature script is present, concise, and covers desktop core responsibilities.
- PASS: `git diff --check`
  - Result: no whitespace errors.
- INFO: `npm run check:brand`, `npm run check:cloud`, and `cargo check --manifest-path src-tauri\Cargo.toml`
  - Result: all passed.
- INFO: `npm run check:traceability`
  - Result: failed on pre-existing broad traceability gaps for files unrelated to this step, including `src-tauri/src/firmware_ota.rs`, `src/lib/bleRecoveryUi.ts`, and `src/lib/marketplaceDiscovery.ts`. `npm run check:traceability -- --write` was inspected and would rewrite a broad manifest set, so that unrelated cleanup was not included in this step.

## Notes

- `npm ci` was required because this dedicated worktree had no `node_modules`; it changed only ignored dependency files.
- A pre-build Rust test attempt failed before `dist/` existed because Tauri `frontendDist` points at `../dist`. After `npm run build`, the focused Rust provider tests passed.
