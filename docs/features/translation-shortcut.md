# Translation Shortcut

## Status

- status: `implemented`
- scope: translation page and dedicated hotkey binding that is kept separate from dictation and QA

## Source Map

- UI: `src/pages/Translation.tsx`
- Shortcut settings: `src/pages/settings/ShortcutsSection.tsx`, `src/components/ShortcutRecorder.tsx`
- Command and overlap validation: `src-tauri/src/commands.rs`
- Runtime listener lifecycle: `src-tauri/src/coordinator.rs`, `src-tauri/src/lib.rs`

## Behavior

- Translation has a dedicated shortcut binding.
- Settings validation rejects overlap between dictation, QA, translation, switch-style, and open-app shortcuts where applicable.
- The translation shortcut listener is started and stopped with the main coordinator lifecycle.

## Verification

```powershell
node scripts\check-hotkey-recorder.mjs
npm run build
```

## Known Limits

- Global shortcut behavior varies across macOS, Windows, Wayland, and desktop environment policy.
