# Listener Type — Windows usage

**English** · [Chinese](USAGE.md)

This guide covers the Windows setup and user controls shown on the current product pages. Check the installed app and firmware versions before applying device settings. The keyboard is optional; use a computer microphone to try the app first.

## Install and make your first dictation

1. Download the current Windows MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases). Match its checksum against that release's `SHA256SUMS.txt`; filenames and hashes belong to the specific release.
2. Install and open Listener Type. Allow microphone access. Windows installation uses WebView2; prepare its runtime first on an offline computer.
3. In Settings → Recording, choose Computer microphone. To use a keyboard, charge it over USB-C, click the knob to power on, and start pairing in Settings → Device. Select `listener` in Windows Bluetooth, return to Type and check the connection. Choose Listener BLE as the recording source.
4. Configure a recognition provider's credentials and model, or prepare the Windows local recognition runtime and model. Try Raw in a plain text editor before adding other services.
5. Focus the destination field. Press Right Ctrl or click the keyboard knob, speak, and press again to stop. Review the final text. If the application blocks insertion, follow the paste prompt or copy from History.

Press Esc while recording to cancel the session. The recording capsule displays a preview; its text does not establish that insertion has succeeded. Streaming insertion is enabled in preferences by default, but actual incremental insertion depends on the platform, provider and mode. The standard installer does not register a system input method.

## Recognition services and local models

The app is free and open source. Normal local use needs no Listener account. Cloud recognition and text providers charge separately; the app does not include cloud keys.

Choose the provider in Settings and enter the required credentials, endpoint and model. Check the provider's own account and quota. For Windows local recognition, prepare Foundry Local and a Whisper model before recording. Downloading a model requires network access; recognition speed depends on the computer. Local recognition needs no cloud ASR key.

Raw is the default and filler cleanup is off by default. Styles, translation and selection questions require a text provider. Voice questions specifically require Volcengine streaming ASR App Key / Access Key as well; a different dictation provider or local model does not replace that configuration.

Network routing is configurable per provider: Direct, System or Custom. Custom routing needs a complete proxy URL. A configured proxy must still be running. Local recognition with a cloud text provider still sends text to that provider.

## Text, styles and translation

| Task | Controls |
| --- | --- |
| Preserve wording | Start with Raw. Review the final transcript. |
| Polish a message | Select Light, Structured or Formal in Styles after configuring a text service. |
| Customize a style | Clone and edit a style pack; import or export its ZIP. |
| Translate | Select a target on the Translation page. Tap Shift during recording to mark that session for translation. |
| Correct terminology | Add terms, notes or presets in Vocabulary. Literal replacement rules help with repeated errors. |
| Recover a result | View, copy, delete or clear History. Retention defaults to seven days and is adjustable. |

Style and translation failures preserve a usable original where possible. Vocabulary helps recognition but cannot guarantee an exact result. Remote style-market features are unconfigured by default and are separate from local style packs.

## Selection questions

Select text in another application and press Ctrl+Shift+; to open Ask. Check that the intended selection has been captured, then use the recording key to ask and to stop. Configure the required services first. Closing the window clears this conversation.

Selected text is sent to the configured text provider. Long selections are bounded; review the captured passage before asking. Selection questions have a separate history preference.

## Shortcuts and preferences

| Action | Windows default |
| --- | --- |
| Start / stop dictation | Right Ctrl |
| Cancel recording | Esc |
| Open selection questions | Ctrl+Shift+; |
| Restyle the previous result | Ctrl+Shift+S |
| Open Listener Type | Ctrl+Shift+O |
| Mark the session for translation | Shift while recording, with a target selected |

Settings include shortcut customization, language, theme, autostart, recording capsule, mute, clipboard and streaming insertion. Automatic clause insertion continues while speaking; after completion, the clipboard retains the whole submitted session body once the last paste is consumed. A user copy cancels pending retention. Editors without readback keep the safe paste payload, and the whole body remains available in History. Auto-send is off by default; optional send actions are Enter or Ctrl+Enter. Closing the main window can hide the app to the tray; choose Quit from the tray to exit fully. App autostart and keyboard automatic shutdown are separate settings.

