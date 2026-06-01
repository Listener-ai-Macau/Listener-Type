# Voice Keyboard Custom Keys 1.2 Validation

Date: 2026-06-01
Agent: codex
Repo: Listener-Type
Branch: ai/codex-voice-keyboard-custom-keys-1.2

## Scope

Implemented desktop-side KEY1-KEY4 custom action mapping for the Listener keyboard firmware F13-F16 fallback entries. EC11 recording/pairing gestures remain firmware-owned; real-device end-to-end validation is deferred to plan step 1.3.

## Automated Checks

- PASS: `npm run test`
- PASS: `npm run build`
- PASS: `npm run verify`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib` (428 tests)
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check`

## Post-Submit Wording Sync

- PASS: aligned active docs with the latest key scheme: EC11 owns recording/recovery gestures, KEY1-KEY4 are custom action keys, and F13-F16 remain safe fallback entries.
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check`

## UI Evidence

- PASS: Headless Edge opened the Vite app, navigated to Settings -> Shortcuts, dismissed the BLE prompt, scrolled to Device custom keys, and captured `docs/validation/voice-keyboard-custom-keys-settings.png`.
- PASS: The UI text contained KEY1, KEY2, KEY3, KEY4, F13, F14, F15, and F16.
- PASS: The checked settings scroll containers had no horizontal overflow.

## Self-Review Notes

- User preferences now default missing `deviceCustomKeys` to four disabled mappings, keeping old installs conservative.
- Settings UI exposes one row per KEY1-KEY4 with localized action labels and action-specific controls.
- Backend action validation rejects arbitrary commands, modifier-only action shortcuts, and self-triggering bare F13-F16 shortcut forwarding.
- Runtime dispatch logs each device-key action and uses existing app, style, translation, selection ask, insertion, and shortcut paths.
