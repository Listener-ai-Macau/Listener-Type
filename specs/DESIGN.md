# Listener Type Design

Listener Type uses the Listener-ai-agent visual direction: quartz paper surfaces, ink text, low-saturation accent color and calm glass panels. It keeps the upstream app workflows but changes the feel and brand.

## Tokens

Primary tokens are in `src/styles/tokens.css`.

| Token | Role |
| --- | --- |
| `--ol-canvas` | outer quartz paper wash |
| `--ol-surface` | elevated paper surface |
| `--ol-ink` | primary text |
| `--ol-ink-3` | secondary text |
| `--ol-blue` | legacy token name, now Listener Type quiet green accent |
| `--ol-blue-soft` | soft accent background |
| `--ol-shadow-*` | quartz-toned shadows |

The `--ol-*` namespace remains for compatibility with the inherited component tree. Do not rename token consumers unless doing a deliberate full UI refactor.

## Shell

- `WindowChrome` provides the paper/glass background.
- `FloatingShell` keeps the compact operational layout: sidebar, tab content and footer.
- Avoid marketing-style hero pages inside the app. The app should open to the usable product.

## Controls

- Buttons use icons where practical.
- Cards are for repeated items, modals and framed tools, not nested page sections.
- Text must fit in buttons and compact panels across narrow desktop windows.
- Letter spacing is `0`.
- Dominant electric-blue styling is not allowed; accent colors should read as muted green/stone.

## Verification

Run `npm run build`, then inspect the main window and capsule in a local Tauri session when available. For visual changes, check text overlap, scrollability and focus rings.
