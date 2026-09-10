<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="128" />
</p>

<h1 align="center">Listener Type</h1>

<p align="center">
  <strong>Speak. Text lands at the cursor.</strong><br/>
  Chat, mail, docs, code comments — press, talk, press again.
</p>

<p align="center">
  <a href="README.zh.md">中文</a> ·
  <a href="docs/product/features.md">Features</a> ·
  <a href="docs/release/1.0.5.md">Release notes</a> ·
  <a href="docs/USAGE.md">Usage</a> ·
  <a href="https://github.com/Listener-ai-Macau/Listener-Firmware">Keyboard firmware</a>
</p>

Listener Type is the desktop app for Listener. Try it with your computer microphone. Pair it with the [Listener voice keyboard](https://github.com/Listener-ai-Macau/Listener-Firmware) and start/stop live in your hand.

This is not a meeting recorder and not an “AI writes the perfect email for you” bot. It is **daily voice input**: when you finish speaking, the words are already in the field you were using.

## Why people buy it

The app is free to install. The product you pay for is the loop: keyboard plus Type.

| Typing today | Listener |
| --- | --- |
| Hands leave the thought to hunt keys | Eyes stay on the screen; one click on the knob |
| Voice trapped in a recorder app | Inserted at the live cursor |
| Vendor lock-in for ASR and polish | Your keys, or on-device ASR; prefs stay local |
| Software hotkey only | Real keys, real lights, wake phrase, a desk device |

1.0.5 is a shippable daily loop: start → speak → capsule and LEDs talk back → text at the cursor, clipboard if the target app blocks insert. That is the sales baseline, not a demo reel.

## 30 seconds

```text
Click a text field
    → Right Ctrl (or EC11 on the keyboard)
    → Speak
    → Press again
    → Look at the cursor, not our editor
```

No keyboard: Settings → Recording → microphone.  
With keyboard: Bluetooth name `listener`. Full device audio is Windows-first.

Guides: [Usage](docs/USAGE.md) · [Voice keyboard](docs/quickstart/voice-keyboard-readme.md)

## What you get

| | |
| --- | --- |
| **Any text field** | Notepad, browser, chat, editors. Toggle recording, not hold-to-talk. |
| **A keyboard in the hand** | EC11 click starts/stops; double-click re-pairs; four keys are yours. REC / AI / BLE lights show the stage. |
| **Tone that matches the job** | Raw, Light, Structured, Formal, or translate on Shift. |
| **Your words** | Local vocabulary for ASR and polish. Style packs copy/export without a remote store. |
| **Mostly your voice** | Wake phrase defaults to “开始录音”. Optional three-sample voiceprint. In controlled rooms, playback of someone else should not become the body text. |
| **Local-first** | History and settings on the machine. No bundled vendor keys. Local Whisper works; cloud Chinese ASR is usually stronger. |

Full catalog, lights, and 1.0.5 limits: [Product features](docs/product/features.md)

## What to buy, what to install

- **Start with the app.** Windows is the supported path for device audio. macOS can use the mic and hotkey first.
- **Then the keyboard.** Open firmware: [Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware).
- **Recognition.** Bring your own cloud ASR account, or switch to local ASR. Quality and cost follow the provider you choose.

Current release is **1.0.5**. [Release notes](docs/release/1.0.5.md). Far-field voiceprint and live multi-speaker isolation are 1.0.6. A passing controlled-interference run is not “works in every room.”

## Open source

Read, build, file issues, send PRs. How we write product features: [writing guide](docs/product/writing.md).

Source and docs live here. The Listener Type name, icon, and mascot are not a trademark grant for renamed builds. A full desktop build still needs the `third_party/denzic-platform` submodule. See [CONTRIBUTING.md](CONTRIBUTING.md).

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

Releases: [Listener-ai-Macau/Listener-Type](https://github.com/Listener-ai-Macau/Listener-Type)
