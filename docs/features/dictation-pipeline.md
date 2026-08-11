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
| Hotkey runtime | `src-tauri/src/global_hotkey_runtime.rs`, `src-tauri/src/hotkey.rs`, `src-tauri/src/combo_hotkey.rs`, `src-tauri/src/qa_hotkey.rs`, `src-tauri/src/shortcut_binding.rs` |
| Recording | `src-tauri/src/recorder.rs`, `src-tauri/src/audio_mute.rs`, `src/components/Capsule.tsx` |
| Embedded BLE audio | `src-tauri/src/embedded_audio.rs`, `src-tauri/src/embedded_ble.rs`, `src-tauri/src/coordinator/dictation.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/cli.rs`, `tools/embedded_audio_replay/**` |
| ASR | `src-tauri/src/asr/mod.rs`, `src-tauri/src/asr/*`, `src-tauri/src/asr/local/**`, `src/pages/LocalAsr.tsx`, `src/lib/localAsr.ts` |
| Polish | `src-tauri/src/polish.rs`, `src-tauri/src/llm_*.rs`, `src/pages/Style.tsx` |
| Insertion | `src-tauri/src/insertion.rs`, `src-tauri/src/unicode_keystroke.rs`, `src-tauri/src/windows_ime_*`, `windows-ime/**` |
| State/history/diagnostics | `src-tauri/src/coordinator.rs`, `src-tauri/src/persistence.rs`, `src-tauri/src/commands.rs`, `src/pages/History.tsx` |

## Behavioral Rules

- `Esc` cancels current recording or processing.
- Each dictation session is independent; Listener Type does not answer the transcript as a chat agent.
- Clipboard fallback must preserve the generated result when direct insertion fails.
- Debug audio recording is opt-in and bounded by retention settings.
- Enrolled owner verification keeps separate OS-protected template banks for the fixed wake phrase and for natural free speech. Wake admission compares only the fixed-phrase bank; in-session owner tracking compares only the free-speech bank. Enrollment audio is discarded after feature extraction.
- When no owner voiceprint is enrolled, wake-derived speaker identity is advisory: uncertain evidence must not erase recognized body text or produce a false no-speech result. Only repeated strong local NonTarget evidence may enforce an owner-isolation veto.
- In-session exclusion measures actual active 100 ms speech frames, not the wall-clock span between the first and last active frame. A pause-spanning fragment with less than 1000 ms active speech cannot become confident NonTarget evidence. Privacy-safe logs include active/speech-span duration, signal RMS, inference latency, score and classification.
- Speaker-model promotion uses `scripts/run-speaker-verification-evaluation.ps1` with the same consented Listener corpus for every candidate. One deployable threshold must pass the overall 20-owner/20-non-owner gates and every short/medium/long active-speech slice plus every clean/noisy/far-field slice. Each slice needs at least five samples per label; reports contain anonymous IDs, signal metrics, scores, model hashes and model-only inference latency, never transcript text or retained enrollment audio.
- Embedded audio keeps the firmware VKA1 packet model intact. Batch debug paths reconstruct a complete PCM session before ASR; streaming paths create the normal ASR consumer on `session_start`, feed each `audio_data` PCM chunk immediately, and finalize through the same `end_session` path on `session_stop`.
- Embedded streaming starts the device AI processing LED when Type accepts the first valid PCM chunk for ASR. The stop boundary only switches the capsule into transcribing feedback; it must not delay the purple AI processing signal until the end.
- Embedded streaming cancel/error/link-loss paths must cancel ASR, restore prepared IME state, return coordinator state to Idle, and show an error capsule.
- Local ASR providers use the same coordinator path as cloud providers after the provider is selected and prepared.
- Windows insertion may use the TSF IME bridge; clipboard/direct insertion fallback remains required so text is not lost.

## Embedded Debug Entrypoints

| Entrypoint | Behavior |
| --- | --- |
| `--submit-embedded-audio <wav-or-pcm>` | Batch file replay: builds VKA1 notifications, reconstructs PCM, then submits to dictation. |
| `--submit-embedded-audio-stream <wav-or-pcm>` | Streaming file replay: builds VKA1 notifications and feeds ASR chunk-by-chunk. |
| `--submit-embedded-audio-ble-once [timeout_ms]` | Batch BLE capture: waits for terminal packet, then submits reconstructed PCM. |
| `--submit-embedded-audio-ble-stream [timeout_ms]` | Streaming BLE capture: forwards notifications as they arrive and finalizes on terminal packet. |

For real hardware smoke, prefer `tools/embedded_audio_replay/run_ble_stream_smoke.ps1`. It starts the CLI with the main window hidden, uses the generated KEY3 path (`~KEY:KEY3:SINGLE`) for product recording timing around the second TTS playback, and saves firmware serial logs. `~VREC:TOGGLE` remains a low-level diagnostic only, and EC11 single-click is a runtime custom-key action, not a recording trigger. EC11 single-click is reserved as `Shift+F13`; KEY1-KEY4 own the bare F13-F24 fallback matrix, and F25+ is outside the supported host hotkey contract. Add `-VerifyHistory` for P13.3 product-chain acceptance so the script checks that the history entry includes embedded BLE audio stats; add `-VerifyInsertion` only when the run should open a temporary Notepad target for automated cursor insertion checking. Manual key presses are only needed when validating the physical recording key itself.

## Verification

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run
cargo test --manifest-path tools/embedded_audio_replay/Cargo.toml
npm run build
npm run check:multi-speaker-timelines
pwsh -NoProfile -File scripts/run-speaker-verification-evaluation.ps1 -Manifest <consented-manifest.json>
pwsh -NoProfile -File tools/embedded_audio_replay/run_ble_stream_smoke.ps1 -Port COM3 -VerifyHistory
```

Platform smoke:

- macOS: microphone permission, Accessibility restart, hotkey start/stop, insertion into Notes or a text editor.
- Windows: microphone privacy, hook status, TSF registration, Notepad insertion.
