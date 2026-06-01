# listener-type-marketplace-backend 1.2 validation

## Summary

- Added a Rust GitHub OAuth device-flow client for `/login/device/code`, `/login/oauth/access_token`, token refresh, and `GET /user`.
- Stored marketplace GitHub access/refresh token metadata in the existing OS credential vault payload.
- Changed mutating marketplace commands to verify GitHub identity via token before forwarding the login to the v1 backend compatibility `X-Dev-User` header.
- Removed browser-dev GitHub OAuth mocks from `src/lib/ipc.ts`.
- Updated marketplace docs and visible settings copy to state that OAuth tokens stay in the OS credential vault.

## Acceptance

- PASS: GitHub device-flow API integration is covered by `github_oauth` Rust tests for start, poll, pending, slow_down, and refresh grant paths.
- PASS: Token, optional refresh token, expiry metadata, scope, and login are stored under `CredentialsVault` in the system keychain-backed credential payload.
- PASS: Upload/like/delete/my marketplace commands call GitHub `/user` through the stored token before backend identity is used.
- PASS: Expiring tokens are refreshed when GitHub provides refresh metadata; non-expiring tokens are validated through `/user` until GitHub rejects them.

## Commands

- PASS: `npx tsc --noEmit`
- PASS: `npm run test`
- PASS: `npm run build`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml github_oauth --lib`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml marketplace_backend --lib`
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib`
- PASS: `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
- PASS: `git diff --check`

## Visual Artifact

- `docs/validation/listener-type-marketplace-backend-1.2-marketplace.html`

## Notes

- No production OAuth client id was added. `GITHUB_OAUTH_CLIENT_ID` remains required to start OAuth login.
- Marketplace remote reads remain disabled by default when no backend URL is configured.
