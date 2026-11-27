# Installation

## macOS and Linux

RC1 provides a Windows MSI. macOS and Linux currently require source builds; see the [platform scope](../../README.md) before downloading.

## Windows

1. Select the latest numbered Windows x64 prerelease in [Listener Type Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases). Verify the MSI against the SHA-256 in that release description.
2. Run the installer. The production installer is per-machine so Windows may ask for administrator approval.
3. Launch Listener Type from the Start menu.
4. On first launch, allow microphone access. New profiles select the included Companion Paraformer Large model by default. For a computer microphone, follow the [first-dictation guide](../USAGE.en.md#install-and-make-your-first-dictation). If you have a Listener keyboard, choose **Start pairing** and follow the guided Bluetooth setup.
5. If pairing gets stuck, open **Settings -> About -> Device recovery**. Listener Type stops the current recording/BLE session and opens Windows Bluetooth; remove `listener`, then return to Recording -> Listener BLE and pair again.
6. If first launch or pairing still fails, open **Settings -> About -> Export diagnostic package** and save the JSON file. The package includes app version, BLE/config status, recent errors and a timeline; it does not include transcripts or API keys, but saved debug recordings may be attached. Debug recording is off by default; review the package and logs before sharing.

## Status Language

Listener Type uses five user-facing states during the Voice Keyboard OOBE:

| State | Meaning | User action |
|---|---|---|
| Ready | Device and desktop app are ready | Press the EC11 knob or the recording hotkey |
| Recording | The device is listening | Press EC11 or the same hotkey again when done |
| Transferring | Audio is being sent to the desktop | Wait for text |
| Error | Connection, subscription, recording, or transcription failed | Retry or export diagnostics |
| Recovery | Reconnect or re-pair is needed | Remove `listener` in Windows Bluetooth and pair again |

The Windows MSI registers the [TSF insertion component](../platform/windows-ime.md). If the target app cannot accept TSF insertion, follow the paste prompt or recover the final text from History.

### WebView2 Runtime

Listener Type uses Tauri/WebView2 on Windows. The installer is configured with `webviewInstallMode: downloadBootstrapper` and `silent: true`, so it silently installs or updates the Microsoft Edge WebView2 Evergreen Runtime when it is missing and the machine has internet access. For an offline factory or lab image, install the Evergreen Runtime before running Listener Type, or build an offline installer variant by switching the Tauri Windows `webviewInstallMode` to `offlineInstaller`.

### Signing And Internal Tests

Public builds should be Authenticode-signed. Internal unsigned builds are acceptable only for lab/OOBE testing; Windows SmartScreen may show an unknown-publisher warning. Testers should verify the filename, version and SHA256 from the release notes before approving the prompt. Do not distribute unsigned builds to external users.

## From Source

Initialize the pinned `third_party/denzic-platform` submodule before building.

```bash
git submodule update --init --recursive
npm ci
npm run build
npm run tauri -- info
cargo check --manifest-path src-tauri/Cargo.toml
```

Use `npm run tauri -- dev` for a local app session.
