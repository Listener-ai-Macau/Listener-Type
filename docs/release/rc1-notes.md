# Listener v1.0.6-rc1 · Type build94 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

- Trial controls stay in place through availability checks, normal style changes and result display. A fixed dialog anchor and reserved workspace keep the submit button from moving under the pointer; font-scaled windows fit the viewport.

- Results now show **not saved in History — copy before closing** beside the Copy control; you do not need to expand the data-use details to discover this.

- Keep typing your draft while trial availability is being checked, is unavailable or needs a retry. Only submission waits for readiness; a readiness response does not replace your text.

- See the selected saved style’s purpose before submitting your draft.
- Add an optional **Everyday list** for messages and notes from the trial’s Clear structure choice or the Style page. It preserves questions and conditions while making separate points easier to scan.
- The optional style is saved separately. Adding or choosing it in a trial does not activate it for recording or send your text.

- **Overview → Try typed text** now opens the draft directly. Choose a trial style in the same window; the active dictation style stays unchanged.
- Compare draft and result side by side on desktop. Cost and text-only scope stay visible in a short notice; expand the details when needed.
- **Copy result** copies the returned text. Wait for **Copied**, then paste into your message. If copying fails, the result stays available to retry.
- A typed trial does not start recording, change your active style or create a History entry. Text processing uses your configured text service and may incur its usual charges. Default Raw needs no text service.

[First setup](https://github.com/Listener-ai-Macau/Listener-Type#make-your-first-dictation) · [Light or Formal?](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/docs/USAGE.en.md#choose-light-or-formal-for-the-message-you-have) · [Service costs and actual usage](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/docs/USAGE.en.md#estimate-your-own-service-costs)

## App, hardware and current limits

**Type is free. US$49 is the Listener hardware price.** The device adds its own desk microphone, physical recording/volume control, four customizable keys and status lights. The free app can use a computer microphone and Right Ctrl; both routes use the recognizer configured on the computer. Cloud setup and usage cost extra. The current release does not include a ready-to-use Listener cloud account or bundled usage; a purchase link has not yet been announced.

This is an **RC1 prerelease for Windows**. Review the final words in noise or overlapping speech. Strong-interference filtering is limited, and intermittent early-close field reports remain unresolved; use the manual finish control when needed. This build stabilizes the typed-trial workspace; it does not claim new wake, recording-stop or Bluetooth fixes. Firmware33 is unchanged, so this app update does not require reflashing a device already on that firmware.

## Check which file you have

The MSI is **Type build94**, source **d19ceefcf1c5** (also shown in the app footer/About). The firmware is **build33**, runtime **e7bd1f1619b696a1**. Compare the downloaded file's SHA-256 below. The download filenames stay fixed within this RC1; the build/source and checksum identify the files.

| Component | Exact source | SHA-256 |
| --- | --- | --- |
| Type94 MSI · 298500096 bytes | [d19ceefcf1c5](https://github.com/Listener-ai-Macau/Listener-Type/commit/d19ceefcf1c5ad2559f0eb079c0cb3b1341424d3) | `e3ac2d6ab245c205ac5d7ec14d49ba4349042826f6546d3171d552ac78713e6a` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

本版稳定试用弹窗及输入/结果区域，避免检查、切换风格或出结果时跳位，并修正字体放大后的视口尺寸。保留试用结果旁直接显示「不写历史、需要时关闭前复制」，无需展开说明。保留可用性检查期间编辑草稿：检查中、失败或重试仍可编辑，只有提交等待检查完成。保留独立可选「日常清单」，适合消息和笔记；试用选择旁显示保存用途。添加或试选不会切换录音默认，也不会发送文字。主页直接打开「文字试用」草稿窗，可在窗内选风格、桌面并排核对结果，并显式「复制结果」；等「已复制」后再粘贴。试用不启动录音、不切换当前风格、不写历史；非原文/自定义风格可能产生现有文字服务费用。

Type 免费，49美元为硬件价，云服务配置与用量另计。硬件增加桌面麦克风、实体录音/音量旋钮、四个自定义按键与状态灯；现有麦克风和快捷键顺手时，可继续用免费软件。当前为 Windows RC1 候选，强干扰与偶发提前结束仍有现场限制；本次不声称修复这些问题，原Firmware33不变。当前文件为Type94/d19ceefcf1c5和Firmware33，文件校验值见上表。
