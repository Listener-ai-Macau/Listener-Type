# Listener v1.0.6-rc1 · Type build109 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

- **Make the four keys yours** on Overview opens the existing layout editor. An actual product photo marks the key you select; the visible cards show the actions in your current draft. This is configuration preview, not a live connection or physical action.
- In **Practice here**, choose single click, double click or hold, then click a software key once to try that mapping in the private field. Each press uses its own configured action. Supported templates/editing and explicit Type-page opening remain local; recording, external actions and knob motion are not simulated. No real double-click or hold is sent to the computer or device.
- Saving a layout still saves only on this computer. Loading one into Device settings prepares a draft; only explicit **Write** changes the selected device. All existing device, service and unsaved-change boundaries remain.

## Retained from build108


- When you change a trial draft, **Earlier result for comparison** keeps the previous edited result with its exact input available to read. It is clearly separate from a result for the new draft; no old Copy/Insert authority or service request is carried over. Restoring the exact input restores its matching edited result. A successful new result for that style replaces its previous snapshot; closing clears these temporary comparisons.
- Recorded-audio requests and transcripts stay in their window when the underlying Type page changes.

## Recorded audio · retained from build107

- **Overview → Transcribe recorded audio** accepts an uncompressed 16 kHz/16-bit mono WAV up to 60 seconds. Choose a file or the included synthetic English sample; selection stays local until you explicitly choose **Transcribe**. It uses your existing direct Volcengine recognition service and its normal charges. Other providers and managed accounts are not supported in this new file entry.
- Review and edit the actual returned transcript, then explicitly copy it for normal pasting. File results are not saved in History; copy before closing. Cancellation stops only that file request; it does not stop a live recording. No microphone, device command, automatic insertion or local-model download is involved. Sample reference words are labelled separately from recognition results; file-processing time is not live recording latency.
- The English typed-trial notice now uses **Original text**, matching its picker.

## Retained workspace features

- Edit the returned text before copying it. Completed results for the same input remain visibly labelled in the open window; reopen one to revise or copy without requesting it again. These temporary results are not History entries.
- Plan your own four-key-and-knob layouts from Overview. Save layouts on this computer and reload them later. Loading a layout in Device settings prepares a draft; only **Write** changes the selected device.
- Choose single, double or hold and try supported template, copy, paste and undo mappings in a local practice field. Its copy buffer is private; it does not touch the system clipboard. Recording, AI, external app launch, other shortcuts and the knob are not simulated.
- Use **Use as a key phrase…** on a reviewed result, choose its key and press type, explicitly use the phrase in that draft, then save the layout. The original result stays available. The window remembers the last layout in that session.
- Each template key previews its own saved text. **Previous enabled style** now explains its real purpose: choose the previous enabled style for later recordings, rather than rewrite selected text.
- Cards also display their actual Type page, shortcut or external-app target. Supported Type pages have a separate explicit **Open** action in practice; unsaved-change protection remains.
- **Use my saved phrases** shows the phrases actually saved in your layouts. Choose **Copy**, then paste normally into your writing. This explicit copy uses the system clipboard, separately from the practice field's private buffer; it does not automatically insert text. A failed copy keeps the phrase available to retry.
- Layout mode controls stay at the top while scrolling. Loading an unsaved draft is separate from using the saved version of a phrase.
- Trial controls remain stable through readiness checks, style changes and results. Draft editing, visible retention guidance, explicit Copy and cancellation protection remain.

Typed processing uses your configured text service and may incur its usual charges. Raw needs no text service. Saving or practising a layout does not start recording or make a service request.

