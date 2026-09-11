<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type is a dictation app for the desktop. You put the cursor in a text field, press Right Ctrl, talk, and press it again; the transcribed text is typed into the field. It works in chat windows, browsers, editors, mail — anywhere that accepts typed text. If the target application refuses programmatic input, the text is left on the clipboard instead.

The app is free and open source, and works with whatever microphone your computer already has. It pairs with the [Listener voice keyboard](https://github.com/Listener-ai-Macau/Listener-Firmware), a small desk device whose firmware lives in the companion repository, but the keyboard is optional.

[中文](README.zh.md) · [繁體中文](README.zh-TW.md) · [Usage guide](docs/USAGE.md) · [1.0.5 release notes](docs/release/1.0.5.md)

### Installation

Windows is the primary platform: download the MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) and install it. The installer relies on WebView2 and fetches it automatically when the machine is online.

| | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Microphone dictation | ✓ | ✓ (12+) | ✓ |
| Global hotkey | ✓ | ✓ | X11 best-effort; Wayland needs a desktop-level binding |
| Keyboard Bluetooth audio | ✓ | not yet | not yet |

### Using it

Recording is a toggle, not push-to-talk. Press Right Ctrl (Right Option on macOS) once to start, speak, press again to stop. While it works, a small capsule window shows the stage it's in: recording, transcribing, processing, done. `Esc` cancels a recording in progress, and nothing half-finished gets inserted.

What you get back depends on the style you picked. Raw keeps your words as spoken; Light removes fillers and adds punctuation; Structured and Formal reshape the text for notes and mail. If you've set a target language, Shift marks the next recording for translation. Styles are plain local files, so you can copy a built-in one and edit it, or share packs as ZIP files.

Other built-in shortcuts: `Ctrl+Shift+;` asks a question about the selected text, `Ctrl+Shift+S` runs the last result through a different style, `Ctrl+Shift+O` brings up the app. On macOS, use `Cmd` instead of `Ctrl`.

### How it works

Audio goes from the microphone (or the keyboard, over Bluetooth) to a recognizer; the transcript optionally passes through a style; the result is inserted into the focused field through the OS's native input path.

None of the providers are locked in. Recognition can run through Volcengine's streaming API, any OpenAI-compatible endpoint, Apple Speech, or entirely on-device. Styles can run through Ark, DeepSeek, or any Anthropic- or OpenAI-compatible endpoint. API keys live in the OS credential store and none ship with the app. History, vocabulary, styles and settings never leave the machine — the whole loop works without any Listener server.

A local vocabulary list (names, product terms, abbreviations) is fed to both the recognizer and the style pass. That's how it learns to stop mangling your colleagues' names.

### The keyboard

The Listener keyboard is a USB-C desk device with a clickable knob (start/stop, double-click to re-pair, turn for volume), four keys you can bind in the app, and six LEDs for power, Bluetooth, recording and processing state.

With the keyboard, you can also start hands-free: the device waits for a wake phrase (default: 「开始录音」), and an optional three-sample voiceprint keeps playback of other people's speech out of your text. The voiceprint is input protection, not authentication — treat it accordingly.

The app works fine without the keyboard; the keyboard just puts the record button under your finger. Its firmware is open source: [Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware).

### What it doesn't do

Worth being explicit, since 1.0.5 is what's shipping:

- The keyboard's Bluetooth audio is Windows-only for now.
- Far-field pickup and two people talking at once are still unreliable; that's 1.0.6 work.
- It isn't a system IME and doesn't try to be — it types into the focused field.

The complete feature list (including what each feature doesn't do) is in [docs/product/features.md](docs/product/features.md).

### Building from source

The app is Tauri v2: a React + TypeScript frontend over a Rust backend, plus a small native text service for Windows insertion.

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

A full desktop build additionally needs the `third_party/denzic-platform` submodule. [CONTRIBUTING.md](CONTRIBUTING.md) covers the rest; security reports go to [SECURITY.md](SECURITY.md). The source is open; the Listener Type name, icon and mascot are not licensed for renamed forks.
