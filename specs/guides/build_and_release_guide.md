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
- Generate updater manifests with `scripts/write-updater-manifest.mjs`.
- Upload release artifacts and manifests to `Listener-ai-Macau/Listener-Type`.
- Do not ship remote marketplace or OAuth as production features until Listener Type owns the backend and OAuth app.
