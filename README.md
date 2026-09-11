<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type is a dictation app for the desktop. You put the cursor in a text field, press Right Ctrl, talk, and press it again; the transcribed text is typed into the field. It works in chat windows, browsers, editors, mail — anywhere that accepts typed text. If the target application refuses programmatic input, the text is left on the clipboard instead.

The app is free and open source, and works with whatever microphone your computer already has. It pairs with the [Listener voice keyboard](https://github.com/Listener-ai-Macau/Listener-Firmware), a small desk device whose firmware lives in the companion repository, but the keyboard is optional.

[中文](README.zh.md) · [繁體中文](README.zh-TW.md) · [Usage guide](docs/USAGE.md) · [1.0.5 release notes](docs/release/1.0.5.md)

### Installation

Windows is the primary platform: download the MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) and install it. The app also runs on macOS and Linux with the computer's microphone; the keyboard's Bluetooth audio path has only been validated on Windows.

### Using it

Recording is a toggle, not push-to-talk. Press Right Ctrl (Right Option on macOS) once to start, speak, press again to stop. While it works, a small capsule window shows what stage it's in — recording, transcribing, processing, done. `Esc` cancels a recording in progress.

What you get back depends on the style you picked. Raw keeps your words as spoken; Light removes fillers and adds punctuation; Structured and Formal reshape the text for notes and mail. If you've set a target language, Shift translates instead. Styles are plain local files — you can copy the built-in ones and edit them.

A few things worth knowing:

- You can teach it vocabulary. Names, product terms and abbreviations from a local list go to both the recognizer and the rewriting step, which cuts down on misheard jargon.
- With the keyboard paired, it can start hands-free: the device waits for a wake phrase (default: 「开始录音」). Enrolling three samples of your voice keeps playback of other people's speech out of your text. That's protection against stray input, not authentication — treat it accordingly.
- Nothing is locked in. Recognition can run through Volcengine, an OpenAI-compatible endpoint, Apple Speech, or fully on-device; rewriting through Ark, DeepSeek, or Anthropic-compatible endpoints. Keys live in the OS credential store and none ship with the app. History, styles and settings stay on your machine.

The complete feature list (including what each feature doesn't do) is in [docs/product/features.md](docs/product/features.md).

### The keyboard

The Listener keyboard is a USB-C desk device with a clickable knob (start/stop, double-click to re-pair, turn for volume), four keys you can bind in the app, and six LEDs for power, Bluetooth, recording and processing state. The app works fine without it; the keyboard just puts the record button under your finger. Its firmware is open source: [Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware).

### What it doesn't do

Worth being explicit, since 1.0.5 is what's shipping:

- The keyboard's Bluetooth audio is Windows-only for now.
- Far-field pickup and two people talking at once are still unreliable; that's 1.0.6 work.
- It isn't a system IME and doesn't try to be — it types into the focused field.

### Building from source

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

A full desktop build additionally needs the `third_party/denzic-platform` submodule. [CONTRIBUTING.md](CONTRIBUTING.md) covers the rest; security reports go to [SECURITY.md](SECURITY.md). The source is open; the Listener Type name, icon and mascot are not licensed for renamed forks.
