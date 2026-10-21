# Listener v1.0.6-rc1 · Type build116 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing / Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

- Fix a credential-field timing defect: an earlier save acknowledgement could clear the dirty flag for a newer edit. Closing settings before that edit's autosave could then discard it. Acknowledgements now belong to their exact edit, and blur submits the latest unsaved value.
- Old failures cannot replace newer success. Save feedback can finish after the field closes, while a late save cannot replace feedback from a newer copy operation.
- Credential storage scope, API addresses, providers and model defaults remain unchanged. This is a reproduced settings UI defect; it has not been established as the cause of earlier recognition-quota reports.

## Retained from build115

- After a recorded transcript returns, choose **Open in text workspace** to bring a copy of its current wording into the existing editable style workspace. This removes manual copying between these two entries. The recording transcript stays in its own window and is not changed by edits to the copy.
- Choose a style and explicitly submit when ready. Opening makes no text-service request; processing uses your existing configured service and its usual charges. You can review, edit, Copy or Save draft there, then return to the transcript. A saved styled draft preserves its text-workspace input and edited result.
- The existing 4,000-character text limit is checked before opening; longer edited transcripts stay intact and are not silently truncated. An earlier separate typed draft is not overwritten. This is recorded-file processing, not live hardware or Bluetooth qualification.

## Retained from build114

- After **Save draft** actually succeeds, the retention note confirms that this version is saved separately from recording History. It no longer asks you to save the version you just saved.
- Editing the result again restores the copy/save reminder; only the exact acknowledged version is called saved. Failure and pending states retain their existing protections. Typed and recorded-audio results follow the same rule.
- Existing local storage, speech/text services, copying, recording and device settings are unchanged. This fixes contradictory guidance, not a data-loss defect or a new hardware capability.

## Retained from build113

- After a recorded-audio transcript returns, choose **Save draft** to keep the actual recognized original and your current edited transcript in the existing local text library. Open **Overview → Saved drafts** to read, copy or delete it later. This stores no audio, makes no additional service request and does not create a recording History entry.
- Saving holds editing, copying, discarding and closing until acknowledgement; a failure keeps the transcript available. A saved current version can close without a false warning. Later edits and new results do not inherit that saved acknowledgement.
- File recognition still requires your configured direct Volcengine service. Selecting a file stays local until explicit **Transcribe**; live recording and file cancellation remain separate.

## Retained from build112

- Read and edit the result first. **Copy result** and **Save draft** are the main actions below it; the original stays beside it on a wide window. The retention notice remains visible.
- **More ways to use this text** opens the existing key-phrase and explicit external-insertion actions. Insertion cancellation remains available even if this section is folded. No copying, saving, insertion or device write happens automatically.
- The unchanged factory technical style is displayed as **Technical instructions**, matching its existing AI-coding/troubleshooting purpose. Your renamed or imported styles, saved prompts and examples remain untouched.

## Retained from build111

- The layout name stays in place when saved layouts finish loading. Text entered while loading remains your draft.
- Select a key from the row above its action editor. The actual product photo shows that key beside the editor, rather than pushing the form below it. Narrow windows put editing first and retain the save controls.
- Existing separate single/double/hold mappings, unsaved-change protection, explicit local saves and clipboard-copy confirmation remain. This update does not write device settings automatically.

## Retained from build110

- Choose **Save draft** beside your current text result to keep both its original input and your edited wording in a separate local draft library. This is explicit saving, not automatic retention or a recording History entry. No audio is stored and no service request is made.
- Open **Overview → Saved drafts** to find a saved version, review its original, explicitly copy the saved text for normal pasting, or delete it after confirmation. Local text remains on this computer until deleted here; the library holds up to 50 drafts within 4 MiB, without silently deleting older entries. Browser previews retain drafts only for the current page session.
- Save failures keep your current text available. Retrying the same save does not duplicate an already committed draft. The current result and its source stay fixed while saving. Corrupt or full libraries are preserved rather than overwritten.

## Retained from build109


