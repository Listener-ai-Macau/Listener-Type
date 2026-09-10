<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="128" />
</p>

<h1 align="center">Listener Type</h1>

<p align="center">
  <strong>Speak. Text lands at your cursor.</strong><br/>
  Open-source voice input for the desktop — in whatever app you are already using.
</p>

<p align="center">
  <strong>English</strong> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.zh-TW.md">繁體中文</a>
</p>

<p align="center">
  <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases"><img src="https://img.shields.io/github/v/release/Listener-ai-Macau/Listener-Type" alt="Release" /></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-blue" alt="Platform" />
</p>

<p align="center">
  <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases">Download</a> ·
  <a href="docs/USAGE.md">Usage</a> ·
  <a href="docs/product/features.md">Features</a> ·
  <a href="docs/release/1.0.5.md">Release notes</a> ·
  <a href="https://github.com/Listener-ai-Macau/Listener-Firmware">Keyboard firmware</a>
</p>

<!-- Hero: add a product screenshot or short demo GIF here once one exists. -->

Listener Type is a local-first dictation app: press a key, talk, press again — the words are typed into the field you were using. It is not a recorder and not a chatbot window: the text goes where your cursor already is.

It is free and open source (Tauri + React + Rust). Your computer's microphone is enough to start. Pair it with the [Listener voice keyboard](https://github.com/Listener-ai-Macau/Listener-Firmware) — a small desk device with a knob, four real keys, and status lights — and start, stop, and "did it hear me?" move into your hand, off the screen.

## Get started in 30 seconds

**Windows:** download the MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases), install, allow the microphone.

```text
Click any text field
  → press Right Ctrl (or click the keyboard knob)
  → speak
  → press again
  → the text is already at the cursor
```

No keyboard? Settings → Recording → input source → microphone.
Have the keyboard? Pair the Bluetooth device named `listener` — the full device audio path is Windows-first today; on macOS and Linux, use the computer microphone.

Step by step: [Usage guide](docs/USAGE.md) · [Voice keyboard handbook](docs/quickstart/voice-keyboard-readme.md)

## Why it feels different

| | |
| --- | --- |
| **Types where you work** | Notepad, browser, chat, code editor — the text is inserted into the target app, and waits on the clipboard if an app refuses insertion. Press to start, press to stop; no holding a key. |
| **A keyboard in your hand** | Click the knob to start/stop, double-click to re-pair, bind four keys to your own actions. PWR / BLE / REC / AI lights show exactly which stage you are in. |
| **Polish for the occasion** | Raw transcript, light cleanup, structured notes, formal email — or translate with Shift. Style packs are local files: copy, edit, export, no store account. |
| **Your words, literally** | A local vocabulary of names, products, and jargon feeds both recognition and polish. |
| **Wake by voice** | Say 「开始录音」 and recording starts. Optional three-sample voiceprint keeps someone else's playback out of your text. |
| **Your providers, your keys** | Recognition via Volcengine, OpenAI-compatible endpoints, Apple Speech, or fully on-device ASR; polish via Ark, DeepSeek, or Anthropic-compatible endpoints. Keys live in the OS credential store; none are bundled. |
| **Local-first** | History, styles, vocabulary, and settings stay on your machine. The app works without any Listener backend. |

Full catalog, light meanings, platform notes: [Product features](docs/product/features.md)

## What 1.0.5 does not promise

- Keyboard BLE audio is Windows-first; macOS and Linux use the computer microphone for now.
- Voiceprint is input protection against stray speech — not identity verification, not a door lock.
- Far-field voiceprint and live multi-speaker isolation are 1.0.6 work, not shipped here.
- It is not a system IME; it inserts into the focused field.

## Build from source

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

A full desktop build also needs the `third_party/denzic-platform` submodule. Read [CONTRIBUTING.md](CONTRIBUTING.md) before sending code; report vulnerabilities via [SECURITY.md](SECURITY.md).

The source is open to read, build, and modify. The Listener Type name, icon, and mascot are not a trademark grant for renamed builds.
