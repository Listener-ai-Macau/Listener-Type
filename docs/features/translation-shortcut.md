# Translation

## Status

- status: `implemented`
- scope: translation page and dedicated hotkey binding that is kept separate from dictation and QA

## Using translation

- **Text only:** on 64-bit Windows, open Overview → Chinese ↔ English on this PC. Confirm the source and target, explicitly prepare the required components once, then translate. No hardware or cloud key is needed for this local text route. Review/edit the result before saving or copying it.
- Successful text translation folds the selected setup and keeps its language/method summary visible. Reopen it to change choices; an error expands it for recovery. Saving keeps the original and edited result in History → Saved drafts.
- **Voice:** speech recognition is separate. Choose the local or cloud translation engine and languages on the Translation page for explicitly triggered voice translation. Plain dictation retains its original language. Text-trial choices do not change voice settings.
- Local Chinese ↔ English translation has no per-request translation API fee; existing cloud recognition or explicitly selected cloud translation still follows provider charges. Other offline pairs and universal translation quality are not established.

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
