<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type is a desktop voice typing app. Put the cursor where you want to write, start recording, and speak. Listener turns the recording into text and puts it back into the app you were using.

It works with the microphone already in your computer. If you use the Listener voice keyboard, the same workflow is available from the knob and keys, with Bluetooth audio and status lights on the desk.

[简体中文](README.zh.md) · [繁體中文](README.zh-TW.md) · [Download](https://github.com/Listener-ai-Macau/Listener-Type/releases) · [Usage guide](docs/USAGE.md) · [All features](docs/product/features.md)

<p align="center">
  <img src="docs/assets/readme/overview-en.png" alt="Listener Type home screen" width="900" />
</p>

## What you can do

- **Dictate into the app in front of you.** Start and stop with a global shortcut or let Listener end after you finish speaking. A small capsule shows the live result; `Esc` cancels the recording.
- **Choose how the text should read.** Keep the raw transcript, lightly remove filler words and restore punctuation, turn it into structured notes, tidy it for formal writing, or translate it.
- **Teach Listener your words.** Personal names, product terms, abbreviations, hotwords, and correction rules are kept with your local settings.
- **Use cloud or local recognition.** Listener supports Volcengine, OpenAI-compatible batch ASR, Apple Speech, Bailian realtime, macOS Qwen local ASR, and Windows Foundry Local Whisper.
- **Keep working after dictation.** Restyle the previous result, ask about selected text, browse local history, and configure shortcuts for the actions you use most.

When an app does not accept direct insertion, Listener keeps the result on the clipboard and asks you to paste it instead of losing the text.

## Try it

1. On Windows, install the latest MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases). macOS and Linux builds can be run from source.
2. Allow microphone access and choose **Computer microphone** in Settings → Recording.
3. Click into a text field. Press Right Ctrl on Windows or Right Option on macOS, speak, then press the shortcut again.

The [usage guide](docs/USAGE.md) covers providers, writing styles, vocabulary, shortcuts, history, and troubleshooting.

## The Listener voice keyboard

Pair the keyboard from Settings → Device. Click the knob to start or stop a dictation, rotate it for volume or brightness, and assign actions to KEY1–KEY4. The PWR, BLE, REC, AI, OK, and WARN lights show what the keyboard and desktop app are doing.

The device page also manages battery status, light brightness, idle timers, pairing recovery, and firmware updates. Normal OTA updates preserve the Bluetooth bond and device settings.

Voice-triggered recording and an optional three-sample voiceprint can be enabled from the app. They are useful input guards rather than identity authentication. We are still tuning noisy, far-field, and overlapping-speaker use, so those situations need a quick check of the final text.

Read the [voice keyboard handbook](docs/quickstart/voice-keyboard-readme.md) for setup and recovery.

<p align="center">
  <img src="docs/assets/readme/recording-settings-en.png" alt="Listener Type recording settings" width="720" />
</p>

## Your data

Settings, history, vocabulary, styles, and correction rules stay on your computer. Provider credentials use the operating system credential store, and Listener ships without provider keys. Debug audio is optional and off by default.

The normal dictation path does not depend on a Listener-operated account service. Recognition audio is sent only to the provider you choose when you use a cloud provider; local engines keep recognition on the computer.

## Platform notes

Windows is the main release platform and supports both the computer microphone and the Listener keyboard. macOS 12+ supports computer-microphone dictation, Apple and local recognition paths, global shortcuts, and Accessibility insertion. Linux is currently a developer path; microphone dictation works, while shortcut behavior depends on X11 or the Wayland desktop environment.

The standard Windows installer writes into the focused field. It does not install Listener as a system input method. A separate TSF path remains an engineering validation tool.

## This repository

Listener is split across three repositories:

- **Listener Type** owns recording sessions, recognition, writing, insertion, history, settings, and the desktop device experience.
- [**Listener Firmware**](https://github.com/Listener-ai-Macau/Listener-Firmware) owns microphone capture, BLE audio and HID, physical controls, lights, battery and power behavior, diagnostics, and device OTA.
- [**Denzic Platform**](https://github.com/Listener-ai-Macau/Denzic-Platform) keeps the versioned protocols and portable state machines shared by the app and firmware.

The app is built with Tauri 2, Rust, React, TypeScript, and Vite.

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

A full build also needs the pinned `third_party/denzic-platform` submodule. See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow, [SUPPORT.md](SUPPORT.md) for bug reports, and [SECURITY.md](SECURITY.md) for private security reports.

Release notes and checksums live on the [Releases page](https://github.com/Listener-ai-Macau/Listener-Type/releases). Listener Type is available under the [Apache License 2.0](LICENSE). Parts adapted from other open-source projects remain covered by their original terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
