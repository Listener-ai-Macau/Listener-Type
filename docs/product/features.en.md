# Listener Type features

**English** · [Chinese](features.md)

Listener Type converts speech to text at your cursor. It can use a computer microphone independently; the optional Listener keyboard adds capture, physical controls and visible status. The current full keyboard-audio path is primarily Windows.

| Need | Feature and entry point |
| --- | --- |
| Start and stop recording | Right Ctrl on Windows, or the keyboard knob; Esc cancels |
| See progress | Recording capsule; text there is a preview until insertion is confirmed |
| Preserve wording | Raw, the default output mode |
| Polish text | Light, Structured or Formal with a configured text provider |
| Customize expression | Editable local styles; ZIP import/export |
| Translate | Target language selection and Shift during recording |
| Improve terminology | Vocabulary terms, notes, presets and literal correction rules |
| Ask about a selection | Ctrl+Shift+; opens Ask; check the captured passage |
| Recover recent text | History, copying, deletion and retention preferences |
| Start by voice | Configurable wake phrase and optional three-sample voiceprint enrollment in Device settings |
| Customize the hardware | Four key mappings and knob click; independent light-zone brightness |
| Save power | Separate plugged/battery timers, low power and automatic shutdown |
| Maintain the product | App updates, matching keyboard OTA, pairing recovery and diagnostics |

The app is free and open source. Cloud services bill separately and use your credentials. Windows local recognition can start without a cloud ASR key after its runtime and model have been downloaded. Styles, translation and voice questions have additional provider requirements.

Voice questions specifically require Volcengine streaming App Key / Access Key and a text provider. A local dictation model does not replace that requirement.

Settings, vocabulary, style packs and history are local. Cloud ASR processes audio; cloud text services process the relevant text. Review diagnostic exports because saved debug audio can be included. Wake and voiceprint reduce interference but do not guarantee separation in noise, at distance or when speakers overlap. Deleting a voiceprint does not turn off automatic wake.

For setup, shortcuts, controls, status lights and troubleshooting, read the [Windows usage guide](../USAGE.en.md). For the physical keyboard, read the [English firmware page](https://github.com/Listener-ai-Macau/Listener-Firmware). The [English foldout manual](../manuals/Listener-fold-EN.pdf) covers everyday operation and care.
