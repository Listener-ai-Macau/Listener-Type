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
| List/search styles | `GET /styles?q=<query>&sort=<new\|popular>&limit=<n>` | Returns `MarketplaceListItem[]`; unauthenticated. |
| Style detail | `GET /styles/{id}` | Returns full metadata and `prompt`; unauthenticated. |
| Style archive download | `GET /styles/{id}/download` | Returns the style-pack zip installed through local import validation. |
| Upload/update style | `POST /styles/upload` | Multipart field `file` contains the zip; optional `originPackId`; dev-mode identity currently uses `X-Dev-User` until OAuth lands. |
| Like style | `POST /styles/{id}/like` | Requires identity; returns like count and whether the user already liked it. |
| Withdraw style | `DELETE /styles/{id}` | Requires identity; backend should soft-delete/withdraw. |
| User likes | `GET /me/likes` | Requires identity; returns remote style ids. |
| User styles | `GET /me/styles` | Requires identity; includes pending/approved/rejected/withdrawn styles. |

Rust classifies backend failures before surfacing them to the UI: network failure, `401 Unauthorized`, `404 Not Found`, other HTTP status, invalid URL, and decode failure.

## Source Map

- Frontend marketplace UI: `src/pages/Marketplace.tsx`, `src/components/MarketplaceModal.tsx`, `src/pages/Style.tsx`.
- IPC wrappers: `src/lib/ipc.ts`.
- Backend commands: `src-tauri/src/commands.rs`.
- Backend HTTP client and REST contract: `src-tauri/src/marketplace_backend.rs`.
- Local pack storage and archive import/export: `src-tauri/src/persistence.rs`.

## Verification

```bash
npm run check:cloud
npm run build
```

Manual smoke: open Style, create a pack, export it, import it, and confirm the remote marketplace can remain empty without breaking local packs.
