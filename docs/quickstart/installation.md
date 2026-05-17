# Installation

## macOS

1. Download the Listener Type `.dmg` from the Listener Type releases page.
2. Drag `Listener Type.app` into Applications.
3. If macOS reports the app as damaged for an ad-hoc build, run:

```bash
xattr -cr "/Applications/Listener Type.app"
```

4. Launch the app and grant permissions.

## Windows

1. Download the Listener Type setup executable.
2. Run the installer as a normal user unless testing the per-machine IME path, which may require elevation.
3. Launch Listener Type from the Start menu.
4. Open Settings -> Permissions and verify microphone and hotkey status.

The Windows installer bundles `ListenerTypeIme.dll` for x64 and x86 and registers the TSF profile used by the insertion bridge.

## From Source

```bash
npm ci
npm run build
npm run tauri -- info
cargo check --manifest-path src-tauri/Cargo.toml
```

Use `npm run tauri -- dev` for a local app session.
