# Embedded BLE Audio Integration

Listener Type can receive voice-keyboard audio over BLE and route it through the
normal dictation pipeline.

## Architecture

```text
Voice keyboard firmware
  -> BLE GATT notifications: session_start / audio_data / session_stop
  -> Listener Type embedded audio source
  -> configured ASR provider
  -> optional polish / style processing
  -> history and text insertion
```

Two host paths are supported:

- Batch capture reconstructs a whole PCM session before submitting it to
  dictation.
- Streaming capture creates a dictation session at `session_start`, forwards
  each `audio_data` PCM chunk as it arrives, and finalizes on `session_stop`.

## Compatibility Rules

- Do not reinterpret `audio_data` as the older `chunk + fragment` model.
- Keep the host subscription order as `CCCD notify -> ValueChanged`.
- Treat `session_cancel`, protocol errors, and link loss as terminal events that
  return the coordinator to idle.
- Current product documentation uses KEY3 / the configurable recording key for
  recording. Older validation notes may mention earlier key mappings.

## Primary Paths

- `src-tauri/src/embedded_audio.rs`
- `src-tauri/src/embedded_ble.rs`
- `src-tauri/src/coordinator/dictation.rs`
- `src-tauri/src/cli.rs`
- `src-tauri/src/commands.rs`
- `tools/embedded_audio_replay/`

## Validation

Use software replay for non-hardware validation:

```powershell
cargo test --manifest-path tools/embedded_audio_replay/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml embedded_audio --lib
cargo test --manifest-path src-tauri/Cargo.toml embedded_ble --lib --no-run
```

Real-device validation requires a voice keyboard, current firmware, and the
configured ASR provider credentials. Use the replay and preflight checks before
running hardware BLE transfer tests.
