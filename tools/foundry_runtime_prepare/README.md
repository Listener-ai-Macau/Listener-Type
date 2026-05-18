# Foundry Runtime Prepare

Small diagnostic tool for preparing Listener Type's Foundry Local native runtime without compiling the full Tauri app.

It reuses `src-tauri/src/asr/local/foundry_native.rs` and writes runtime files under:

```text
%APPDATA%\Listener Type\models\foundry-local\runtime
```

Run:

```powershell
cargo run --manifest-path tools\foundry_runtime_prepare\Cargo.toml -- prepare --runtime-source auto
```

Expected machine-readable line:

```text
foundry_runtime_result_json={...}
```

Use this before compiling `tools/foundry_asr_probe` when `foundry-local-sdk` would otherwise download native NuGet packages silently from its build script.
