# Listener Type Demo Removal Validation - 2026-05-30

Worktree: `Listener-Type-wt-adhoc-oai2-remove-demo`
Branch: `adhoc/oai2/remove-demo`

## Scope

- Removed the first-run/offline demo mode from the product UI.
- Removed demo event plumbing and the `src/lib/demoMode.ts` helper.
- Removed the demo content feature doc/index entry.
- Removed unused demo translation keys from all app locales.
- Fixed the capsule X/cancel action so it remains clickable during stop-pending, transcribing, and polishing/thinking states while the confirm action stays limited to recording/error retry.

## Validation

- `rg -n "OPEN_DEMO|requestDemoMode|consumePendingDemoMode|DemoModeCard|quickStartDemo|providerPrompt\.openDemo|overview\.demo|demoTitle|demoPlays|embeddedBleOpenDemo|docs/features/demo-content|demo_content_package|Open demo|先看 Demo|離線 Demo|离线 Demo|오프라인 데모|オフラインデモ" src docs/features -S`
  - PASS: no app/demo feature matches.
- `git diff --check`
  - PASS: no whitespace errors.
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
  - PASS: feature script check passed.
- `npm run test`
  - PASS: capsule preview/layout, device health, BLE recovery UI, and firmware OTA JS tests passed.
- `npm run build`
  - PASS: TypeScript and Vite build passed.
  - Note: Vite emitted the existing large chunk warning for `assets/index-*.js`.
- `npm run verify`
  - PASS: `tsc --noEmit`, brand check, dark-mode check, unit tests, and Vite build all passed.
- `npm run test:capsule-actions`
  - PASS: capsule cancel is enabled for recording/transcribing/polishing/error; confirm remains disabled once stop is pending or processing has started.

## Residual Notes

- Remaining `demo` strings are limited to style marketplace mock data in `src/lib/ipc.ts` and vendored Qwen ASR upstream documentation. They are not the TypeApp first-run/offline demo path.
- The capsule cancellation fix is frontend-only. The existing Rust coordinator already accepts `cancel_dictation` during `Processing`; this change makes the visible X button actually reachable while the capsule shows `thinking`.
