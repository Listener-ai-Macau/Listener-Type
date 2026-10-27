# Type build213 / Firmware38 · Listener v1.0.6-rc1

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## Keep the deadline attached to its time

Offline translation previously treated the dot in an ordinary abbreviation as a sentence ending. A retained English deadline containing **3 p.m. tomorrow** was split before “tomorrow”, producing a separate suggestion to talk tomorrow instead of keeping the deadline together. Common title/time continuations now stay in one model input. Genuine next sentences, including a separate no-send instruction or “May I…” question after a time, stay separate.

This changes input sentence boundaries, without rewriting the source or replacing words in the output. Existing Argos1.9 weights, embedded runtime, paragraph boundaries, bounded long-input handling and local/cloud selection remain. Existing prepared components do not need a new model download.

## Leave the text workspace without scrolling back up

The workspace title and **Close** stay visible while reading or saving a long result. Returning from saved drafts keeps the edited output and name; the Close action remains reachable after scrolling. Existing save and close safeguards are retained.

Fourteen retained/new actual-worker cases and source-boundary controls passed. The matching ordinary213 native app used only prepared local components with empty service credentials and confirmed the deadline/title cases, exact local save/reopen/return, and scrolled Close. These are bounded engineering checks. The independent assumed-owned211 text task remains the latest natural customer experience; it is not213 customer or physical acceptance.

Retains211 saved-draft navigation,210 direct drafting/original-source cloud retranslation and208 question-answer preview/readiness/save changes. Firmware38 is unchanged; no firmware flash is required for this Type update.

## Existing limits

This does **not** fix general NMT semantic accuracy. Local translation can still alter important actions, dates, names or negation relationships; local and cloud outputs need review. Strong background-speech isolation, some early-ending conditions, actual global/physical/cross-PC/long-term qualification and the original brownout trigger remain open. RC1 remains a prerelease.

US$49 is the hardware price. Type is free/open source. Service preparation and usage charges are separate; no official cloud account or allowance is included.

## File identity

- Type source: `d965daf77a96893c265d550ba1847d81dd002416` · `build-source/type213-20261028`
- MSI: 299794432 bytes · SHA256 `1b78150edf3854c2c14a574acc1c446d499e90ddf0d3327b3696e8129965821c`
- Native EXE SHA256: `f73df259db5a3824df356dc156e1be754d63d9b8bed12ca0d1701255acc87c2e`
- Native DLL SHA256: `5164b2d837bb0cbfe34c5f5085bb29d0baa9f1659673a22cfa5978ddb8c05b1b`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

修复本机翻译把称呼或时间缩写中的点误作句末的问题：原有英文稿的「3 p.m. tomorrow」曾被拆成两个模型输入，导致截止时间变成「下午三点前……明天再说吧」，现在保住「明天下午三点前」的关系；真正的后续句子仍分别处理。源文字不被改写，原模型、运行组件、段落、长文本和否定／仅存草稿保护保留。工作区标题和关闭按钮随滚动保持可见，查看成稿返回仍保留正文与名称。

精确213普通原生包已用空服务凭据和既有本机组件验证截止时间、称呼／星期时间、保存／回找／返回与滚动关闭；不是213自然顾客、真人语音或实物通过。211假设已购顾客完整文字任务及原售前反馈保留。通用译义、强背景干扰、真实全局／实物／跨机长期和欠压触发源仍未完成；固件38不变。
