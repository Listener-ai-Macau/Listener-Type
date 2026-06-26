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

- Official update metadata uses `latest-{{target}}-{{arch}}.json` and starts at Listener Type `1.0.0`.
- Development builds may be shared manually with explicit hashes, but they are not an updater channel.
- Marketplace and OAuth are not production channels until a Listener Type backend exists.
- Homebrew or other distribution channels must be created under Listener Type ownership.
- Windows installers use the Microsoft Edge WebView2 Evergreen Runtime through the Tauri `downloadBootstrapper` strategy. Offline or factory-image distribution must be a separately labeled variant with a preinstalled runtime or an `offlineInstaller` build.
- Public Windows artifacts must be Authenticode-signed. Unsigned artifacts are for local development only and must be shared with SHA256 hashes plus an explicit SmartScreen/unknown-publisher warning.
- The installer UX must leave a visible diagnostic path: **Settings -> About -> Export diagnostic package** creates the 2.5/3.1 minimal JSON package without audio, transcripts or API key values.

Run `npm run check:brand` and `npm run check:cloud` after touching packaging, installer, updater, OAuth, marketplace or docs.
