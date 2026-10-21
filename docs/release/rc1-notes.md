# Listener v1.0.6-rc1 · Type build120 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

**Formal result → Suggest more courteous requests** is optional and uses your existing configured text service. Opening a result does not request suggestions. Clicking sends the current result and can incur normal service charges.

Read the proposed original/replacement pairs, select the changes you want, then choose **Use selected edits**. Nothing is preselected or automatically applied. Keep your wording instead if preferred. Your original draft stays beside the result, and accepted edits can be reviewed, copied or saved using the existing actions.

Suggestions must match unique, non-overlapping local source spans. Checks screen changes to recognised numbers, dates, firm requirements, negations and protected code/URL anchors. These are limited checks, not a semantic or all-language guarantee: review meaning and names before accepting. If you edit while a request is running, its earlier suggestions cannot overwrite the new text. Failed or unusable replies leave your current text intact.

This does not replace default Formal wording, change the selected dictation style, add a provider/model or run a local model. Previous save/edit protections remain.

## Settings reliability retained

A text-service settings save failure now remains visible when you close, even if the save already finished. A newer successful retry for that field clears the failure; unrelated writes and obsolete failures cannot misreport it. Returning explicitly without saving keeps your text. An earlier dismissed visit does not block a later visit without edits.

## Recorded-text workspace retained

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
| Type120 MSI · 299110400 bytes | [6311902575d6](https://github.com/Listener-ai-Macau/Listener-Type/commit/6311902575d6121826fd9980fd779a02992c0687) | `98d0ecf78c4bc45c32452d364d928c8923eaee2b9d1a8607630df3136202e397` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

120针对需要更礼貌请求的成稿，增加明确请求既有服务→比较局部原句/建议→选中后采用的可选动作。打开结果不调用，点击获取可能按原服务正常计费；建议默认不选、不自动修改。原文保留，接受后的稿仍用原复制/保存入口。数字、日期、强制/否定词及URL/代码锚点检查只是有限保护，不保证全部语义或语言，采用前应核对含义和名字；旧稿回复和失败不覆盖新稿。不替换默认正式表达，不改实时输入/账号/提供商/模型。


118补全保存错误结果追踪：即使失败先于关闭返回，也会明确提醒；同一字段后续重试成功才清除错误，旧失败不盖新成功，不把已放下的上次窗口错误带到新窗口。原稿/改稿继续保留。117提供的「转写已有录音→打开文字工作区→文本服务设置」入口保留。使用原有文本提供商配置面板，原录音稿、草稿和修订结果留在下方。点击关闭或Esc先提交当前字段，等保存结束再返回并刷新就绪状态；保存失败明确留在设置中，也可选择返回文字。不会自动发送文字、复制、改默认风格或写设备，沿用既有凭据后台和服务配置。

已有文字试用、录音转写、原地修订、明确保存成稿/重开、布局库和软件练习保留。成稿与录音历史独立，不存音频，不自动保存/删除。文件转写仍仅支持自行配置的火山服务和16kHz/16位/单声道WAV（最长60秒）；选择文件不上传，点击转写才请求服务并正常计费。原稿副本超过4,000字符不截断。

Type免费，49美元为纯硬件价，云账号与用量另计。Firmware33不变；本次不代表新的实体、实时语音、外部上屏、强干扰或蓝牙验收，亦不代表真实购买或成交。
