# Build And Release Guide

## Local Verification

```bash
npm ci
npm run build
npm run check:brand
npm run check:cloud
npm run check:traceability
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

## Release Notes

- Confirm product name, package name and bundle id match `docs/release/branding-and-channels.md`.
- For Windows, confirm `src-tauri/tauri.conf.json` keeps `bundle.windows.webviewInstallMode` at `downloadBootstrapper` with `silent: true` for normal online distribution. Use an explicit offline installer variant only for factory/lab machines without internet access.
- Generate the Windows installer with `cargo tauri build` or `npm run tauri -- build -- --bundles nsis,msi` on a Windows MSVC machine. The resulting installer must start on a clean Windows user account without Node.js, Rust, ESP-IDF, or repo files.
- Run a clean Windows install/OOBE check: install, launch from Start, grant permissions, enter the Listener BLE pairing flow, and export **Settings -> About -> Export diagnostic package**. Inspect the JSON for app version, BLE/config status, recent errors and timeline, and confirm it contains no audio recordings, transcript text, API keys or access tokens.
- Sign public Windows artifacts with Authenticode. Internal unsigned builds must be labeled as internal test builds and accompanied by SHA256 hashes because Windows SmartScreen may show an unknown-publisher warning.
- Generate updater manifests with `scripts/write-updater-manifest.mjs`.
- Upload release artifacts and manifests to `Listener-ai-Macau/Listener-Type`.
- Do not ship remote marketplace or OAuth as production features until Listener Type owns the backend and OAuth app.
