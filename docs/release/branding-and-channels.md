# Branding And Channels

## Product Identity

| Field | Value |
| --- | --- |
| Product name | `Listener Type` |
| Package name | `listener-type` |
| Bundle identifier | `com.listener.type` |
| Rust package | `listener-type` |
| Rust library | `listener_type_lib` |
| Credential service | `com.listener.type` |
| GitHub repository | `Listener-ai-Macau/Listener-Type` |

## Channel Rules

- Stable update metadata uses `latest-{{target}}-{{arch}}.json`.
- Beta metadata uses `latest-{{target}}-{{arch}}-beta.json`.
- Marketplace and OAuth are not production channels until a Listener Type backend exists.
- Homebrew or other distribution channels must be created under Listener Type ownership.

Run `npm run check:brand` and `npm run check:cloud` after touching packaging, installer, updater, OAuth, marketplace or docs.
