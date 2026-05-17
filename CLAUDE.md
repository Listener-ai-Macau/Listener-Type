# Listener Type - Claude Entry

This file mirrors `AGENTS.md` for Claude Code. Use the same project facts and verification rules.

Listener Type is a Tauri desktop voice input app. The key user flow is:

1. Global hotkey starts recording.
2. ASR transcribes the recording.
3. Optional polish/style-pack processing rewrites the transcript.
4. The result is inserted at the current cursor, with clipboard fallback.

Read these documents before changing the corresponding area:

- `specs/ARCHITECTURE.md` for backend, persistence, provider, hotkey, session and insertion work.
- `specs/DESIGN.md` for frontend and UI work.
- `docs/platform/windows-ime.md` for Windows TSF IME and installer work.
- `docs/release/updater.md` for updater and release-channel work.
- `specs/traceability/files.md` to understand file ownership.

Hard constraints:

- Package name: `listener-type`.
- Product name: `Listener Type`.
- Bundle identifier and credential service: `com.listener.type`.
- Remote marketplace and OAuth are opt-in Listener Type configuration, not production defaults.
- Updater endpoints must use `https://github.com/Listener-ai-Macau/Listener-Type`.
- Keep source files traceable with `npm run check:traceability`.
