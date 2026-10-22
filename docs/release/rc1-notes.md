# Listener v1.0.6-rc1 · Type build130 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

If you select a recording before configuring recognition, **Speech settings** now opens over that recording. Returning keeps the same selected file. It waits for settings writes to finish and checks readiness again; an unsaved setting remains explicit. Nothing is uploaded until you press **Transcribe**.

Speech and text setup share the existing save/failure protection. The text workspace still retains drafts, reviewed results and pending replies. **Saved drafts** now explains that it includes recorded-audio transcripts as well as text-workspace results.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and have separate usage charges. This release does not include a ready-to-use official cloud account or allowance. File transcription currently accepts short16kHz/16-bit mono WAV through direct Volcengine. Review important words; optional rewriting can require manual edits.

RC1 is a prerelease. Strong background speech separation remains incomplete and intermittent early-close cases remain under review. This UI update does not qualify new live-wake, automatic-ending, global-input, BLE throughput or physical-hardware behavior. Daily recording and Firmware33 are retained. This publication does not install over the daily Type instance. USB flashing and Bluetooth OTA use the same ZIP.

## File identity

- Type source: `cd327e23827289503cbd9af9fba4035608816ab6` · `build-source/type130-20261022`
- Type130 MSI: 299175936 bytes · SHA256 `e0b7ddc8bdebe21485eb83f866155d70717ff825065fe003945a2630d8828e83`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- Firmware ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

130修复选好录音后进入语音设置会丢选择的问题。原录音保留，设置写入完成才返回重查；保存失败仍明确提示，不自动上传。五语言保存列表也说明包括录音转写。文字保稿和已验路径保留。49美元为纯硬件，云服务准备与用量另计；固件、日常录音与蓝牙未改，原强干扰/偶发提前结束及改写质量限制保留。
