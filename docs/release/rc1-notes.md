# Listener v1.0.6-rc1 · Type build239 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

When a user-selected term marker is corrupted in a local Chinese translation, the app retries once locally with a different marker. If it still cannot preserve the selected terms, it keeps the original and reports the problem. The retry can be cancelled and does not use a cloud service. Select terms explicitly and review the full translation.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

These bounded checks do not guarantee currency-to-amount association, mixed-currency handling or general translation meaning; compare against the original. Some valid expressions may be rejected. Preferred terms are optional and unmatched entries have no effect. Enhanced CPU translation requires optional components and can stop under critical memory pressure. Speech-service calls and explicit wording suggestions may incur charges. Long files are split into overlapping segments; check boundaries and marked unrecognized segments. Closing the transcription window loses audio and resume progress, while copied or saved text remains. MP3/M4A depend on system codecs. Existing intermittent GATT timeout and historical brownout causes, real live recordings over 60 seconds, other-PC and long-term physical behavior remain unresolved. Wording suggestions require review and may be unavailable.

## File identity

- Type source: `5146da8ceae8683dc7d11d8c585d5d583bfefcb8` · `build-source/type239-20261029`
- MSI: 299913216 bytes · SHA256 `3e527a4424a971b53ed034fd6f312e0ac0472df993f2e3cb589dac756b2eb825`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

中文本机翻译中，用户指定术语的保护标记损坏时，软件会换标记在本机重试一次；仍无法保留时保留原文并提示。重试可取消，不使用云端。术语需自行指定，整句仍需核对。
