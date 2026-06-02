# ec11-rotary-volume-control 1.2 validation

Date: 2026-06-02
Repo: Listener-Type
Branch: ai/oai1-ec11-rotary-volume-control-1.2

## Commands

- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
  - Result: `PASS: Listener-Type repo feature script is present, concise, and covers desktop core responsibilities.`
- PASS: stale LocalAsr navigation/i18n residual search
  - Command: `rg -n "NAVIGATE_LOCAL_ASR_EVENT|goToLocalAsr|localAsrGoDownload|localAsrManage|nav\.marketplace|nav\.localAsr|shell\.footer\.account|modal\.sections\.account|modal\.account|currentTab.*localAsr|setCurrentTab\('localAsr'|<LocalAsr embedded" src`
  - Result: no matches.
- PASS: `npm run build`
  - Result: TypeScript and Vite build passed. Vite reported the existing >500 kB chunk-size warning.
- PASS: `npm run verify`
  - Result: frontend verification passed: product-copy check, `tsc --noEmit`, brand check, dark-mode check, unit tests, and Vite build.
- PASS: `npm run test`
  - Result: capsule preview/action/layout, device health, BLE recovery UI, and firmware OTA tests passed.
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run`
  - Result: Rust test target compiled successfully.

## Visual Evidence

Local dev URL used for visual validation:

- `http://127.0.0.1:5182/?visual=local-asr&devOs=mac`

Artifacts:

- `docs/validation/ec11-rotary-volume-control-1.2/advanced-local-asr.png`
- `docs/validation/ec11-rotary-volume-control-1.2/providers-local-asr-hint.png`
- `docs/validation/ec11-rotary-volume-control-1.2/providers-local-asr-body.txt`

Manual visual check:

- PASS: Settings > Advanced still renders the embedded LocalAsr model-management UI.
- PASS: Providers renders the local Qwen ASR hint and downloaded model state/list after local Qwen is active.
- PASS: Providers no longer renders the dead LocalAsr navigation buttons (`localAsrManage` / `localAsrGoDownload`).
- PASS: screenshots show no blank page, modal overlay, obvious overlap, or clipped controls in the changed states.
