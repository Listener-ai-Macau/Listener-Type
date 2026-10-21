# Listener v1.0.6-rc1 · Type build117 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

From **Transcribe recorded audio → Open in text workspace**, **Text-service settings** now opens the existing text-provider controls without discarding your transcript, working draft or edited result. Close or Escape flushes the focused credential edit and waits for existing writes before returning and refreshing availability. If that save fails, the settings stay open with an explicit choice to return to your text. Opening or closing settings never sends your text for processing.

The same existing credential backend and provider controls are used. This update does not change service defaults, accounts, API addresses, recognition, live recording or firmware. The previous protection against an older save acknowledgement discarding a newer edit remains.

## Available software paths

- **Overview → Try typed text**: choose a style, explicitly process with your configured text service, compare, review and edit the result. Original text needs no text service.
- **Transcribe recorded audio**: direct Volcengine speech recognition for uncompressed 16 kHz, 16-bit mono WAV, up to 60 seconds. An included synthetic sample is labelled. Select locally, then explicitly Transcribe to upload. Bring a copy of the transcript into the text workspace if wanted; the source remains separate. The existing text limit is 4,000 characters, without silent truncation.
- **Save draft → Overview → Saved drafts**: explicitly retain original and edited text locally, reopen, copy or confirm deletion. Separate from recording History; no audio is stored. Up to 50 drafts and 4 MiB, without automatic deletion. Browser previews retain drafts only for that page session.
- Save your own four-key/knob layout locally, inspect key positions on the product photo and try supported actions in the independent software practice field. Writing to a selected real device remains explicit. Software practice is not a hardware demonstration.

Type is free; US$49 is the hardware price. Cloud service setup and usage are separate. Real microphone input, hardware, Bluetooth, external application insertion, strong interference and long-term stability are not newly qualified by this software update. Firmware33 is unchanged. RC1 remains a prerelease.

| Component | Exact source | SHA-256 |
| --- | --- | --- |
| Type117 MSI · 299094016 bytes | [0d47630aefa5](https://github.com/Listener-ai-Macau/Listener-Type/commit/0d47630aefa59e6fa2e12a07a21b54cc93089679) | `5e09d4498cad4b5027086552907d1295e66d79cfd81d8e18a72f6010baef95bd` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

117补通「转写已有录音→打开文字工作区→文本服务设置」。使用原有文本提供商配置面板，原录音稿、草稿和修订结果留在下方。点击关闭或Esc先提交当前字段，等保存结束再返回并刷新就绪状态；保存失败明确留在设置中，也可选择返回文字。不会自动发送文字、复制、改默认风格或写设备，沿用既有凭据后台和服务配置。

已有文字试用、录音转写、原地修订、明确保存成稿/重开、布局库和软件练习保留。成稿与录音历史独立，不存音频，不自动保存/删除。文件转写仍仅支持自行配置的火山服务和16kHz/16位/单声道WAV（最长60秒）；选择文件不上传，点击转写才请求服务并正常计费。原稿副本超过4,000字符不截断。

Type免费，49美元为纯硬件价，云账号与用量另计。Firmware33不变；本次不代表新的实体、实时语音、外部上屏、强干扰或蓝牙验收，亦不代表真实购买或成交。
