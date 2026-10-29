# Listener v1.0.6-rc1 · Type build238 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Local translation checks common explicit negative-number forms and a specific case where an unspecified Chinese 元 amount becomes an ungrounded dollar amount. If verification fails, the app keeps the original and asks you to review or choose another method; it does not guess the intended currency. Some valid expressions can be rejected.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

These bounded checks do not guarantee currency-to-amount association, mixed-currency handling or general translation meaning; compare against the original. Some valid expressions may be rejected. Preferred terms are optional and unmatched entries have no effect. Enhanced CPU translation requires optional components and can stop under critical memory pressure. Speech-service calls and explicit wording suggestions may incur charges. Long files are split into overlapping segments; check boundaries and marked unrecognized segments. Closing the transcription window loses audio and resume progress, while copied or saved text remains. MP3/M4A depend on system codecs. Existing intermittent GATT timeout and historical brownout causes, real live recordings over 60 seconds, other-PC and long-term physical behavior remain unresolved. Wording suggestions require review and may be unavailable.

## File identity

- Type source: `bb621b2f1e0010e4e9bd95ce8222bd82f756cedf` · `build-source/type238-20261029`
- MSI: 299905024 bytes · SHA256 `9c2edb78edb4ecf66db26999c98b5adce09f436a7fbd81736a55d85ee9de3b67`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

本机翻译检查更多明确负数写法，以及原文未指定美元时“元”被译成美元的已复现情况。无法核对时保留原文并提示，不猜测原文币种；部分正确表达也可能被拦下。
