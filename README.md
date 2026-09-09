<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type app icon" width="128" />
</p>

<h1 align="center">Listener Type</h1>

<p align="center">
  <strong>Local-first voice input for macOS and Windows.</strong><br/>
  Press a hotkey, speak, and insert clean text wherever your cursor is.
</p>

<p align="center">
  <a href="README.zh.md">中文</a> ·
  <a href="docs/USAGE.md">Usage</a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="docs/release/1.0.5-commercial-readiness.md">1.0.5 boundary</a> ·
  <a href="specs/ARCHITECTURE.md">Architecture</a> ·
  <a href="specs/traceability/files.md">Code Traceability</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

> Looking for the full product manual? Start with the [中文 product guide](README.zh.md), which documents desktop hotkeys, Listener keyboard keys, LED zones, custom actions, voiceprint/interference handling, recovery, privacy, firmware updates, and clipboard behavior.

> **Current release boundary:** 1.0.5 is the usable daily-input baseline. The
> broader owner-enrollment and real-human interference generalization is
> intentionally deferred to 1.0.6. See the [commercial-readiness boundary](docs/release/1.0.5-commercial-readiness.md)
> before using a single acceptance run as a universal product claim.

## What It Does

Listener Type turns speech into usable written text at the current cursor. It supports raw transcript, light cleanup, structured prompt writing, formal writing, translation, vocabulary hotwords, history, style packs, a floating capsule, and a selection QA panel.

The product is designed around a short loop: press once, speak, press again, and keep working in the app that already has focus. With the optional Listener keyboard, physical keys, an EC11 knob, and four independently adjustable LED zones make recording, recovery, and processing status visible without opening the main window. Voiceprint-assisted wake-up and session-level speaker protection help reduce accidental starts and bystander speech, while still leaving the final result reviewable by the user.

The app is local-first:

- User preferences, style packs, vocabulary, history and recordings live on the local machine.
- Provider credentials are stored in the platform credential vault.
- Cloud ASR/LLM providers are bring-your-own-key.
- Remote marketplace and OAuth are disabled until a Listener Type backend is configured.

## Brand Identity

Listener Type is represented by a small puppy companion resting on the app's voice capsule. The mascot is intentionally simple: rounded floppy ears, happy closed eyes, soft cheeks, chunky paws, and a calm expression that makes voice input feel approachable instead of technical.

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

## Is 1.0.5 ready to sell?

The current product experience is coherent enough for a controlled commercial
baseline: the daily dictation loop, output modes, Listener keyboard feedback,
clipboard recovery and the tested owner-plus-computer-playback path are all
documented. Selling or publishing a branded build still requires a separate
license, dependency, signing, privacy and support review. In particular, the
full public build remains **BLOCKED** until the pinned platform submodule is
publicly readable and compatibly licensed. This is a release-governance limit,
not a claim that the desktop experience is unusable.

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

## Open-source development

This repository is intended to be released under the [MIT License](LICENSE).
Read [CONTRIBUTING.md](CONTRIBUTING.md) for the clean-clone setup, checks, and
pull request expectations. Use [SUPPORT.md](SUPPORT.md) for questions and
[SECURITY.md](SECURITY.md) for private vulnerability reports.

The license covers the source code and documentation; the `Listener Type` name,
logo, mascot, and signed release identity remain product marks and may not be
used to imply an official build.

The full Tauri build currently uses the pinned `third_party/denzic-platform`
submodule. Its public license and read access must be confirmed before this
repository can promise a fully reproducible public build. The current release
status is **blocked on that dependency boundary**; see
[docs/OPEN_SOURCE.md](docs/OPEN_SOURCE.md) for the exact boundary.

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
- [Open-source status and boundaries](docs/OPEN_SOURCE.md)
- [1.0.5 commercial-readiness boundary](docs/release/1.0.5-commercial-readiness.md)
- [Changelog](CHANGELOG.md)

## Repository

Remote anchor: [github.com/Listener-ai-Macau/Listener-Type](https://github.com/Listener-ai-Macau/Listener-Type).
