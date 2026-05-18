# Feature Traceability

## Dictation

| Capability | Source files | Verification |
| --- | --- | --- |
| Hotkey start/stop/cancel | `src-tauri/src/global_hotkey_runtime.rs`, `src-tauri/src/platform_hotkey.rs`, `src-tauri/src/combo_hotkey.rs`, `src/components/Capsule.tsx` | Rust hotkey tests, manual hotkey smoke |
| Audio recording | `src-tauri/src/recorder.rs`, `src-tauri/src/audio.rs` | Rust unit tests, microphone smoke |
| Embedded BLE audio ingest | `src-tauri/src/embedded_audio.rs`, `src-tauri/src/embedded_ble.rs`, `src-tauri/src/coordinator/dictation.rs`, `tools/embedded_audio_replay/**` | VKA1 replay tests, cargo check, hardware BLE smoke |
| ASR routing | `src-tauri/src/asr.rs`, `src-tauri/src/asr/*`, `src/pages/LocalAsr.tsx` | Provider validation, local ASR smoke |
| Polish/output modes | `src-tauri/src/polish.rs`, `src-tauri/src/llm_*.rs`, `src/pages/Style.tsx` | Rust unit tests, style pack smoke |
| Insert and clipboard fallback | `src-tauri/src/inserter.rs`, `src-tauri/src/windows_ime.rs`, `windows-ime/**` | macOS insertion smoke, Windows IME smoke |

## Product Surfaces

| Capability | Source files | Verification |
| --- | --- | --- |
| Main shell and navigation | `src/components/FloatingShell.tsx`, `src/components/WindowChrome.tsx`, `src/pages/*` | `npm run build`, visual smoke |
| Settings and credentials | `src/pages/Settings.tsx`, `src/components/SettingsModal.tsx`, `src-tauri/src/commands.rs`, `src-tauri/src/persistence.rs` | Settings save/load smoke, credential tests |
| History | `src/pages/History.tsx`, `src-tauri/src/history.rs`, `src-tauri/src/persistence.rs` | Rust unit tests, retention smoke |
| Vocabulary | `src/pages/Vocab.tsx`, `src-tauri/src/dictionary.rs` | Hotword injection tests, UI smoke |
| Selection QA | `src/pages/QaPanel.tsx`, `src/pages/SelectionAsk.tsx`, `src-tauri/src/selection.rs`, `src-tauri/src/qa_hotkey.rs` | QA hotkey smoke |

## Local-First Remote Boundaries

| Capability | Source files | Verification |
| --- | --- | --- |
| Marketplace local fallback | `src/pages/Marketplace.tsx`, `src/components/MarketplaceModal.tsx`, `src-tauri/src/commands.rs`, `src-tauri/src/style_packs.rs` | `npm run check:cloud`, local pack import/export smoke |
| Updater | `src-tauri/tauri.conf.json`, `scripts/write-updater-manifest.mjs` | `npm run check:cloud`, updater manifest tests |
| Branding | `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `windows-ime/**` | `npm run check:brand`, Windows static checks |

## Documentation And Audits

| Capability | Source files | Verification |
| --- | --- | --- |
| Documentation inheritance | `docs/**`, `specs/**`, `scripts/check-doc-inheritance.mjs` | `npm run check:docs` |
| Per-file tracking | `specs/traceability/files.md`, `scripts/check-traceability.mjs` | `npm run check:traceability` |
| Cloud-service safety | `scripts/check-cloud-services.mjs` | `npm run check:cloud` |
