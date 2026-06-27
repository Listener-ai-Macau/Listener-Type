# Listener Type Feature Map

`tools/ai/repo_features.ps1` prints a machine-readable summary of the product
surface. The files in this directory are the human-readable feature map: each
document should state what is implemented, where the code lives, how to validate
it, and which limits remain.

## Index

| Feature | Document | Primary code |
| --- | --- | --- |
| Dictation pipeline | `dictation-pipeline.md` | `src-tauri/src/coordinator/`, `src-tauri/src/asr/`, `src-tauri/src/insertion.rs`, `src/components/Capsule.tsx` |
| Embedded BLE audio | `p13_embedded_audio_software_integration.md`, `embedded-ble-dictation-quality.md` | `src-tauri/src/embedded_audio.rs`, `src-tauri/src/embedded_ble.rs`, `tools/embedded_audio_replay/` |
| Provider credentials and diagnostics | `provider-credentials-diagnostics.md` | `src-tauri/src/persistence.rs`, `src-tauri/src/commands.rs`, `src/pages/settings/ProvidersSection.tsx` |
| Local ASR management | `local-asr-management.md` | `src-tauri/src/asr/local/`, `src/lib/localAsr.ts`, `tools/foundry_*` |
| Windows IME insertion | `windows-ime-insertion.md` | `windows-ime/`, `src-tauri/src/windows_ime_*`, `src-tauri/nsis/listener-type-ime-cleanup-hooks.nsh`, optional `src-tauri/nsis/listener-type-ime-hooks.nsh` |
| Updater and desktop shell | `updater-release.md` | `src/components/AutoUpdate*.tsx`, `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json` |
| QA, translation, vocabulary, and style packs | `qa-selection-ask.md`, `translation-shortcut.md`, `vocab-correction.md`, `style-pack-marketplace.md` | `src/pages/`, `src-tauri/src/coordinator/`, `src-tauri/src/persistence.rs` |

## Maintenance Rules

- Keep this directory aligned with `docs/features/index.json`.
- Do not list planned work as implemented until code and validation evidence exist.
- Use `known_limits` for platform dependencies, credentials, hardware, or packaging requirements that remain true.
- Keep local planning, validation logs, and review state outside these product
  feature docs.
