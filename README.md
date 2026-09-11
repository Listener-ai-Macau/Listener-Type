<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Speak, and the text appears at your cursor. Listener Type is a desktop dictation app: click into any text field, press Right Ctrl, talk, press again, and the words are typed in. It's open source, runs on Windows, macOS and Linux, and works with whatever microphone your computer has.

[中文](README.zh.md) · [繁體中文](README.zh-TW.md) · [Usage guide](docs/USAGE.md) · [1.0.5 release notes](docs/release/1.0.5.md)

## Try it in 30 seconds

1. On Windows, download the MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) and install it (it needs WebView2, fetched automatically when online).
2. Launch it, allow the microphone.
3. Click into Notepad, press Right Ctrl, say something, press again.

The words should already be in Notepad. `Esc` cancels mid-recording; if an app refuses programmatic input, the text goes to the clipboard and you're asked to paste it. On macOS the hotkey is Right Option. If that worked, read on.

macOS 12+ and Linux run the app with the computer's microphone; the keyboard's Bluetooth audio is validated on Windows only. On Linux the global hotkey is best-effort under X11, and on Wayland you bind the CLI commands in your desktop environment — details in the [usage guide](docs/USAGE.md).

## A day with it

**Answering messages.** The default Light style removes the "um"s and adds punctuation, nothing else. You talk like you talk; it reads like a person wrote it.

**Mail and formal writing.** Switch to Formal for a tidier tone — it won't invent content. If the other side reads another language, set a target language once, then press Shift before speaking and out comes the translation.

**Notes, tasks, prompts.** Structured organizes what you said by topic and goal. For code comments, or anything you want verbatim, use Raw — it leaves your words alone.

**It learns your words.** Names, product terms and abbreviations go into a local vocabulary that both recognition and polish consult. That's how it stops mangling your colleagues' names.

Three more shortcuts worth remembering: `Ctrl+Shift+;` asks a question about the selected text, `Ctrl+Shift+S` runs the last result through a different style, `Ctrl+Shift+O` opens the app (macOS: `Cmd` instead of `Ctrl`).

## The keyboard on your desk

The [Listener voice keyboard](https://github.com/Listener-ai-Macau/Listener-Firmware) is the companion hardware: a clickable knob for start/stop, double-click to re-pair, turn for volume; four keys you can bind in the app; six LEDs that say where things stand — power, Bluetooth, recording, processing.

Don't want to touch it? Turn on "start on voice" and say 「开始录音」. Enroll three voice samples and playback of someone else's speech won't end up in your text. The voiceprint guards against stray input; it is not authentication, so don't treat it as a lock.

The app works fine without the keyboard — the keyboard just puts the record button under your finger. Its firmware is open source too: [Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware).

## Your data, your keys

Recognition can run through Volcengine's streaming API, any OpenAI-compatible endpoint, Apple Speech, or fully on-device; polish through Ark, DeepSeek, or any Anthropic- or OpenAI-compatible endpoint. Keys live in the OS credential store and none ship with the app. History, vocabulary, styles and settings stay on your machine — the whole loop runs without any Listener server.

## Current limits

Shipping is 1.0.5. Far-field pickup and overlapping speakers aren't reliable yet; that's on the 1.0.6 list. It isn't a system IME — text goes into the focused field. The full feature list, including what each feature doesn't do: [docs/product/features.md](docs/product/features.md).

## Hacking on it

Tauri v2 + React + Rust. Windows insertion goes through a TSF text service in `windows-ime/`; macOS goes through the Accessibility API.

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

A full desktop build also needs the `third_party/denzic-platform` submodule. See [CONTRIBUTING.md](CONTRIBUTING.md) before sending code; security reports go to [SECURITY.md](SECURITY.md). The source is open; the Listener Type name, icon and mascot are not licensed for renamed forks.
