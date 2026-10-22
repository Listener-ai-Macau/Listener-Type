# Listener v1.0.6-rc1 · Type build132 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

The main navigation now has three entries: **Overview, History and Tools**. Open Tools for Style, Vocabulary, Translation and Selection Ask. Each tool has a short description and a return link; settings remain at the gear. Existing device page shortcuts still open their original pages.

The update also restores complete frontend test discovery and corrects obsolete test contracts against the current implementation. Existing history, vocabulary, credential editing, draft/settings return and key-layout software paths were checked in controlled environments. These checks do not qualify new live speech, physical key presses, arbitrary external text fields or Bluetooth performance. Recognition providers, wake/end logic, streaming, clipboard behavior and firmware are unchanged in this batch. The diagnostic repair from131 is retained.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

RC1 is a prerelease. Strong background-speech separation remains incomplete and intermittent early-close reports remain under review. Optional local-model first use, physical/long-term experience and arbitrary foreground input are not newly qualified by this update.

## File identity

- Type source: `7b8edfc22428689bb4f2964cee41b6b636fbd9e3` · `build-source/type132-20261022`
- MSI: 299196416 bytes · SHA256 `2206ffc729399114058f2c2ff45c735ebaebd43fe4d4c472924e9cc1bae18b45`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

主导航由六项收为概览、历史、工具；四项辅助功能集中在工具页，原设备页入口与齿轮设置保留。已核对受影响的软件路径、精确原生导航及安装包对应关系；本轮没有修改语音、自动结束、剪贴板或蓝牙参数，不据软件检查宣称实物、强干扰或任意上屏全部通过。保留131诊断修复与原Firmware33。49美元为纯硬件，云服务准备及用量另计。
