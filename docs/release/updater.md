# Updater

Listener Type uses the Tauri updater plugin. Release metadata is scoped to the Listener Type repository.

## Current Policy

- Automatic background checks are off by default for local-first builds until Listener Type release manifests are published. Users can still run the manual About-panel check, and enabling the setting checks the official Listener Type release channel below.
- Stable updater endpoint: `https://github.com/Listener-ai-Macau/Listener-Type/releases/latest/download/latest-{{target}}-{{arch}}.json`.
- Mirror manifests are not generated unless `LISTENER_TYPE_UPDATE_MIRROR_BASE_URL` is explicitly set.
- Listener Type `1.0.0` is the first managed release line. Do not publish development builds through the updater endpoint.
- Signing key rotation must update `src-tauri/tauri.conf.json` and release automation together.

## Manifest Generation

```bash
LISTENER_TYPE_UPDATE_TARGET=darwin \
LISTENER_TYPE_UPDATE_ARCH=aarch64 \
node scripts/write-updater-manifest.mjs
```

## Verification

```bash
npm run check:cloud
node scripts/write-updater-manifest.test.mjs
```

Do not point updater endpoints at any non-Listener Type production repository.
