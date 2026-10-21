# Listener

## An AI starter gadget for your desk · US$49

A microphone, a recording knob and four customizable keys. Speak a message, email or note into the field you are using with Windows Type.

**US$49 is the hardware price. Type is free and open source.**

| Purpose | Direct download |
| --- | --- |
| **Windows app** | **[Download MSI · Type build111](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **Firmware: USB flashing / Bluetooth OTA** | **[Download firmware ZIP · both methods](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

**English** · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md)

<img src="docs/assets/readme/keyboard-front.jpg" alt="Listener hardware: four transparent keys, black knob and six status lights" width="720">

*Actual Listener hardware. Planned appearance: transparent case and keycaps, black knob, broadly as shown. This is a photograph, not an operation video.*

- **Say it:** the knob starts and stops dictation; Type handles recognition and writing on the computer.
- **Shape the words:** keep the original by default, or choose a text style or translation with a configured text service.
- **Make it yours:** assign the four keys and adjust lights or power settings. Each device keeps its own settings.

**[Hear a short message become text · 25-second software example](docs/VOICE_EXAMPLE.md).** Prerecorded synthetic voice and actual Type results; no hardware operation shown.

**Before you start:** the full hardware-audio path needs Windows Type and a configured recognition service or local model. Cloud usage is billed separately; no ready-to-use official cloud account or allowance is included in this release.

**RC1 is a prerelease.** Strong-interference separation is incomplete and intermittent early-close cases remain under review. [Current limits and release notes](https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.0.6-rc1). Hardware purchase link not yet announced.

[First setup](https://github.com/Listener-ai-Macau/Listener-Type#make-your-first-dictation) · [Look through the user guide when needed](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/docs/USAGE.en.md)

<details>
<summary>Already have a device? Firmware, settings and current build</summary>

Firmware: **ESP32-S3 N16R8**, build `e7bd1f1619b696a1`. Select the same ZIP for USB flashing or Bluetooth OTA in Type.
Type defaults to **USB wired flashing**, with the flasher included in the MSI. No Python, ESP-IDF or separate flashing software is required.
Device settings are independent: select one USB/Bluetooth device, read its settings, then write changes back to that device. Names, lights, power, keys and knob configuration stay on the board. Use this paired MSI and firmware ZIP.

Type build111 keeps the key-layout name field in place while layouts load. Pick a key above its editor and see the selected physical position beside it, with fewer trips up and down the form. Explicit local saved drafts, saved phrases and independent press mappings remain. Firmware33 is unchanged.

[Release notes / 发布说明](https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.0.6-rc1) · [Firmware / 固件](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/tag/v1.0.6-rc1)

</details>

---

## Stay where you are writing

<picture>
  <source media="(prefers-reduced-motion: reduce) and (max-width: 600px)" srcset="docs/assets/listener/Listener-demo-en-mobile-poster.png">
  <source media="(prefers-reduced-motion: reduce)" srcset="docs/assets/listener/Listener-demo-en-poster.png">
  <source media="(max-width: 600px)" srcset="docs/assets/listener/Listener-demo-en-mobile.gif">
  <img src="docs/assets/listener/Listener-demo-en.gif" alt="Workflow animation: focus a field, press and speak, then stop and review text; not a live recording" width="960">
</picture>

1. Place the cursor in the destination field.
2. Press Right Ctrl, speak, then press it again to stop.
3. Review the inserted text; follow the paste prompt if insertion is blocked.

This illustrates the workflow; it is not a live recording. Raw keeps the original by default. Choose a style or translation when needed.

## Make your first dictation

| What you have | Where to start |
| --- | --- |
| A computer | Install, allow microphone access, and select **Computer microphone** in Settings → Recording. |
| A Listener keyboard | Pair `listener` in Settings → Device, check readiness and select **Listener BLE** input. |

Then configure the recognition provider's credentials/model, or prepare a runtime and model in local-ASR settings. Try one sentence in a plain text editor before using it in your daily work.

Windows is the primary installation and full keyboard-audio path. macOS / Linux currently use source builds; see platform scope below.

### Free app, separate service costs

The app is free; cloud costs are set by each provider. Without an API key, prepare a Windows local model and start with Raw.

<details>
<summary>Expand costs, local models and first-use setup</summary>

- **The app:** Listener Type is free and open source. Normal local use needs no Listener account, and the keyboard is optional.
- **Cloud services:** Bring your own recognition or text-provider credentials. Charges, quotas and trial terms are set by each provider. No cloud keys are bundled.
- **No API key:** On Windows, prepare the Foundry Local runtime and a Whisper model in local recognition settings. Choose your computer microphone and try Raw dictation. Local recognition needs no cloud ASR key; download the model first. Speed depends on your computer. Styles, translation and voice QA have separate service requirements.

[Recognition services and local model setup](docs/USAGE.en.md#recognition-services-and-local-models)

</details>

## Choose how your words come out

| Your task | What to use |
| --- | --- |
| Keep the original wording | Raw dictation, recording-capsule preview, cursor insertion and clipboard fallback |
| Tidy a message or email | Light / Formal; optional Everyday list for several action points |
| Prepare instructions for an AI coding tool | Structured; styles support editing and ZIP import/export |
| Write in another language | Select a target, then tap Shift during recording |
| Help with names and terminology | Local vocabulary, presets and literal correction rules |
| Ask about a passage | Select text, open QA with Ctrl+Shift+;, then use the recording key to ask |
| Recover previous text | History, copy, delete and retention controls |

Streaming insertion is enabled in preferences by default; actual incremental insertion depends on the platform, mode and provider. Text in the recording capsule is a preview, not confirmation that it has been inserted into the target field.

**QA setup:** Voice questions currently use Volcengine streaming ASR. Configure its App Key / Access Key and a text provider; another ASR provider or local model selected for dictation does not replace this requirement.

**Raw is the default.** Styles and translation require configuration. For example, Light may turn “um, remind me about tomorrow's three o'clock meeting” into “Remind me about tomorrow's 3 pm meeting.” This illustrates a style, not a guaranteed output.

Want to compare styles before speaking? Use **Overview → Try typed text** with your own draft, then **Copy result**. [Choosing Light or Formal](docs/USAGE.en.md#choose-light-or-formal-for-the-message-you-have) explains what changes to expect; [estimating service costs](docs/USAGE.en.md#estimate-your-own-service-costs) separates audio recognition from text processing and shows where to check actual usage.

## One knob. Four keys. Six status lights.

**US$49 buys the hardware:** its own microphone, Bluetooth audio to Type, a recording/volume knob, four customizable keys and six status lights. The microphone and controls can sit beside your working hand. Each board keeps its own settings.

The free app uses your computer microphone and Right Ctrl. With Listener selected, audio comes from the device microphone instead. Both routes use the recognition service or local model configured on the computer; the hardware price does not include cloud usage. Windows Type is required for the full device-audio path. Review results in noise or overlapping speech.


**The optional keyboard is in its presale phase; a purchase link has not been announced. You can use the app on its own now.**

<picture>
  <source media="(prefers-color-scheme: dark) and (max-width: 600px)" srcset="docs/assets/listener/usage-scene-en-mobile-dark.png">
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/listener/usage-scene-en-dark.png">
  <source media="(max-width: 600px)" srcset="docs/assets/listener/usage-scene-en-mobile.png">
  <img src="docs/assets/listener/usage-scene-en.png" alt="Keyboard and computer input composite; not a live-use photograph" width="1200">
</picture>

Repository keyboard photo and app screenshot; the computer, input field and sample text are illustrated.

Connect in Settings → Device. Click the knob to start/stop, rotate for volume, double-click to reset pairing, or hold until shutdown confirms. Customize click, double-click and long-press actions on the four keys.

Configure wake phrase and voiceprint in **Device settings**. Enroll with three samples of the current phrase, and enroll again after changing it. Voiceprint reduces input interference; review the final result in noise or overlapping speech. Deleting the voiceprint does not disable voice wake. Enrollment creates a protected local template; original sample recordings are not retained after enrollment.

[Hardware controls and light language](https://github.com/Listener-ai-Macau/Listener-Firmware) · [User guide](docs/USAGE.en.md)

## Configure the services and understand the data flow

Settings, vocabulary, styles and history are local. Credentials use the OS credential store. Cloud ASR processes audio; cloud polishing, translation and QA process relevant text. Local ASR with a cloud text provider still sends text. Debug audio is off by default.

Choose direct, system-proxy or custom-proxy routing per provider. The remote marketplace is unconfigured by default; normal local use does not require a Listener backend or account.

## Platform scope

| Platform | Current path |
| --- | --- |
| Windows | MSI; computer microphone and primary Listener BLE audio/device configuration path |
| macOS 12+ | Planned: Apple Speech recognition, Accessibility insertion, packaged app. Today: build from source — computer microphone, local-model recognition, global shortcut |
| Linux | Source build; computer microphone; best-effort X11 shortcuts or desktop-bound CLI commands on Wayland |

The standard installer does not register a system IME. Wake and voiceprint do not guarantee speaker separation at every distance or in every noisy environment.

<details>
<summary>View the app interface</summary>

![Listener Type overview](docs/assets/listener/overview-en.png)

Existing repository screenshot; statistics and provider settings are examples.

</details>

## Documentation and development

For connection, recording or insertion problems, start with panel 08 of the foldout. If the issue persists, read [Support](SUPPORT.md) and [report the issue](https://github.com/Listener-ai-Macau/Listener-Type/issues) with your app version, operating system and input source. Include the firmware version when using the keyboard.

[Usage](docs/USAGE.en.md) · [Feature catalog](docs/product/features.en.md) · [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

Tauri 2 / Rust / React / TypeScript / Vite. Full desktop builds also require the `third_party/denzic-platform` submodule.

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

App and firmware have separate open-source repositories.

[Apache-2.0](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/LICENSE)
