<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener.ai app icon" width="128" />
</p>

<h1 align="center">Listener.ai</h1>

<p align="center">
  <strong>Local-first voice input for macOS and Windows.</strong><br/>
  Press a hotkey, speak, and insert clean text wherever your cursor is.
</p>

<p align="center">
  <a href="README.zh.md">中文</a> ·
  <a href="docs/USAGE.md">Usage</a> ·
  <a href="specs/ARCHITECTURE.md">Architecture</a> ·
  <a href="specs/traceability/files.md">Code Traceability</a>
</p>

## What It Does

Listener Type turns speech into usable written text at the current cursor. It supports raw transcript, light cleanup, structured prompt writing, formal writing, translation, vocabulary hotwords, history, style packs, a floating capsule, and a selection QA panel.

The app is local-first:

- User preferences, style packs, vocabulary, history and recordings live on the local machine.
- Provider credentials are stored in the platform credential vault.
- Cloud ASR/LLM providers are bring-your-own-key.
- Remote marketplace and OAuth are disabled until a Listener Type backend is configured.

## Brand Identity

Listener.ai is represented by a small puppy companion resting on the app's voice capsule. The mascot is intentionally simple: rounded floppy ears, happy closed eyes, soft cheeks, chunky paws, and a calm expression that makes voice input feel approachable instead of technical.

The capsule beneath the puppy mirrors the in-app recording surface. Its five rounded vertical bars echo the live audio-level animation used while recording, with a warm amber center bar as the only highlight. The icon uses the product UI palette: quartz off-white surfaces, muted sage outlines, deep ink facial details, soft line gray, and a restrained warm amber accent.

The source brand asset is kept at [docs/assets/brand/listener-ai-app-icon.png](docs/assets/brand/listener-ai-app-icon.png), and generated desktop icon assets live under [src-tauri/icons](src-tauri/icons).

## Status

- Tauri 2 + Rust backend + React/TypeScript frontend.
- macOS 12+ and Windows 10+ target.
- ASR: Volcengine streaming, OpenAI-compatible batch ASR, Apple Speech, local Qwen ASR, Windows Foundry Local.
- Polish providers: Ark, DeepSeek/OpenAI-compatible, Anthropic-compatible, custom OpenAI-compatible endpoints.
- Windows insertion: native hook path plus Listener Type TSF IME.
- Auto-update metadata points at [Listener-ai-Macau/Listener-Type](https://github.com/Listener-ai-Macau/Listener-Type).

## Install

Release artifacts will be published from the Listener Type repository.

- macOS: download the `.dmg`, drag Listener Type to Applications, then grant Microphone and Accessibility permissions.
- Windows: run the setup executable and allow Microphone access. The installer registers the Listener Type TSF IME used by the Windows insertion bridge.

See [docs/quickstart/installation.md](docs/quickstart/installation.md) and [docs/quickstart/permissions.md](docs/quickstart/permissions.md).

## Build

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

Run audits before shipping:

```bash
npm run check:brand
npm run check:cloud
npm run check:traceability
```

## Documentation

- [Usage](docs/USAGE.md)
- [Provider setup: Volcengine](docs/setup/volcengine.md)
- [Windows build](docs/platform/windows-build.md)
- [Windows IME](docs/platform/windows-ime.md)
- [Dictation pipeline](docs/features/dictation-pipeline.md)
- [Style pack marketplace and local fallback](docs/features/style-pack-marketplace.md)
- [Updater and release channels](docs/release/updater.md)
- [Branding and channels](docs/release/branding-and-channels.md)
- [Tauri CSP](docs/security/tauri-csp.md)
- [Architecture](specs/ARCHITECTURE.md)
- [Design](specs/DESIGN.md)
- [Traceability](specs/traceability/files.md)

## Repository

Remote anchor: [github.com/Listener-ai-Macau/Listener-Type](https://github.com/Listener-ai-Macau/Listener-Type).
