# Dictation Pipeline

Listener Type keeps the core voice input path small and explicit:

```mermaid
flowchart LR
  Hotkey["Global hotkey"] --> Recorder["Recorder"]
  Embedded["Embedded BLE VKA1 audio"] --> EmbeddedCollector["Embedded audio collector"]
  EmbeddedCollector --> ASR
  Recorder --> ASR["ASR provider"]
  ASR --> Polish["Polish/style pipeline"]
  Polish --> Insert["Insert at cursor"]
  Insert --> Clipboard["Clipboard fallback"]
  Polish --> History["History"]
```

## Source Map

| Stage | Main files |
| --- | --- |
| Hotkey runtime | `src-tauri/src/global_hotkey_runtime.rs`, `src-tauri/src/platform_hotkey.rs`, `src-tauri/src/combo_hotkey.rs`, `src-tauri/src/qa_hotkey.rs` |
| Recording | `src-tauri/src/recorder.rs`, `src-tauri/src/audio.rs`, `src/components/Capsule.tsx` |
| Embedded BLE audio | `src-tauri/src/embedded_audio.rs`, `src-tauri/src/embedded_ble.rs`, `src-tauri/src/coordinator/dictation.rs`, `tools/embedded_audio_replay/**` |
| ASR | `src-tauri/src/asr.rs`, `src-tauri/src/asr/*`, `src/pages/LocalAsr.tsx` |
| Polish | `src-tauri/src/polish.rs`, `src-tauri/src/llm_*.rs`, `src/pages/Style.tsx` |
| Insertion | `src-tauri/src/inserter.rs`, `src-tauri/src/windows_ime.rs`, `windows-ime/**` |
| State/history | `src-tauri/src/coordinator.rs`, `src-tauri/src/persistence.rs`, `src/pages/History.tsx` |

## Behavioral Rules

- `Esc` cancels current recording or processing.
- Each dictation session is independent; Listener Type does not answer the transcript as a chat agent.
- Clipboard fallback must preserve the generated result when direct insertion fails.
- Debug audio recording is opt-in and bounded by retention settings.
- Embedded audio keeps the firmware VKA1 packet model intact. Batch debug paths reconstruct a complete PCM session before ASR; streaming paths create the normal ASR consumer on `session_start`, feed each `audio_data` PCM chunk immediately, and finalize through the same `end_session` path on `session_stop`.
- Embedded streaming cancel/error/link-loss paths must cancel ASR, restore prepared IME state, return coordinator state to Idle, and show an error capsule.

## Embedded Debug Entrypoints

| Entrypoint | Behavior |
| --- | --- |
| `--submit-embedded-audio <wav-or-pcm>` | Batch file replay: builds VKA1 notifications, reconstructs PCM, then submits to dictation. |
| `--submit-embedded-audio-stream <wav-or-pcm>` | Streaming file replay: builds VKA1 notifications and feeds ASR chunk-by-chunk. |
| `--submit-embedded-audio-ble-once [timeout_ms]` | Batch BLE capture: waits for terminal packet, then submits reconstructed PCM. |
| `--submit-embedded-audio-ble-stream [timeout_ms]` | Streaming BLE capture: forwards notifications as they arrive and finalizes on terminal packet. |

## Verification

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run
cargo test --manifest-path tools/embedded_audio_replay/Cargo.toml
npm run build
```

Platform smoke:

- macOS: microphone permission, Accessibility restart, hotkey start/stop, insertion into Notes or a text editor.
- Windows: microphone privacy, hook status, TSF registration, Notepad insertion.