- **Make the four keys yours** on Overview opens the existing layout editor. An actual product photo marks the key you select; the visible cards show the actions in your current draft. This is configuration preview, not a live connection or physical action.
- In **Practice here**, choose single click, double click or hold, then click a software key once to try that mapping in the private field. Each press uses its own configured action. Supported templates/editing and explicit Type-page opening remain local; recording, external actions and knob motion are not simulated. No real double-click or hold is sent to the computer or device.
- Saving a layout still saves only on this computer. Loading one into Device settings prepares a draft; only explicit **Write** changes the selected device. All existing device, service and unsaved-change boundaries remain.

## Retained from build108


- When you change a trial draft, **Earlier result for comparison** keeps the previous edited result with its exact input available to read. It is clearly separate from a result for the new draft; no old Copy/Insert authority or service request is carried over. Restoring the exact input restores its matching edited result. A successful new result for that style replaces its previous snapshot; closing clears these temporary comparisons.
- Recorded-audio requests and transcripts stay in their window when the underlying Type page changes.

## Recorded audio · retained from build107

- **Overview → Transcribe recorded audio** accepts an uncompressed 16 kHz/16-bit mono WAV up to 60 seconds. Choose a file or the included synthetic English sample; selection stays local until you explicitly choose **Transcribe**. It uses your existing direct Volcengine recognition service and its normal charges. Other providers and managed accounts are not supported in this new file entry.
- Review and edit the actual returned transcript, then explicitly copy it for normal pasting. File results are separate from recording History; explicitly Copy or Save draft before closing. Cancellation stops only that file request; it does not stop a live recording. No microphone, device command, automatic insertion or local-model download is involved. Sample reference words are labelled separately from recognition results; file-processing time is not live recording latency.
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

This is an **RC1 prerelease for Windows**. Review final words in noise or overlapping speech. Strong-interference filtering is limited, and intermittent early-close field reports remain unresolved; use the manual finish control when needed. This build extends explicit local draft retention to recorded transcripts; it does not claim new wake, recording-stop or Bluetooth fixes. Firmware33 is unchanged, so a device already on that firmware needs no reflash. One synthetic English file was checked through the existing service; this does not establish general recognition quality, new live-speech, external-editor or physical qualification.

## Check which file you have

The MSI is **Type build116**, source **a24621bc5132** (also shown in the app footer/About). Firmware is **build33**, runtime **e7bd1f1619b696a1**. Filenames stay fixed within RC1; the source and SHA-256 identify the files.

