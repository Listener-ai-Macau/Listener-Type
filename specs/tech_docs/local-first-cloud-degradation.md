# Local-First Cloud Degradation

Listener Type must keep local dictation and style pack functionality without a Listener Type backend.

## Disabled By Default

- Remote marketplace backend.
- Marketplace upload/like/delete.
- GitHub OAuth device flow.
- Background automatic update checks. Manual checks remain available, and enabling the setting checks only Listener Type release metadata.
- Any production URL outside `Listener-ai-Macau/Listener-Type` release metadata.

## Enabled By Explicit Config

- `LISTENER_TYPE_MARKETPLACE_BASE_URL` enables a future marketplace backend.
- `GITHUB_OAUTH_CLIENT_ID` enables a Listener Type-owned GitHub OAuth app.
- `LISTENER_TYPE_UPDATE_MIRROR_BASE_URL` enables an explicit release asset mirror.

## Audit

```bash
npm run check:cloud
```
