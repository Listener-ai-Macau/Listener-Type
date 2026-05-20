# Windows Build

Windows builds include the Tauri app and the native Listener Type TSF IME.

## Important Files

- `windows-ime/ListenerTypeIme.sln`
- `windows-ime/ListenerTypeIme.vcxproj`
- `windows-ime/src/`
- `src-tauri/nsis/listener-type-ime-hooks.nsh`
- `src-tauri/wix/listener-type-ime.wxs`
- `scripts/windows-package-msvc.ps1`
- `scripts/windows-package-msvc.test.mjs`

## Build

Use a Windows runner with Visual Studio Build Tools and Rust installed.

```powershell
npm ci
npm run build
node scripts/windows-package-msvc.test.mjs
powershell -ExecutionPolicy Bypass -File scripts/windows-package-msvc.ps1
```

The generated installer is the user-facing artifact. A clean target machine should only need Windows, the installer, and network access for WebView2 Evergreen Runtime bootstrap if the runtime is not already installed. It must not require Node.js, Rust, ESP-IDF, this repository, or firmware flashing tools.

## WebView2 Policy

`src-tauri/tauri.conf.json` sets `bundle.windows.webviewInstallMode` to `downloadBootstrapper` with `silent: true`. This keeps the installer small and lets the installer silently install/update the Microsoft Edge WebView2 Evergreen Runtime on online Windows machines. For an offline factory image, preinstall WebView2 Evergreen Runtime or produce a separately named `offlineInstaller` variant.

## Internal Test Signing

Release Windows installers must be Authenticode-signed. Unsigned packages are allowed only for internal clean-machine/OOBE testing; publish the SHA256 hash next to the artifact and tell testers to expect the Windows SmartScreen unknown-publisher prompt.

## Clean Install/OOBE Smoke

On a clean Windows user account:

1. Install the NSIS or MSI artifact.
2. Launch Listener Type from Start.
3. Confirm the app enters permissions onboarding and then the Listener BLE pairing prompt/OOBE path.
4. Open Settings -> About -> Export diagnostic package and save the JSON.
5. Inspect the diagnostic JSON for app version, BLE status, config status, recent errors and timeline, and verify it does not include transcript text, audio files, API keys or access tokens.

## Static Checks On Non-Windows Hosts

macOS/Linux can still run the JS packaging assertions:

```bash
node scripts/windows-package-msvc.test.mjs
node scripts/windows-startup-lifecycle-contract.test.mjs
```

Full IME registration and insertion smoke require Windows.
