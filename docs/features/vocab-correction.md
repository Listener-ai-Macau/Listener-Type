# Vocabulary And Correction Rules

## Status

- status: `implemented`
- scope: vocabulary entries, presets, correction rules, and session hit tracking

## Source Map

- UI: `src/pages/Vocab.tsx`
- Presets: `src/lib/vocabPresets.ts`, `src/lib/vocab-presets.json`
- Backend commands: `src-tauri/src/commands.rs`
- Persistence and migrations: `src-tauri/src/persistence.rs`
- Correction engine: `src-tauri/src/correction.rs`
- History display: `src/pages/History.tsx`

## Behavior

- Users can add vocabulary phrases and optional notes.
- Correction rules support literal replacements with syntax validation.
- Presets make repeated vocabulary sets reusable.
- Dictation sessions can record vocabulary hit counts and emit `vocab:updated` for UI refresh.

## Verification

```powershell
cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run
npm run build
```

## Known Limits

- Vocabulary is contextual help, not a guarantee that every ASR provider will recognize or preserve a term.
