# Style Packs And Marketplace

Style packs are local first. Built-in and user-created packs must work without any network service.

## Local Capabilities

- Create and edit style packs.
- Activate a pack for polish output.
- Export a pack to a zip.
- Import a pack from a zip.
- Track pack origin metadata for future sync without requiring it today.

## Remote Degradation

Listener Type does not ship with a production marketplace backend. The default backend URL is empty and the Rust command layer returns an empty list for passive marketplace reads. Mutating remote actions return a clear configuration error.

To enable a future Listener Type backend:

```bash
LISTENER_TYPE_MARKETPLACE_BASE_URL=https://your-listener-type-backend.example
GITHUB_OAUTH_CLIENT_ID=<listener-type-oauth-client-id>
```

Do not use old service domains or OAuth clients.

## Backend API Contract

Contract version: `listener-type-marketplace-v1`

All remote marketplace traffic goes through Rust IPC commands. The WebView must not call these endpoints directly.

| Operation | Method and path | Notes |
|---|---|---|
| List/search styles | `GET /styles?q=<query>&category=<raw\|light\|structured\|formal>&sort=<new\|popular>&limit=<n>&offset=<n>` | Returns `MarketplaceListPage`; unauthenticated. Older array responses are still accepted by the desktop client. |
| Style detail | `GET /styles/{id}` | Returns full metadata and `prompt`; unauthenticated. |
| Style archive download | `GET /styles/{id}/download` | Returns the style-pack zip installed through local import validation. |
| Upload/update style | `POST /styles/upload` | Multipart field `file` contains the zip; optional `originPackId`; identity comes from GitHub OAuth and is forwarded as the v1 `X-Dev-User` compatibility header. |
| Like style | `POST /styles/{id}/like` | Requires identity; returns like count and whether the user already liked it. |
| Withdraw style | `DELETE /styles/{id}` | Requires identity; backend should soft-delete/withdraw. |
| User likes | `GET /me/likes` | Requires identity; returns remote style ids. |
| User styles | `GET /me/styles` | Requires identity; includes pending/approved/rejected/withdrawn styles. |

Rust classifies backend failures before surfacing them to the UI: network failure, `401 Unauthorized`, `404 Not Found`, other HTTP status, invalid URL, and decode failure.

GitHub OAuth uses device flow through Rust IPC only. The access token, optional refresh token, expiry, scope, and resolved login are stored in the system credential vault with other Listener Type credentials. Mutating marketplace commands refresh an expiring GitHub token when GitHub returns refresh metadata, then call `GET https://api.github.com/user` and use the returned login for the marketplace request. Non-expiring GitHub tokens are treated as valid until GitHub rejects `/user`.

## Source Map

- Frontend marketplace UI: `src/pages/Marketplace.tsx`, `src/components/MarketplaceModal.tsx`, `src/pages/Style.tsx`.
- Marketplace discovery helpers: `src/lib/marketplaceDiscovery.ts`.
- IPC wrappers: `src/lib/ipc.ts`.
- Backend commands: `src-tauri/src/commands.rs`.
- GitHub OAuth client: `src-tauri/src/github_oauth.rs`.
- Backend HTTP client and REST contract: `src-tauri/src/marketplace_backend.rs`.
- Local pack storage and archive import/export: `src-tauri/src/persistence.rs`.

## Verification

```bash
npm run check:cloud
npm run build
```

Manual smoke: open Style, create a pack, export it, import it, and confirm the remote marketplace can remain empty without breaking local packs.