| Component | Exact source | SHA-256 |
| --- | --- | --- |
| Type116 MSI · 299106304 bytes | [a24621bc5132](https://github.com/Listener-ai-Macau/Listener-Type/commit/a24621bc5132fb9c87f0e6233bf41dc762ca0d71) | `853baac22a4fb531c07f2facccabfb45d42ed32f0867cba13de376aea861425f` |
| Firmware33 ZIP · 1956905 bytes | [149b6d950ebc](https://github.com/Listener-ai-Macau/Listener-Firmware/commit/149b6d950ebc5f416924c8670cf2b207bdc2b5b7) | `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272` |

## 中文说明

116修复凭据字段旧回执误清后续修改，避免新值在自动保存前关设置时被丢弃；旧失败不覆新成功、旧保存提示不盖新复制反馈。沿用原有后台存储、scope/API/提供商/模型。此UI缺陷与历史额度错误因果未证，不宣称已根除所有识别错误。

115把录音转写与现有文字工作区接起来：明确打开当前稿的副本，选择样式并提交后才请求既有文字服务；可编辑、复制或保存结果，再回到保留的转写窗口。没有自动发送、复制、激活风格或覆盖其它键入草稿；超过原4,000字符限制不截断。仍是文件转写，不是实时硬件或蓝牙验收。

114修正保存后的矛盾提示：仅当前版本确实收到成功回执才显示已保存，后续修改重新提示复制或保存，失败或未完成不会误报成功。文字和录音转写结果一致；没有改变存储、识别、剪贴板或固件。

113将已有录音的真实原转写和当前修订稿接入同一个明确「保存成稿」入口；可关窗后从总览重开、复制、确认删除。只保留本机文字，不存音频、不写录音历史、不自动保存、不新增云请求。保存失败留稿，保存中锁定冲突操作，后续修改或新结果不冒用旧保存回执。

112先展示结果正文及实际来源，正文下明确复制/保存；按键短语与外部插入可从「更多文字用途」展开，插入中的取消始终可用。默认技术风格显示为「技术指令」，保留旧工厂持久化名字、用户改名/导入内容、提示词及示例。没有改识别、默认输出或自动保存行为。

111固定载入前后的布局名称框位置，保留载入时已输入草稿；选键横排、编辑与实物照片并列，窄窗先显示编辑，减少来回滚动。原单双长按独立映射、未保存保护、显式保存和实际复制确认保持。本次不自动写设备。

110可在当前文字结果旁明确「保存成稿」，保留确切原文及当前修订稿到本机文本文件；从「总览 → 已保存成稿」重开、复制、确认删除。与录音历史／时长／音频独立，不自动保存、不请求服务、不自动上屏。最多50份且文件上限4MiB，不自动清旧稿；保存失败保留当前文字，同次重试不重复建档，坏库或满库不覆盖。浏览器预览只保留在当前页面会话，安装版才写本机。

109在主页提供实物照片与自己的布局入口，配置中标出所选物理位置；练习框可选择单击／双击／长按对应动作，只在私有输入框里试用，不向电脑或设备发送真实长按／双击。保存到本机和明确写入设备的边界保持。照片／软件练习不代表实体效果或真实购买已确认。

改稿后可只读对照「上一份结果」和对应原输入，不将旧结果当作新稿的输出，也不会重新请求服务。恢复完全相同的原稿时可恢复匹配修订；本风格生成新结果才替换旧快照，关闭清除。已有录音窗口由主界面持有，底层换页不会丢掉它的请求或文字。

「总览 → 转写已有录音」可选择未压缩16 kHz、16位单声道WAV（最长60秒）或内置英文合成样例，明确点击「转写」才上传到当前自行配置的火山识别服务并正常计费；暂不支持其它提供商或托管账号。返回文字可核对、修订、明确复制或保存成稿，与录音历史独立；关闭前复制或保存需要保留的内容。取消只停止这条文件请求，不中断实时录音。此入口不采音、不控制设备、不自动上屏、不下载本地模型；样例参考词和实际识别结果分开，文件耗时不代表实时延迟。

本版可原地修订文字结果，在当前窗口查看同一原文已完成的不同风格结果，打开旧结果不会再次请求服务。主页可保存自己的四键/旋钮布局，并用独立练习框试支持的短语、复制、粘贴、撤销等单击／双击／长按映射。练习复制使用私有缓冲，不操作系统剪贴板，不模拟录音、AI、外部应用、其它快捷键或旋钮。

修订后的结果可「放到按键布局」，明确选择键/按法、用于草稿后保存；原始成稿保留。短语键直接显示各自内容；「上一个已启用风格」明确是为后续录音切换风格，不改写选中文字。布局保存到本机，载入设备设置只是草稿，只有写入才改变所选设备。

「使用已保存短语」可明确复制真实保存的内容，再正常粘贴到写作位置；这项复制才使用系统剪贴板，失败保留内容可重试，不自动上屏。卡片显示实际目标，支持的Type本机页面可从练习中明确打开；未保存保护保持。长配置窗口的顶部模式入口保持可见。

Type免费，49美元是硬件价，云服务配置和用量另计。试用不启动录音、不改变当前风格、不写历史；非原文风格可能产生现有文字服务费用。Firmware33不变；本次不声称修复强干扰、偶发提前结束、唤醒或蓝牙，也不代表新的实物/真实语音/外部输入整体验收。当前Type116/a24621bc5132与Firmware33的校验值见上表。
