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
- Keep dictation decisions and text reconciliation shared across Windows and macOS. Put OS input and accessibility calls behind platform adapters; a Windows-only workaround does not establish product-wide acceptance.

## Verification

### Root-cause repair rule

- For a recording, wake, streaming, speaker, endpoint, preview, or insertion defect, trace one affected session from raw audio/capture timestamps through provider events, local decisions, UI updates, and final insertion. Identify the first incorrect decision and the violated invariant before changing production logic.
- Fix the shared decision or state ownership that caused the defect. Keep distinct states such as confirmed owner, confirmed other, and unobserved/pending evidence distinct; missing or delayed evidence is not proof of another speaker or of silence. Do not add a session-specific text, duration, score, or last-row exception as the sole fix.
- Add a regression that reproduces the observed failure and neighboring cases that must continue to work, especially bystander rejection, normal auto-end, pause continuation, and final text replacement. Run the relevant existing regressions before installing.
- Compare the installed executable hash with the current build and validate on new sessions from that build. A source test, an old-session log, or a narrow symptom fix alone is not grounds to claim the issue is root-fixed or ready to release. Keep the release gate until runtime evidence supports it.

Before claiming completion, run the narrowest relevant checks plus:

```bash
npm run build
npm run check:brand
npm run check:cloud
npm run check:traceability
cargo check --manifest-path src-tauri/Cargo.toml
```

For Windows IME edits, also run the static scripts under `scripts/windows-*.test.mjs` and document any Windows runtime gap if you are not on Windows.

## Product delivery

- Follow the user's current scope and the product behavior in this repository. AIW Goal contracts, archives and blocking acceptance popups are not development prerequisites.
- Preserve existing uncommitted work. Use focused behavioral regressions while iterating; run release checks once a candidate is ready.
- For delivery, build and install the candidate MSI, verify its payload against the installed executable, and identify the matching firmware. Distinguish source tests, installed-app tests and real-device evidence.
- Do not claim hardware or end-to-end acceptance from static checks. Report an unavailable device or external service precisely and continue independent work.
- Recording priorities and accepted behavior: `docs/architecture/repair-order-a-g.md` and `docs/architecture/known-good-2026-09-11.md`.

### Voice wake / voiceprint (anti-regression)

- Persistent voiceprint enrollment uses three guided samples of the current wake phrase
  (about 9 seconds total), Xiaomi-style. Do not add a fourth free-speech prompt. The
  three samples feed both wake and in-session speaker banks; raw enrollment audio is
  discarded after protected templates are saved.
- Deleting the voiceprint must **not** disable automatic wake. No-template path open-gates
  on phrase hit (`speaker_verification::verify` returns match when unenrolled).
- `buffered_speaker_candidate_kind(VoiceActivation, _, enrolled=false)` must still be
  `Verification`, never `Rejected`.
- Primary automatic wake uses `StreamingDetector::new` (sensitive). Reserve `new_strict`
  for in-session diagnostic reactivation only.
- Without enrollment, do not stall on the 1.1s owner-speech window.
- Firmware `voiceAutoStartEnabled` must be on for device VAD auto-start; settings UI:
  「检测到人声后自动开始」.

### Always-latest runtime gate (mandatory)

- 运行、日志采集和人工验收只允许使用当前 active tree 构建后安装到
  `C:\Program Files\Listener Type\listener-type.exe` 的版本；禁止从 sibling snapshot、旧桌面副本或旧快捷方式启动。
- 每次改动后，在启动前核对所有 `listener-type` 进程的绝对路径、ProductVersion/FileVersion 和 SHA-256；允许同一哈希的主进程和 `--local-wake-helper` 子进程。发现旧进程、未知副本或哈希不一致，先停止并重新安装/启动，未通过核对不得称为“已修复”。
- 日志中的 session 只有在上述运行时核对通过后才可作为本次代码验收证据。
- 闸门命令：`pwsh -NoProfile -File .\scripts\verify_latest_runtime.ps1 -RequireRunning`。
