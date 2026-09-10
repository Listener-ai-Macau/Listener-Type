<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type types what you say. Put the cursor in any text field, press Right Ctrl, talk, press again — the text shows up in that field, not in our window.

It runs on Windows, macOS and Linux. A computer microphone is enough; if you want a physical record button on your desk, it pairs with the [Listener voice keyboard](https://github.com/Listener-ai-Macau/Listener-Firmware).

[中文](README.zh.md) · [繁體中文](README.zh-TW.md) · [Usage](docs/USAGE.md) · [Release notes](docs/release/1.0.5.md)

## Using it

On Windows, download the MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases), install it, allow the microphone. That's the whole setup.

After that it's always the same motion: click into a text field, press Right Ctrl, speak, press Right Ctrl again. If the target app refuses inserted text, the result waits on your clipboard. macOS and Linux use Right Option / Right Alt instead — details in the [usage guide](docs/USAGE.md).

## What it can do

- Type into whatever app has focus: notepad, browser, chat, code editor.
- Clean up what you said — verbatim, lightly tidied, structured, formal, or translated with Shift. Styles are local files; copy and edit them as you like.
- Learn your vocabulary: keep a local list of names and jargon that recognition and cleanup both consult.
- Start hands-free: say 「开始录音」. If you enroll three voice samples, someone else talking — or a recording of you — won't start a session.
- Run on your own accounts. Recognition through Volcengine, OpenAI-compatible endpoints, Apple Speech, or fully offline; cleanup through Ark, DeepSeek, or Anthropic-compatible endpoints. Keys live in the OS credential store, none are bundled.
- Keep everything on the machine: history, styles, vocabulary, settings.

The full list, with platform notes, is in [docs/product/features.md](docs/product/features.md).

## The keyboard

The [Listener voice keyboard](https://github.com/Listener-ai-Macau/Listener-Firmware) is a small USB-C desk device: a knob you click to start and stop, four keys you can bind, and lights for power, Bluetooth, recording and processing. Its firmware is open source too. You don't need it — the app works fine with a microphone — but a real button beats hunting for a hotkey.

## What it doesn't do yet

- The keyboard's Bluetooth audio path is Windows-only for now; on macOS and Linux, use the computer mic.
- The voiceprint prevents accidental recordings. It is not a security feature.
- Far from the mic, or two people talking at once, it still struggles. That's planned for 1.0.6.
- It's not a system IME — it types into the focused field.

## Building from source

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

A full desktop build also needs the `third_party/denzic-platform` submodule. See [CONTRIBUTING.md](CONTRIBUTING.md) before sending code, and [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

The code is open; the Listener Type name, icon and mascot are not — please don't reuse them for a renamed fork.
