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
2. Run the installer. The production installer is per-machine so Windows may ask for administrator approval.
3. Launch Listener Type from the Start menu.
4. On first launch, grant microphone/hotkey permissions, then follow the Listener BLE pairing prompt. Use **Start pairing** to switch into the OOBE pairing flow; you should not need Node.js, Rust, ESP-IDF, a serial port, or UUID entry.
5. If first launch or pairing fails, open **Settings -> About -> Export diagnostic package** and save the JSON file. The package includes app version, BLE/config status, recent errors and a timeline; it does not include audio recordings, transcripts, or API keys.

The Windows installer bundles `ListenerTypeIme.dll` for x64 and x86 and registers the TSF profile used by the insertion bridge.

### WebView2 Runtime

Listener Type uses Tauri/WebView2 on Windows. The installer is configured with `webviewInstallMode: downloadBootstrapper` and `silent: true`, so it silently installs or updates the Microsoft Edge WebView2 Evergreen Runtime when it is missing and the machine has internet access. For an offline factory or lab image, install the Evergreen Runtime before running Listener Type, or build an offline installer variant by switching the Tauri Windows `webviewInstallMode` to `offlineInstaller`.

### Signing And Internal Tests

Public builds should be Authenticode-signed. Internal unsigned builds are acceptable only for lab/OOBE testing; Windows SmartScreen may show an unknown-publisher warning. Testers should verify the filename, version and SHA256 from the release notes before approving the prompt. Do not distribute unsigned builds to external users.

## From Source

```bash
npm ci
npm run build
npm run tauri -- info
cargo check --manifest-path src-tauri/Cargo.toml
```

Use `npm run tauri -- dev` for a local app session.
