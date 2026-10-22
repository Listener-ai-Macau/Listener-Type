# Listener v1.0.6-rc1 · Type build129 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Opening **Text-service settings** from a text workspace now keeps that workspace open underneath, whether you started from Overview, Style or a recorded transcript. Previously, the Overview and Style routes closed it: your input remained, but an edited result or a response still arriving could be lost.

Your original draft, selected style and reviewed result stay available on return. A response already requested can finish while settings are open. Returning rechecks service readiness and does not automatically send your text again. Existing save-failure feedback and explicit return remain.

The local Help center, explicit saved drafts, recording settings and Firmware33 are retained. This update does not change your configured accounts, daily dictation mode, firmware, recognition or Bluetooth transport. USB flashing and Bluetooth OTA use the same ZIP.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and have separate usage charges; this release does not include a ready-to-use official cloud account or allowance. Review important text. Optional rewriting can still require manual edits.

RC1 is a prerelease. Strong background speech separation remains incomplete and intermittent early-close cases remain under review. This update does not qualify new live-wake, automatic-ending, global-input or physical-hardware behavior. It has not been installed over the daily Type instance as part of this publication.

## File identity

- Type source: `a793bce05c3428a5519c2a64a4311183832e8b1b` · `build-source/type129-20261022`
- Type129 MSI: 299110400 bytes · SHA256 `8a5987ed586212d5f20ebaf0862b53e0e53c6c5f0dfed33bcb3e2ce52bcf5007`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- Firmware ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

129统一文字工作区的设置窗口生命周期。首页、风格页和录音稿入口进入文字服务设置后，原稿、手改结果及已开始的处理返回都会保留；返回不重发文字，设置写入失败仍明确提示。49美元为纯硬件价，云服务准备与用量另计。固件、日常录音与蓝牙未改，改写质量、强干扰与偶发提前结束的原有限制保留。
