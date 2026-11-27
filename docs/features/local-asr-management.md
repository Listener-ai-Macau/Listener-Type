# Local speech recognition

## Windows

Numbered Windows x64 prereleases include Companion Paraformer Large for local Chinese and English dictation. Fresh Windows profiles select it by default. Existing recognizer choices are retained. Use **Settings → Local Recognition** to select the included model after using another provider.

Local recognition processes audio on the PC. Recognition speed and running memory depend on the system. Text processing has separate settings, including optional downloads for local text translation.

The runtime loads the verified bundled model for CPU inference. The Windows packaging script stages the model and its license and notice files. Source builds may need those files before local dictation becomes available.

## macOS

Source builds expose the experimental Qwen3-ASR path. Its model is prepared separately. The vendor bridge is in [src-tauri/vendor/qwen-asr](../../src-tauri/vendor/qwen-asr).

## Implementation

- [Local recognition settings](../../src/pages/LocalAsr.tsx)
- [Frontend IPC](../../src/lib/localAsr.ts)
- [Provider and runtime](../../src-tauri/src/asr/local)
- [Windows packaging](../platform/windows-build.md)
