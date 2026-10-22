# Listener v1.0.6-rc1 · Type build126 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Style preset changes are made visible only after their metadata file has been written successfully. When creation, editing, enabling/resetting, deleting or importing a style fails, the previously saved style state remains in place. A failed deletion also retains the existing icon. You can correct the storage problem and retry without a ghost preset from the failed attempt.

This is a persistence reliability fix. Existing recording behavior, provider/model/account settings and Firmware33 are retained. The concise text option below remains optional; this update makes no additional rewriting or interference-removal claim.

## Concise text retained from build125

**Try typed text → Add concise style** saves an optional **Concise message** preset on this computer and selects it for the text workspace. It is designed to shorten wordy phrasing while preserving details, conditions and the original language. Already concise text may remain unchanged. Review the result before using it; this is text-service processing, not guaranteed semantic preservation or speech recognition.

Adding or selecting the preset does not send text. Choose **Try this text** when ready to use your configured text service; normal service charges may apply. Original text and other completed style results remain available to compare, and you can edit, copy or explicitly save the result. The saved preset can be selected again without creating a duplicate.

Text-workspace presets are saved outside the daily recording style cycle. Adding Concise message or the optional Everyday list no longer expands that cycle or rewrites recording preferences. They are available for explicit text trials; enabling/activating a style for live dictation remains a separate choice in Style. Existing saved styles, normal style-manager creation, default Original/Light prompts, provider/model/account settings and firmware are retained.

## Wording recovery retained from build124

Optional text-service suggestions now distinguish **a request that did not complete** from **a reply that could not be used safely**. A rejected suggestion does not tell you to change a working account. Your existing text remains available: continue with local wording choices, edit directly, copy or save. No automatic retry sends another paid request.

Failed suggestions stay associated with the text that was submitted. If you edit the draft while a request runs, its later failure does not appear against your newer draft. Unsafe proposals remain unavailable, and edits still require an explicit choice. Recognition, accounts, providers, models, prompts and live dictation defaults are unchanged.

## Local wording retained from build123

**Original text → Wording options** works without a text-service account. Enter your own text, choose **Try this text**, then compare local English request wording beside Copy and Save. Your original remains available; nothing changes until you choose **Use this wording**. This local path makes no cloud request. Other result styles can use these options too.

The options now find the request within a sentence instead of requiring the entire sentence to start with it. For example, “Hi, could you send the schedule by Friday?” can offer “Hi, could you please send the schedule by Friday?” or “Hi, would you be able to send the schedule by Friday?”. Greetings, addressees, dates, preceding conditions and the rest of the message stay intact. These are limited English request shortcuts, not general grammar or multilingual rewriting; review meaning and choose whether they suit you.

**Ask text service for alternatives** stays separate, with its fee notice before the action. It is disabled when no text service is configured; local choices remain available. Configuring a service is optional for this local path. Its existing endpoint/model/prompt and guarded local-span response checks are unchanged.

If a text service returns exactly your original draft, Type now explicitly labels it **unchanged** and says no reason was provided. It does not invent a judgement that your draft is already good or claim a rewrite happened. You can keep it, edit directly or compare local options.

Default Original text, selected dictation style, recognition, accounts and firmware remain unchanged. No local model or new provider is added. Original/edit/save, stale reply and cloud-proposal guards remain.

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
| Type126 MSI · 299110400 bytes | [70b162bf83ef](https://github.com/Listener-ai-Macau/Listener-Type/commit/70b162bf83ef48a2a5ef50773e9614ca28493800) | `5e418d9a0134c9107a8a08c8ed77faa29f3335774914b0629e8f0a33eaaa0492` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

126修复风格持久化失败后内存仍显示未保存修改的源头：先写元数据，成功后才更新可见状态；失败删除保留原资产。修正存储条件后可重试，不留下失败时的假保存。录音、账号/模型/提供商与Firmware33保持，本次不宣称文字质量或抗干扰提升。

125提供可选“简洁成稿”：文字试用中主动添加并选中，不发送文字；明确处理时才使用既有文本服务并正常计费。用于压缩啰嗦表达，需自行核对信息与条件；已简洁的稿可能不变。原稿、不同风格结果比较与修订复制/保存保留。

文字试用中新建的简洁/日常列表风格保存在日常循环之外，不改录音偏好；用于实时录音的启用/激活仍需在风格页另选。普通风格管理创建和已保存风格保持，默认原文/Light提示、账号/模型/提供商及Firmware33不变。

124区分“请求未完成”与“服务已返回但建议无法安全采用”，不把后者误导成需要改账号；原文、本地选项、主动复制/保存保留，不自动付费重试。编辑了新稿后，旧请求的错误不会挂在新稿上。原建议校验保持，不改云模型/提示、默认识别或固件。

123的默认原文路径可直接使用本地英文措辞，无需文本服务账号。输入文字、明确处理原文后，点击措辞选项，比较礼貌请求与更柔和问句，主动采用后才改稿，不调用云端。请求按句内片段定位，招呼、称呼、日期、前置条件与其余正文保留；这是有限英文请求便利，不是通用语法或多语言改写，采用前仍需核对含义。

云建议与费用单独呈现，未配置文本服务时禁用云按钮，本地选项仍可用。既有云后端/模型/提示和局部保护不改。服务输出与原稿完全相同时，明确标为原样返回/未提供原因，不编造原稿已足够好的解释。原文、修订保存与旧回复保护保留，默认原文和实时输入路径不变，Firmware33不变。

118补全保存错误结果追踪：即使失败先于关闭返回，也会明确提醒；同一字段后续重试成功才清除错误，旧失败不盖新成功，不把已放下的上次窗口错误带到新窗口。原稿/改稿继续保留。117提供的「转写已有录音→打开文字工作区→文本服务设置」入口保留。使用原有文本提供商配置面板，原录音稿、草稿和修订结果留在下方。点击关闭或Esc先提交当前字段，等保存结束再返回并刷新就绪状态；保存失败明确留在设置中，也可选择返回文字。不会自动发送文字、复制、改默认风格或写设备，沿用既有凭据后台和服务配置。

已有文字试用、录音转写、原地修订、明确保存成稿/重开、布局库和软件练习保留。成稿与录音历史独立，不存音频，不自动保存/删除。文件转写仍仅支持自行配置的火山服务和16kHz/16位/单声道WAV（最长60秒）；选择文件不上传，点击转写才请求服务并正常计费。原稿副本超过4,000字符不截断。

Type免费，49美元为纯硬件价，云账号与用量另计。Firmware33不变；本次不代表新的实体、实时语音、外部上屏、强干扰或蓝牙验收，亦不代表真实购买或成交。
