# Listener v1.0.6-rc1 · Type build121 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

**Formal result → Wording options** sits beside **Copy result** and **Save draft**. Opening it makes no service request. For supported simple English requests, Type immediately offers a local courteous question, such as “Please send me the draft by Thursday.” → “Could you please send me the draft by Thursday?” Your request body and the rest of the message stay unchanged. Choose **Use this wording** explicitly, then review, copy or save. This is a narrow English convenience, not a general grammar or multilingual rewriting tool; unsupported text stays editable.

**Ask text service for alternatives** is separate and optional. Clicking sends the current result to your already configured text service and can incur normal charges. Read each original/replacement pair, select wanted edits and choose **Use selected edits**, or keep your wording. Nothing is preselected. The optional service prompt now permits ordinary request grammar to become a courteous question, instead of merely stacking polite words.

Untrusted service proposals still require exact unique, non-overlapping local spans. Checks screen recognised numbers, dates, firm requirements, negations and protected code/URL anchors. These checks are limited, not a semantic or all-language guarantee: review meaning and names. Earlier replies cannot overwrite newer wording; failed replies keep your text. The recomputed local option does not relax these service-proposal checks.

Default Formal, selected dictation style, recognition, accounts, providers/models and firmware remain unchanged. No local model is added. Existing original/edit/save protections remain.

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
| Type121 MSI · 299106304 bytes | [1474b5bd8b5c](https://github.com/Listener-ai-Macau/Listener-Type/commit/1474b5bd8b5c9699b7f7fc9c2dc48e8772132e35) | `3a1a3c513a9535d83c157209fa0783d8cea23a407652e3e76c2b757347e51cba` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

121把「措辞选项」放到复制、保存旁。打开不请求服务；支持的简单英文请求在本机即时提供礼貌问句，主体与其余正文保持原样，核对后明确采用，不额外收费。这是有限的英文便利功能，不是通用语法/多语言改写；不支持的正文仍可手动编辑。

「向文本服务获取其它建议」单独选择、按原服务正常计费。可选云提示允许把普通请求改成自然礼貌问句，避免只叠加Please/kindly；原有强制要求、否定、日期/数字与精确片段检查保留。有限保护仍需核对名字与含义，失败/过时回复不覆盖正文。默认正式表达、实时输入、账号/模型/提供商不改，无新增本地模型。

118补全保存错误结果追踪：即使失败先于关闭返回，也会明确提醒；同一字段后续重试成功才清除错误，旧失败不盖新成功，不把已放下的上次窗口错误带到新窗口。原稿/改稿继续保留。117提供的「转写已有录音→打开文字工作区→文本服务设置」入口保留。使用原有文本提供商配置面板，原录音稿、草稿和修订结果留在下方。点击关闭或Esc先提交当前字段，等保存结束再返回并刷新就绪状态；保存失败明确留在设置中，也可选择返回文字。不会自动发送文字、复制、改默认风格或写设备，沿用既有凭据后台和服务配置。

已有文字试用、录音转写、原地修订、明确保存成稿/重开、布局库和软件练习保留。成稿与录音历史独立，不存音频，不自动保存/删除。文件转写仍仅支持自行配置的火山服务和16kHz/16位/单声道WAV（最长60秒）；选择文件不上传，点击转写才请求服务并正常计费。原稿副本超过4,000字符不截断。

Type免费，49美元为纯硬件价，云账号与用量另计。Firmware33不变；本次不代表新的实体、实时语音、外部上屏、强干扰或蓝牙验收，亦不代表真实购买或成交。
