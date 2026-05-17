# Tauri CSP

Listener Type uses a restrictive CSP in `src-tauri/tauri.conf.json`.

Current principles:

- Scripts load from `self`.
- Styles load from `self` with inline style support for the existing React inline-style surface.
- Images load from `self`, Tauri asset protocols, blobs and data URLs.
- Connect sources are limited to Tauri IPC and local development endpoints.
- Remote marketplace and OAuth calls originate from Rust command handlers, not direct WebView fetches.

When adding a new network integration, prefer a Rust command with explicit configuration and document the endpoint in `docs/features/` or `docs/release/`.
