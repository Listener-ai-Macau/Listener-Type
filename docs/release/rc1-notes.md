# Listener v1.0.6-rc1 · Type build197 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## Voice questions in this update

With Listener connected and the existing speech and answer services configured, say **“开始提问”** followed by a question. A resizable card appears at the recording capsule position and shows the question being recognized. Pausing speech can finish capture and request an answer; **Finish and ask** remains available. The card defaults to 360×240 and keeps its adjusted size when reopened. The **“开始录音”** dictation command and the computer-microphone button route remain available. Questions and answers are not automatically pasted into another app or the clipboard.

Question capture now has its own completion ownership, uses the existing enrolled identity reference independently of its command phrase, and can recover retained audio once after a transient speech-transport failure using the same configured provider. A single current local timestamp is supplied with answer requests; model-generated facts still need review. The five-language preparation hint now distinguishes components that need downloading from those already ready to use.

The guide describes both voice and button entry points. No new model, cloud account or firmware flash is required for this app update.

## Verification and remaining scope

An actual build196 Listener question produced live previews and a source-confirmed automatic stop after the user stopped speaking. Its calendar answer was wrong; the original failed answer is retained separately from the capture success. The shared prompt now supplies only one current timestamp, as requested; this does not guarantee model answer correctness. One prior computer-microphone button question also returned the correct answer with user confirmation.

The exact build197 MSI was extracted and checked against the built executable. Its controlled private native check covered card position/size, owned previews through recognition finishing, stale and late event rejection, one final question, close and resize/reopen behavior. This check used labelled synthetic display events, not another live microphone or hardware trial. Existing targeted capture, identity and recovery checks are retained at their actual source revisions.

RC1 remains a prerelease. Strong background-speech separation, some external-selection/global-trigger combinations, physical lighting observations and long-term/cross-PC behavior remain incomplete. Independent AI software feedback is not a real order or hardware qualification. Optional unprepared local-ASR and remote-marketplace execution are not claimed. Firmware37 is unchanged; earlier actual USB/Bluetooth settings, wired UI and backend OTA receipts retain their own scope. A normal UI Bluetooth OTA end-to-end qualification remains separate.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and may have usage charges; this release does not provide a ready-to-use official cloud account or allowance. Optional local translation components are prepared separately; translation drafts can lose meaning and need review. USB flashing and Bluetooth OTA use the same firmware ZIP.

## File identity

- Type source: `18de2f6ba3492a62799597f3db9a6b041e402f74` · `build-source/type197-20261026`
- MSI: 299466752 bytes · SHA256 `d12b12e482bbcd4ac0577982ff8ba2eafe27f406072d7c06f0540e310cc29912`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

新增“开始提问”口令与实时问题预览；停说后可自动收尾请求回答，也保留“结束并提问”按钮。卡片位于录音胶囊同位置，默认360×240，可缩放并保留调整后的大小。听写仍用“开始录音”；原电脑麦克风按钮入口保留，问题和答案不自动写入正文或剪贴板。

本批将Question的收尾归属与编辑器上屏等待分开，既有登记身份与新口令独立绑定；识别发生暂时传输故障时，用原配置对保留音频恢复一次。回答只附一条实际当前本地时间，不再附星期或次日推算，不保证模型事实答案正确。五语言提示区分首次准备翻译组件与直接使用已准备组件。

196真人记录已看到问题逐步出现和源端自动STOP；同次模型答错日期的原记录保留，不能拿录音成功替代语义正确。197匹配原生检查只验证标记合成显示事件、位置/尺寸、问题归属、迟到保护和关闭等范围，不冒充新真人讲话或全功能验收。强人声干扰、部分外部选区/全局触发、实体灯光和长期等范围仍未完成。固件37不变，无需为本次软件更新重刷。MSI与固件ZIP入口在上方。
