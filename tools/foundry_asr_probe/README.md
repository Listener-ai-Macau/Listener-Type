# Foundry ASR Probe

Small diagnostic tool for checking Listener Type's Foundry Local Whisper path without compiling the full Tauri app.

Prepare runtime first:

```powershell
cargo run --manifest-path tools\foundry_runtime_prepare\Cargo.toml -- prepare --runtime-source auto
```

Then compile/run this tool with the prepared runtime as the SDK native override:

```powershell
$env:FOUNDRY_NATIVE_OVERRIDE_DIR = Join-Path $env:APPDATA "Listener Type\models\foundry-local\runtime"
cargo run --manifest-path tools\foundry_asr_probe\Cargo.toml -- status --model whisper-small
cargo run --manifest-path tools\foundry_asr_probe\Cargo.toml -- prepare --model whisper-small --runtime-source auto
cargo run --manifest-path tools\foundry_asr_probe\Cargo.toml -- transcribe --model whisper-small --runtime-source auto --language zh --audio path\to\audio.wav
```

Expected machine-readable line:

```text
foundry_probe_result_json={...}
```
