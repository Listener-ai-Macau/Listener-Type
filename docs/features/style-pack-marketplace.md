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

## Source Map

- Frontend marketplace UI: `src/pages/Marketplace.tsx`, `src/components/MarketplaceModal.tsx`, `src/pages/Style.tsx`.
- IPC wrappers: `src/lib/ipc.ts`.
- Backend commands: `src-tauri/src/commands.rs`.
- Local pack storage and archive import/export: `src-tauri/src/persistence.rs`.

## Verification

```bash
npm run check:cloud
npm run build
```

Manual smoke: open Style, create a pack, export it, import it, and confirm the remote marketplace can remain empty without breaking local packs.