[First setup](https://github.com/Listener-ai-Macau/Listener-Type#make-your-first-dictation) · [Light or Formal?](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/docs/USAGE.en.md#choose-light-or-formal-for-the-message-you-have) · [Service costs](https://github.com/Listener-ai-Macau/Listener-Type/blob/master/docs/USAGE.en.md#estimate-your-own-service-costs)

## App, hardware and current limits

**Type is free. US$49 is the Listener hardware price.** The device adds its own desk microphone, physical recording/volume control, four customizable keys and status lights. The free app can use a computer microphone and Right Ctrl; both routes use the recognizer configured on the computer. Cloud setup and usage cost extra. The current release does not include a ready-to-use Listener cloud account or bundled usage; a purchase link has not yet been announced.

This is an **RC1 prerelease for Windows**. Review final words in noise or overlapping speech. Strong-interference filtering is limited, and intermittent early-close field reports remain unresolved; use the manual finish control when needed. This build expands local key-layout previews and practice; it does not claim new wake, recording-stop or Bluetooth fixes. Firmware33 is unchanged, so a device already on that firmware needs no reflash. One synthetic English file was checked through the existing service; this does not establish general recognition quality, new live-speech, external-editor or physical qualification.

## Check which file you have

The MSI is **Type build109**, source **439f444f6afe** (also shown in the app footer/About). Firmware is **build33**, runtime **e7bd1f1619b696a1**. Filenames stay fixed within RC1; the source and SHA-256 identify the files.

| Component | Exact source | SHA-256 |
| --- | --- | --- |
| Type109 MSI · 299081728 bytes | [439f444f6afe](https://github.com/Listener-ai-Macau/Listener-Type/commit/439f444f6afe22aaf8218a1454ed09dd538e343c) | `581852420d22511c1daa7c4a283e9853a153fa24857ff15a042abd042a9e9c6e` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

109在主页提供实物照片与自己的布局入口，配置中标出所选物理位置；练习框可选择单击／双击／长按对应动作，只在私有输入框里试用，不向电脑或设备发送真实长按／双击。保存到本机和明确写入设备的边界保持。照片／软件练习不代表实体效果或真实购买已确认。

改稿后可只读对照「上一份结果」和对应原输入，不将旧结果当作新稿的输出，也不会重新请求服务。恢复完全相同的原稿时可恢复匹配修订；本风格生成新结果才替换旧快照，关闭清除。已有录音窗口由主界面持有，底层换页不会丢掉它的请求或文字。

「总览 → 转写已有录音」可选择未压缩16 kHz、16位单声道WAV（最长60秒）或内置英文合成样例，明确点击「转写」才上传到当前自行配置的火山识别服务并正常计费；暂不支持其它提供商或托管账号。返回文字可核对、修订和明确复制，不写历史，关闭前复制需要保留的内容。取消只停止这条文件请求，不中断实时录音。此入口不采音、不控制设备、不自动上屏、不下载本地模型；样例参考词和实际识别结果分开，文件耗时不代表实时延迟。

本版可原地修订文字结果，在当前窗口查看同一原文已完成的不同风格结果，打开旧结果不会再次请求服务。主页可保存自己的四键/旋钮布局，并用独立练习框试支持的短语、复制、粘贴、撤销等单击／双击／长按映射。练习复制使用私有缓冲，不操作系统剪贴板，不模拟录音、AI、外部应用、其它快捷键或旋钮。

修订后的结果可「放到按键布局」，明确选择键/按法、用于草稿后保存；原始成稿保留。短语键直接显示各自内容；「上一个已启用风格」明确是为后续录音切换风格，不改写选中文字。布局保存到本机，载入设备设置只是草稿，只有写入才改变所选设备。

「使用已保存短语」可明确复制真实保存的内容，再正常粘贴到写作位置；这项复制才使用系统剪贴板，失败保留内容可重试，不自动上屏。卡片显示实际目标，支持的Type本机页面可从练习中明确打开；未保存保护保持。长配置窗口的顶部模式入口保持可见。

Type免费，49美元是硬件价，云服务配置和用量另计。试用不启动录音、不改变当前风格、不写历史；非原文风格可能产生现有文字服务费用。Firmware33不变；本次不声称修复强干扰、偶发提前结束、唤醒或蓝牙，也不代表新的实物/真实语音/外部输入整体验收。当前Type109/439f444f6afe与Firmware33的校验值见上表。
