# Listener Type Provider Dark UI And Sample Removal Validation - 2026-05-31

Branch: `adhoc/tai2/volcengine-dark-remove-demo`

## Changes

- Changed the shared `SelectLite` popover from hardcoded light colors to theme tokens so provider dropdowns, including Volcengine, render correctly in dark mode.
- Removed the browser fallback marketplace sample pack and extra imported style-pack sample from `src/lib/ipc.ts`.
- Updated quickstart, OOBE, and factory QA docs so no user path points to a removed trial mode.
- Added `npm run verify` coverage that fails if product source under `src/` reintroduces `demo` copy.
- Moved the 7-day chart and recent transcript list from Overview into History.
- Adjusted History detail layout so long transcript text wraps inside its cards and narrow widths use responsive columns.

## Validation

- `rg -n "demo|Demo|DEMO" src docs README.md README.zh.md package.json -S`
  - PASS: no product source hits; remaining matches are historical validation notes only.
- `npm run check:dark-mode`
  - PASS: dark mode CSS complete, no hardcoded white backgrounds, IIFE invoked.
- `npm test`
  - PASS: all listed frontend unit checks completed.
- `npx tsc --noEmit`
  - PASS.
- `npm run verify`
  - PASS: no product demo copy, TypeScript, brand, dark-mode, unit tests, and Vite build all green.
- `npm run build`
  - PASS: Vite production frontend built successfully.
- `npx tauri build --debug`
  - PASS: built `src-tauri/target/debug/listener-type.exe` and debug installer bundles; rebuilt after the History layout move.
- `git diff --check`
  - PASS: no whitespace errors.

## Built Artifacts

- `src-tauri/target/debug/listener-type.exe`
- `src-tauri/target/debug/bundle/msi/Listener Type_1.3.3_x64_en-US.msi`
- `src-tauri/target/debug/bundle/nsis/Listener Type_1.3.3_x64-setup.exe`
