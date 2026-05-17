# Listener Type Usage

## First Launch

1. Start Listener Type.
2. Grant Microphone permission.
3. Grant Accessibility permission on macOS, then quit and reopen the app.
4. Open Settings and configure at least one ASR provider and one polish provider, or choose a local ASR runtime when available.

## Dictation

1. Place the cursor in any text field.
2. Press the global hotkey. Defaults are right Option on macOS and right Control on Windows.
3. Speak naturally.
4. Press or release the hotkey based on the configured trigger mode.
5. Listener Type transcribes, applies the selected output mode, and inserts the result. If direct insertion is blocked, the result is copied to the clipboard.

Press `Esc` while recording or processing to cancel the current session.

## Output Modes

| Mode | Behavior |
| --- | --- |
| Raw | Inserts the transcript with minimal post-processing. |
| Light | Fixes filler words, punctuation and obvious recognition mistakes while preserving wording. |
| Structured | Turns loose speech into a structured prompt with context, constraints and requested output. |
| Formal | Converts spoken phrasing into a more formal written style. |
| Translation | Uses the translation hotkey to speak directly into the configured target language. |

## Vocabulary

Use Vocabulary for names, product terms, acronyms and domain words. Enabled entries are injected into supported ASR providers as hotwords and are also sent as semantic hints to the polish step.

## Style Packs

Style packs define reusable writing behavior. Built-in packs work offline. User packs can be created, edited, exported and imported locally. Remote marketplace calls are disabled until a Listener Type backend is configured.

## Provider Network

Each cloud provider has its own Network setting in Settings. The default is provider-aware: domestic providers such as DeepSeek, Ark, SiliconFlow, Bailian and Volcengine use direct connections by default, while overseas, OAuth, aggregation and custom providers follow the system proxy by default.

Use Direct when a stale local system proxy points at a closed port such as `127.0.0.1:1087`. Use System proxy for providers that require a proxy on your network. Custom HTTP proxy accepts URLs such as `http://127.0.0.1:7890`.

## History

History stores recent sessions, including raw transcript, polished output, mode, provider metadata and optional debug recordings. Retention is controlled in Settings.

## Selection QA

Select text in another app and use the QA hotkey to open the floating QA panel. The panel answers against the selected text. Dictation and QA have separate hotkeys and state.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Hotkey does not start recording | Check permissions, current hotkey mode, and whether another app captured the same key. |
| Empty transcript | Check ASR credentials, microphone permission, selected input device and local model availability. |
| DeepSeek or another domestic provider fails only when the proxy app is closed | In Settings, set that provider's Network mode to Direct, or disable the stale system proxy. |
| Text copied instead of inserted | The target app blocked direct insertion; paste from clipboard manually or use Windows IME insertion where available. |
| Remote marketplace is empty | Expected by default. Configure `LISTENER_TYPE_MARKETPLACE_BASE_URL` only when a Listener Type backend exists. |
| No automatic update check on launch | Expected by default for local-first builds. Use the manual About-panel check or enable the setting after Listener Type releases are published. |
