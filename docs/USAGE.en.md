# Listener Type — Windows usage

**English** · [Chinese](USAGE.md)

This guide covers the current [v1.0.6-rc1 Windows candidate](https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.0.6-rc1). Check its build, checksum and known limits on the release page, and use the paired [firmware release](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/tag/v1.0.6-rc1). The keyboard is optional; use a computer microphone to try the app first. Earlier 1.0.5 release documents are historical.

## No API key: choose a starting point

| What you want | Current path | What it requires |
| --- | --- | --- |
| Turn speech into text | Prepare Foundry Local and a Whisper model in Windows local-recognition settings; select Computer microphone and start with default Raw | Internet and disk space for the first download; memory while running. Speed depends on the computer. No cloud ASR key |
| Remove fillers or organize a message automatically | Configure an available text service, then select Light, Structured or Formal | Separate from recognition; a downloaded speech model does not also provide these text services. Provider charges may apply |
| Use your existing cloud recognizer | Enter that provider's required credentials, endpoint and model, then try Raw | Provider availability, quota and charges |

You can view, edit and import/export built-in style packs locally. **That does not make Light text processing offline.** Default Raw needs no text service; a customized Raw pack can have different requirements. Check its card and trial status. “Configured” does not guarantee remaining quota or a successful request.

### Windows clicks for the no-key path

1. Open **Settings → Providers**. In the speech-recognition section, click **Prepare local recognition**. You do not need to select a cloud provider or enter a key first.
2. On **Windows Foundry Local Whisper**, keep the default **Whisper Small (whisper-small)**. Base and Tiny are smaller alternatives with a possible accuracy tradeoff.
3. Check the displayed model size and runtime status. Size comes from the current catalog; “unknown” means it has not been obtained, not a zero-byte download. Total disk use also includes the runtime and extracted files.
4. Click **Prepare and use local recognition** and wait for download, loading and activation. Cancel or failure preserves the old provider. **Download / load only** does not activate it. Check the loaded-model and active status before recording.
5. Select **Computer microphone** in Recording settings and keep default **Raw** in Styles. Focus a writable field, press Right Ctrl, speak, and press again to stop and review.

This path does not provide automatic polishing or instant use before downloading. Use the size currently shown to decide whether to proceed; unknown download and running-memory sizes are not fixed promises.

## Install and make your first dictation

1. Download the current Windows MSI from [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases). Match its checksum against the SHA-256 table in the release description; filenames and hashes belong to the specific release.
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

### Estimate your own service costs

Check **Overview** for the selected speech recognizer and text provider; **Settings → Providers** shows their configuration. They can be different services. “Configured” describes setup, not a balance or a free allowance. Type does not currently display a consolidated provider bill or remaining credit.

| What you do | Which service may charge |
| --- | --- |
| Default Raw dictation using cloud recognition | The selected speech recognizer for audio |
| Dictation with Light, Structured, Formal or translation | Speech recognition, plus the text provider for the text operation |
| A typed-text trial with a non-Raw or customized style | The text provider; this trial does not submit audio to a recognizer |
| Default Raw typed-text trial | No service request |

To estimate recognition use, multiply your average spoken minutes per day by the number of days you expect to use it. For example, 5 minutes on each of 20 days is 100 spoken minutes. Apply your provider's current charging unit, rates and account terms; some services use a different unit or minimum. For text operations, check that provider's input/output usage and rates after a representative short trial. Text length, the saved prompt and output length can all affect usage. The response time shown in Type is not a price estimate.

Use the provider's own account page for actual usage and remaining credit. Hardware at US$49 does not include these charges in the current release. No monthly dollar amount is promised here because the selected provider, its rates and your usage determine it.

## Text, styles and translation

### Choose Light or Formal for the message you have

**Light** keeps your conversational voice and cleans up fillers, punctuation and flow. **Formal** aims for restrained workplace wording: complete sentences and a professional tone, without adding promises, facts or stock greetings. It is not an automatic email template. A short message that is already clear may change very little in either style; selecting Formal does not guarantee a longer message, subject line or signature.

For example, an illustrative draft, “hey Maya can we move Thursday's review to Friday at 10:30 I can send the link by 6 pm Thursday,” could keep its chat tone in Light or use a more restrained request in Formal. In both, Maya, the proposed change, the times and **can send** must remain intact. A proposal must not become a confirmed meeting, and “can send” must not become a promise. These are examples of what to compare, not guaranteed model outputs.

