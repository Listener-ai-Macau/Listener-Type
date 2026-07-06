# Windows Build

Windows builds include the Tauri app. Default 3.5/OOBE installers do not bundle
or register the optional Listener Type TSF IME, so installing the product should
not add a system input method. The installer does include cleanup hooks to
unregister a legacy Listener Type TSF IME left by an older package.

## Important Files

- `windows-ime/ListenerTypeIme.sln`
- `windows-ime/ListenerTypeIme.vcxproj`
- `windows-ime/src/`
- `src-tauri/nsis/listener-type-ime-cleanup-hooks.nsh`
- `src-tauri/wix/listener-type-ime-cleanup.wxs`
- `src-tauri/nsis/listener-type-ime-hooks.nsh` (optional TSF packaging only)
- `src-tauri/wix/listener-type-ime.wxs` (optional TSF packaging only)
- `scripts/windows-package-msvc.ps1`
- `scripts/windows-package-msvc.test.mjs`

## Build

Use a Windows runner with Visual Studio Build Tools and Rust installed.

```powershell
npm ci
npm run build
node scripts/windows-package-msvc.test.mjs
pwsh -NoProfile -File scripts/windows-package-msvc.ps1 -SkipRustInstall -SkipNpmCi -IncrementalReleaseBuild -CleanArtifacts
```

The generated installer is the user-facing artifact. A clean target machine should only need Windows, the installer, and network access for WebView2 Evergreen Runtime bootstrap if the runtime is not already installed. It must not require Node.js, Rust, ESP-IDF, this repository, or firmware flashing tools.

For fast package refreshes after docs/scripts/workflow-only commits, reuse the
already-built release executable and only relink the MSI:

```powershell
pwsh -NoProfile -File scripts/windows-package-msvc.ps1 -SkipRustInstall -SkipNpmCi -CleanArtifacts -ReuseExistingExe
```

Do not use `-ReuseExistingExe` after changes under `src`, `src-tauri`, package
manifests, icons, WiX inputs, or other product inputs. The script refuses dirty
or stale product inputs, but final publish builds should still use the full
command above.

The packaging script writes command output to `.artifacts/windows-msvc/*.log`
and prints heartbeats while Rust is quiet, so a long `Compiling ...` line is not
treated as a frozen terminal. Local package refreshes should keep
`-IncrementalReleaseBuild`; if the machine is memory constrained or a toolchain
bug appears, pass `-CargoBuildJobs 1` to reproduce the old serial build
behavior. If `sccache` is installed, pass `-UseSccache` to set `RUSTC_WRAPPER`
for that run.

The default package path intentionally skips `ListenerTypeIme.dll` and the TSF
registration hooks. It may unregister and remove stale IME files from a previous
package. Use `scripts/windows-ime-register.ps1` only for manual developer
validation of the optional TSF insertion bridge.

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

Optional IME registration and insertion smoke require Windows and explicit
manual registration; they are not part of the default installer smoke.
