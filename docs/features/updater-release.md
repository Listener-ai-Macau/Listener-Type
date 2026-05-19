# Updater And Release

## Status

- status: `implemented`
- scope: Tauri updater integration, update UI, and release manifest tooling

## Source Map

- Update UI: `src/components/AutoUpdate.tsx`, `src/components/AutoUpdateGate.tsx`
- Tauri updater registration and restart helper: `src-tauri/src/lib.rs`
- Release docs: `docs/release/`
- Manifest tooling: `scripts/write-updater-manifest.mjs`, `scripts/write-updater-manifest.test.mjs`
- Platform build docs/scripts: `docs/platform/`, `scripts/windows-package-msvc.ps1`

## Behavior

- Settings/About and the background gate share the same update UI code.
- Background update checks are gated by user preferences.
- Release manifest generation is scripted and test-covered.

## Verification

```powershell
node scripts\write-updater-manifest.test.mjs
npm run build
```

## Known Limits

- Actual delivery requires correctly signed release artifacts, valid manifest hosting, and configured updater endpoints.