## Keyboard controls and mappings

| Control | Default action |
| --- | --- |
| Knob click | Start / stop dictation; power on after shutdown |
| Knob rotation | System volume; brightness or disabled can be selected |
| Knob double-click | Reset Bluetooth pairing |
| Knob hold | Hold until shutdown confirms |
| KEY1 / KEY2 | Right Ctrl / Ctrl+C |
| KEY3 / KEY4 | Ctrl+V / Enter |

Map key click, double-click, long-press and knob click in Device settings. Options include dictation, copy/paste, undo, styles, translation, questions, templates, shortcuts, opening apps or disabling the action. Key double-click and long-press default to disabled. Knob double-click and hold remain reserved for hardware recovery and shutdown.

## Lights and power

PWR indicates battery or charging. BLE distinguishes pairing/reconnection, Type not ready and Type ready. REC follows recording audio; AI indicates transfer or processing; OK confirms completion or OTA progress; WARN directs you to an error. See the [English keyboard controls and status guide](https://github.com/Listener-ai-Macau/Listener-Firmware#defaults-you-can-make-your-own) and the [English foldout](manuals/Listener-fold-EN.pdf).

Battery defaults are low power after one idle minute and shutdown after ten. USB defaults are low power after three minutes and no automatic shutdown. A key or knob wakes low-power mode; click the knob after actual shutdown. Adjust brightness separately for status, keys, knob and edge zones. A minute value of zero disables that timer. Connect the keyboard and verify a successful write before assuming a setting reached the device. Low power retains PWR feedback; an unknown battery reading is not zero percent.

## Wake phrase and voiceprint

Connect the keyboard and check automatic start and the displayed wake phrase in Settings → Device. Apply changes and check that they were saved. Follow enrollment and say the current phrase three times, about nine seconds in total. Re-enroll after changing the phrase. The protected local template is retained; original enrollment recordings are discarded after completion.

Without enrollment, anyone saying the correct phrase may trigger recording. Deleting a voiceprint does not disable wake; turn automatic start off separately. Automatic start and stop default to on in current device settings; check the saved values and stop manually when needed. Voiceprint reduces input interference and is not identity authentication. Noise, distance and overlapping voices require checking the final text.

With wake enabled, candidate audio may be sent to the computer before a full session starts. REC off does not mean the microphone is fully off.

## Updates, recovery and troubleshooting

Check app updates manually. Download a matching keyboard OTA ZIP from [firmware Releases](https://github.com/Listener-ai-Macau/Listener-Firmware/releases), select it in Device settings, keep power and connection stable, then check the firmware version after reboot. Ordinary OTA preserves pairing and settings.

Double-click the knob to re-pair. If necessary, remove the old Windows `listener` entry before pairing again. Pairing reset is distinct from factory erase.

| Symptom | Check |
| --- | --- |
| No recording or no text | Input source, microphone access, keyboard connection, provider credentials/model and network |
| Shortcut does nothing | Shortcut configuration, permission status and conflicts with another app |
| Text is not inserted | Test a plain text editor; paste from the clipboard or History if the target blocks input |
| Cloud service only works with a proxy | The provider's Direct/System/Custom setting and the proxy endpoint |
| Wake is unreliable | Current phrase, saved settings, distance, noise and voiceprint enrollment |
| Keyboard seems asleep | Wake it with a key or knob; after shutdown use the knob |
| No charging | A different cable and compliant 5 V supply |

Use About for device recovery and diagnostic export. A diagnostic package omits keys and transcript text but can attach saved debug audio. Debug recording is off by default. Review logs and packages before sharing; use [Support](../SUPPORT.md) and report versions, input source, exact steps and the observed result.

## Data and care

Preferences, vocabulary, styles and history are local; credentials use the OS credential store. Cloud recognition processes audio. Cloud polishing, translation and questions process relevant text. Local recognition alone does not make every configured operation offline.

Keep the keyboard microphone clear and charging ventilated. Do not open, crush, pierce, short, immerse or burn the battery. Disconnect for unusual heat, odor, leaking or swelling and follow local recycling rules. Batch specifications and supplied items must be confirmed against the physical product's documents.