From **Overview → Try typed text**, try a saved style with your own non-sensitive draft and compare the result before using it for dictation. This uses the selected saved prompt without activating it. **Copy result** puts the returned text on the clipboard only when you request it; wait for **Copied**, then paste. Processing alone does not save a History entry or write the clipboard. The saved example is an illustration, separate from your result.

| Task | Controls |
| --- | --- |
| Preserve wording | Start with Raw. Review the final transcript. |
| Polish a message | Select Light, Structured or Formal in Styles after configuring a text service. |
| Customize a style | Clone and edit a style pack; import or export its ZIP. |
| Translate | Select a target on the Translation page. Tap Shift during recording to mark that session for translation. |
| Correct terminology | Add terms, notes or presets in Vocabulary. Literal replacement rules help with repeated errors. |
| Recover a result | View, copy, delete or clear History. Retention defaults to seven days and is adjustable. |

Style and translation failures preserve a usable original where possible. Vocabulary helps recognition but cannot guarantee an exact result. Remote style-market features are unconfigured by default and are separate from local style packs.

Open **History → Saved drafts** to find text you explicitly saved. Change **Draft name**, then click **Save name** or press Enter to distinguish similar versions. Renaming keeps the text, attached original, record identity and saved time unchanged. New translation drafts start with labels for local/cloud processing and manual edits; existing names are retained. **Edit a copy** saves revised text as another draft without overwriting the original. Select and delete drafts you no longer need explicitly.

## Voice questions and selection questions

With Listener connected and both speech and answer services configured, say the Chinese command **“开始提问”** followed by your question. The question card appears at the recording capsule position and previews the words being recognized. After you stop speaking, it finishes capture and requests an answer; **Finish and ask** is also available if capture continues. The existing **“开始录音”** command still starts dictation. Questions and answers are not automatically inserted into another app or the clipboard; use Copy when needed. The card defaults to 360×240, is resizable and keeps its adjusted size when reopened.

The button route remains available through **Tools → Selection questions**: open the card, click **Start asking**, speak into the computer microphone, then click **Finish and ask**. This route keeps the computer microphone source without changing the everyday Listener input setting.

To ask about selected text, first record your question shortcut in **Settings → Shortcuts**. Select text in another application and use that configured shortcut to open Ask. Check that the intended selection has been captured before asking. Selected text is sent to the configured text provider; long selections are bounded. The card supports follow-up questions. Closing it clears this conversation. Question history has a separate preference.

Speech recognition and answers use your configured services and their usage charges. Each answer request includes one current local timestamp as context; review important facts. Saved configuration alone does not establish network access, valid credentials or available quota.

## Shortcuts and preferences

From Type build217, all five computer global shortcuts start **Not set**. Record only the shortcuts you need in **Settings → Shortcuts**, or clear them individually. Updates preserve existing saved bindings.

| Action | First-use state |
| --- | --- |
| Start / stop dictation | Not set |
| Mark the session for translation | Not set |
| Open question window | Not set |
| Restyle the previous result | Not set |
| Open Listener Type | Not set |

Configured recording shortcuts use Toggle by default: press once to start and again to stop. You can also use the recording button or Listener knob without configuring a computer shortcut. Esc still cancels an active recording. A configured translation shortcut also requires a target language selected on the Translation page. Check shortcut monitoring in **Settings → Permissions** if your configured binding does not respond. Hardware mappings are separate.

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

Battery defaults are low power after one idle minute and shutdown after ten. USB defaults are low power after three minutes and no automatic shutdown. A key or knob wakes low-power mode; click the knob after actual shutdown. Adjust brightness separately for status, keys, knob and edge zones. A minute value of zero disables that timer. Low power retains PWR feedback; an unknown battery reading is not zero percent.

From build 147, select USB or Bluetooth and the target keyboard in Device settings. Choices and switches save automatically. Finish editing names, templates or numbers and move focus away to save; release a brightness slider to save it. Wait for **Saved to device**. Failed saves keep your edits and offer **Retry save**. Reading, connecting or switching targets never writes default values. The device owns its name, lighting, power, keys and knob settings; USB and Bluetooth read the same configuration, which stays with the keyboard on another computer. The saved-layout library is a separate local draft library: **Apply to current device** explicitly saves a whole layout to the keyboard.

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
