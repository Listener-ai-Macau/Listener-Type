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

## Static Checks On Non-Windows Hosts

macOS/Linux can still run the JS packaging assertions:

```bash
node scripts/windows-package-msvc.test.mjs
node scripts/windows-startup-lifecycle-contract.test.mjs
```

Full IME registration and insertion smoke require Windows.
