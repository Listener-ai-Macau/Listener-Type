# Updater And Desktop Shell

## Status

- status: `implemented`
- scope: Tauri updater integration, update UI, desktop shell plugins, tray/autostart behavior, and release manifest tooling

## Source Map

- Update UI: `src/components/AutoUpdate.tsx`, `src/components/AutoUpdateGate.tsx`
- Settings entry: `src/pages/settings/AboutUpdateControl.tsx`, `src/pages/settings/RecordingSection.tsx`
- Tauri updater, tray, single-instance, autostart, restart helper: `src-tauri/src/lib.rs`
- Tauri config and permissions: `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`
- Release docs: `docs/release/`
- Manifest tooling: `scripts/write-updater-manifest.mjs`, `scripts/write-updater-manifest.test.mjs`
- Platform build docs/scripts: `docs/platform/`, `scripts/windows-package-msvc.ps1`

## Behavior

- Settings/About and the background gate share the same update UI code.
- Background update checks are gated by user preferences.
- Autostart state is owned by the OS plugin, not regular app preferences.
- The tray menu exposes quick app access and product controls such as microphone/input/style entries where supported.
- Single-instance behavior focuses the existing app window instead of starting duplicate desktop shells.
- Release manifest generation is scripted and test-covered.

## Verification

```powershell
node scripts\write-updater-manifest.test.mjs
cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run
npm run build
```

## Known Limits

- Actual delivery requires correctly signed release artifacts, valid manifest hosting, and configured updater endpoints.
- Tray capabilities and autostart implementation details vary by platform.
