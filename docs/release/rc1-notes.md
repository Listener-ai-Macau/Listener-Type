# Listener v1.0.6-rc1 · Type build90 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

- **Overview → Try typed text** now opens the draft directly. Choose a trial style in the same window; the active dictation style stays unchanged.
- Compare draft and result side by side on desktop. Cost and text-only scope stay visible in a short notice; expand the details when needed.
- **Copy result** copies the returned text. Wait for **Copied**, then paste into your message. If copying fails, the result stays available to retry.
- A typed trial does not start recording, change your active style or create a History entry. Text processing uses your configured text service and may incur its usual charges. Default Raw needs no text service.

[First setup](https://github.com/Listener-ai-Macau/Listener-Type#make-your-first-dictation) · [Light or Formal?](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/docs/USAGE.en.md#choose-light-or-formal-for-the-message-you-have) · [Service costs and actual usage](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/docs/USAGE.en.md#estimate-your-own-service-costs)

## App, hardware and current limits

**Type is free. US$49 is the Listener hardware price.** The device adds its own desk microphone, physical recording/volume control, four customizable keys and status lights. The free app can use a computer microphone and Right Ctrl; both routes use the recognizer configured on the computer. Cloud setup and usage cost extra. The current release does not include a ready-to-use Listener cloud account or bundled usage; a purchase link has not yet been announced.

This is an **RC1 prerelease for Windows**. Review the final words in noise or overlapping speech. Strong-interference filtering is limited, and intermittent early-close field reports remain unresolved; use the manual finish control when needed. This build improves the text-trial workspace; it does not claim new wake, recording-stop or Bluetooth fixes. Firmware33 is unchanged, so this app update does not require reflashing a device already on that firmware.

## Check which file you have

The MSI is **Type build90**, source **eaf0de83582c** (also shown in the app footer/About). The firmware is **build33**, runtime **e7bd1f1619b696a1**. Compare the downloaded file's SHA-256 below. The download filenames stay fixed within this RC1; the build/source and checksum identify the files.

| Component | Exact source | SHA-256 |
| --- | --- | --- |
| Type90 MSI · 298631168 bytes | [eaf0de83582c](https://github.com/Listener-ai-Macau/Listener-Type/commit/eaf0de83582cefc04e4d208c08a069240ac0807b) | `99b7e46e2be3b6d75d1f477dc7e3b6a6f5bf1d943c35e8a9e01f1dbf969162cc` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

本版让主页直接打开「文字试用」草稿窗，可在窗内选风格、桌面并排核对结果，并显式「复制结果」；等「已复制」后再粘贴。试用不启动录音、不切换当前风格、不写历史；非原文/自定义风格可能产生现有文字服务费用。

Type 免费，49美元为硬件价，云服务配置与用量另计。硬件增加桌面麦克风、实体录音/音量旋钮、四个自定义按键与状态灯；现有麦克风和快捷键顺手时，可继续用免费软件。当前为 Windows RC1 候选，强干扰与偶发提前结束仍有现场限制；本次不声称修复这些问题，原Firmware33不变。当前文件为Type90/eaf0de83582c和Firmware33，文件校验值见上表。
