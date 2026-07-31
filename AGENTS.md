# Listener Type - Agent Operating Guide

Listener Type is a local-first desktop voice input app. It records speech, transcribes it, optionally rewrites it with a user-selected style pack, and inserts the result at the current cursor. Keep that core path intact.

## Stack

| Layer | Technology |
| --- | --- |
| Desktop | Tauri v2, Rust |
| Frontend | React 18, TypeScript, Vite |
| Native input | macOS accessibility insertion, Windows hooks and TSF IME |
| ASR | Volcengine, OpenAI-compatible batch ASR, Apple Speech, local Qwen ASR, Windows Foundry Local |
| Polish | Ark, DeepSeek/OpenAI-compatible, Anthropic-compatible, custom OpenAI-compatible endpoints |

## Project Map

- `src/` - React app, pages, shared IPC wrappers, styles and UI state.
- `src-tauri/src/` - Rust backend: recording, ASR, polish, preferences, insertion, updater, IME bridge.
- `windows-ime/` - Native Windows TSF text service used for reliable insertion.
- `scripts/` - Build, packaging, smoke-test and audit scripts.
- `docs/` - User-facing and operational documentation.
- `specs/` - Architecture, design and code traceability.

## Required Reading

- Frontend or visual work: read `specs/DESIGN.md`.
- Backend/session/persistence/provider work: read `specs/ARCHITECTURE.md`.
- Windows packaging/IME work: read `docs/platform/windows-ime.md` and `specs/tech_docs/windows-platform.md`.
- Release/updater work: read `docs/release/updater.md`.
- Any broad change: update `specs/traceability/files.md` or run `npm run check:traceability -- --write`.

## Invariants

- Local dictation, style packs, history, settings and insertion must work without any Listener Type backend.
- Remote marketplace and GitHub OAuth are disabled unless explicitly configured for Listener Type.
- Updater metadata points only at `Listener-ai-Macau/Listener-Type`.
- Do not reintroduce upstream product names, bundle ids, service domains, OAuth client ids or TSF GUIDs.
- Preserve user data boundaries: app data is under `Listener Type`; credential service is `com.listener.type`.
- Keep the legacy `--ol-*` CSS token namespace unless a deliberate full UI refactor updates every consumer and traceability doc.

## Verification

Before claiming completion, run the narrowest relevant checks plus:

```bash
npm run build
npm run check:brand
npm run check:cloud
npm run check:traceability
cargo check --manifest-path src-tauri/Cargo.toml
```

For Windows IME edits, also run the static scripts under `scripts/windows-*.test.mjs` and document any Windows runtime gap if you are not on Windows.

## Shared agent rules (Grok / Claude / Codex)

Also follow `C:\Users\Billy\Desktop\Denzic\ai-collaboration-workflow\docs\shared_product_engineering_rules.md`
(or `$env:AI_WORKFLOW_REPO\docs\shared_product_engineering_rules.md`): fix-before-acceptance,
fresh test binary, anti-regression contracts, **§1.4 always-latest Program Files Type + matching firmware**
(install MSI / flash or OTA before owner opens the app for acceptance).

### Voice wake / voiceprint (anti-regression)

- Deleting the voiceprint must **not** disable automatic wake. No-template path open-gates
  on phrase hit (`speaker_verification::verify` returns match when unenrolled).
- `buffered_speaker_candidate_kind(VoiceActivation, _, enrolled=false)` must still be
  `Verification`, never `Rejected`.
- Primary automatic wake uses `StreamingDetector::new` (sensitive). Reserve `new_strict`
  for in-session diagnostic reactivation only.
- Without enrollment, do not stall on the 1.1s owner-speech window.
- Firmware `voiceAutoStartEnabled` must be on for device VAD auto-start; settings UI:
  「检测到人声后自动开始」.
