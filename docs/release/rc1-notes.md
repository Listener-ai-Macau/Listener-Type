# Listener v1.0.6-rc1 · Type build267 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI · build267](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| Firmware · USB flashing or Bluetooth OTA | [Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip) |

## What's changed since build266

- **Preserve the final streaming paste payload during handoff.** If the same session's last paste remains unconfirmed and contains a different payload from the complete final, Type keeps that fragment until a real receipt arrives or the existing five-second observation window from paste dispatch ends. An already-confirmed paste or identical payload still permits immediate final copying.
- **Keep final copying separate from insertion.** The complete final is copied in the existing background clipboard worker; this operation does not send another paste or hold the completion UI open. Completion-first copying remains enabled by default, with saved choices respected.
- **Protect newer clipboard ownership.** A new user copy, including a new clipboard revision with the same text, or a superseding session prevents the waiting job from replacing newer content. An expired window is still recorded as unconfirmed, never as proof that the target editor consumed the paste.

## Included from builds265–266

- **Keep ready final clauses on the ordinary streaming path.** Ready text receives its normal incremental delivery pass before the automatic-stop decision. Recognition callbacks publish observations; one endpoint loop owns that decision, with a fresh session check after asynchronous delivery.
- **Confirm pending fragments without ending capture.** The Windows owner-activity ordinary-recognition path can now confirm a stable pending fragment after an already-submitted source boundary. Confirmed new words use the same incremental ledger. Stale, cancelled or mismatched results cannot authorize a new insertion. This reuses the configured recognition service and may incur additional provider usage; clip length and total replay remain bounded.
- **Let covered quiet tails finalize independently of punctuation.** Once existing audio-coverage, continuous-quiet and delivery safeguards are satisfied, an open ending or pending recognition can enter normal finalization. Pending words remain available to final recognition and delivery. New speech, in-flight delivery and source/identity protections remain in place.

## Included from build264

- **Less repeated background computation.** Retired wake-recovery jobs check cancellation before heavy inference. Model worker threads park between runs. Personal activity and identity checks coalesce adjacent updates while retaining every source chunk and draining the final tail.
- **Release temporary memory after dictation analysis.** Once the final personal-activity worker exits, Type reclaims unused working buffers while keeping its model loaded for reuse. Cleanup does not publish recognition, speaker or stop evidence.
- **Clear completion feedback.** Successful dictation uses the short “Completed” status. Routine completion and other nonblocking status messages no longer look like errors; actual failure states retain their handling. History and numeric diagnostics retain the underlying delivery facts.
- **Better evidence for intermittent problems.** Bounded timing, signal-health and resource counters help distinguish early ending and delayed delivery. Raw audio recording remains off by default.

Controlled local tests of the real detection models reduced CPU time for an eight-second synthetic input by about 42%; this is not a fixed whole-app reduction. The production memory/reuse test released about 63 MiB, and a separate maximum-window-first allocation layout released about 139 MiB. Cleanup took about 12 ms in the production test, and five source-window lengths produced identical scores before and after cleanup in the same loaded model. Memory savings vary with allocation history. Background wake checks still have CPU and temporary-memory peaks; long-term whole-app RAM reduction has not been established across all workloads.

## Dictation and clipboard

Windows retains incremental on-screen insertion. After finalization, Type copies the complete finalized text from the same session's history to the clipboard. This final copy does not send another paste. Completion-first copying is on by default on Windows; an existing saved choice is respected.

In the unconfirmed conflicting-payload case, the clipboard may update later, at most until five seconds after the last paste was dispatched. Under the existing completion-first preference, Type then copies the full final even without a receipt. A target that still has not read the fragment can therefore still read the complete text instead; the timeout does not prove consumption or eliminate every slow-target duplicate. Review the inserted result. A newer user clipboard copy is protected. Windows clipboard history is not required for ordinary copying and pasting. The experimental clipboard-free streaming route remains disabled.

Build258's background wake-recovery receiver fix, guarded final-sentence drain and stale wake-control protections are included. Source/identity guards, live audio delivery and the configured stop wait remain in place.

## Current limits and setup

**This remains a prerelease.** Builds265–267 fix specific pending-tail, delivery-order, quiet-finalization and premature final-clipboard handoff paths. They do not establish that every intermittent ending or tail-delivery case is resolved. Ongoing speech can still end prematurely; capture-continuation confirmation failures, acoustic wake misses, other target applications and long-term behavior remain under observation.

Intermittent Bluetooth timeouts, other-computer behavior and long-term physical reliability remain open. Recognition and optional wording/translation services require setup and may incur separate usage charges. Review translations and wording against the original because meaning or numbers can change. Closing file transcription loses unsaved audio and resume progress.

US$49 is the hardware price. Type is free and open source. No ready-to-use official cloud account or allowance is included. Firmware38 is unchanged; USB flashing and Bluetooth OTA use the same firmware ZIP. This publication updates the Windows MSI download; it does not create an automatic-updater manifest or a macOS installer.

## File identity

- Windows runtime source: `d4b441129b5cc7543329c59a80b05fe56775ed83` · build267.
- Windows MSI: 305643520 bytes · SHA256 `887d1498c1ab58fe528d84162a75850c7f653ce4a15efd61abe1d962894b6c14`.
- The release tag includes a documentation-only follow-up that aligns the download pages; application/build inputs match the runtime source above.
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`.
- Firmware ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`.

## 中文说明

本次发布已在本机安装验证的 Type267。针对最后一次短片段粘贴尚未确认、完整终稿却立即覆盖剪贴板的竞争：先保留原短片段，收到真实回执可提前复制完整终稿，否则沿原有观察窗等到该次粘贴发出后5秒，再按完整复制优先设置复制。此步只更新剪贴板，不额外粘贴，也不拖住“已完成”界面。等待期间用户的新复制（含同文新版本）和新会话所有权继续受保护。观察窗到期仍记未确认；极慢目标超过该窗口仍可能读到整稿，不能宣称所有重复均已消除。

包含之前Type266的改进。修复普通流式路径中尾句只能等待整段结束确认的缺口：已提交边界后的稳定片段可以使用现有识别服务做有界确认，合格新词沿同一增量账本上屏；取消、换会话或来源不匹配的旧结果不能继续写入。片段确认可能增加提供方用量，片段长度与累计回看量受限。识别回调只发布观察，由唯一端点循环先给就绪文字正常交付机会，再判定自动停止。已满足音频覆盖、持续安静等保护的尾句，也不再因句号或确认状态被额外拖住；未确认文字继续保留给正常最终识别。

同时包含Type264的减少重复模型计算、旧唤醒任务取消、线程闲时休眠和检测任务退出后的临时内存回收。模型继续保留以供下一次复用。实际模型代码测试释放约63 MiB，另一种受控分配布局约139 MiB；不同音频和缓存历史的收益会变化，不能当作整套软件的固定降幅。后台唤醒计算仍有短时CPU和内存峰值。

正常完成统一显示“已完成”，普通状态说明不再呈现得像报错。实际失败处理、历史记录和诊断事实保留。实时分段上屏继续，结束后仅复制与历史一致的完整终稿，不再额外粘贴；Windows默认开启完整复制，已有用户选择继续尊重。末段未确认且内容不同时，完整复制可能稍后完成；目标软件若超过上述窗口仍未读取，仍可能读到完整终稿，请留意实际输入结果。普通复制不需要开启Windows剪贴板历史。

RC1仍为预发布。此次包含明确的尾句等待、交付先后、安静收尾与末段剪贴板过早覆盖修复；持续讲话时偶发提前结束、其它应用中的尾句交付、噪声下唤醒和蓝牙间歇故障仍保留自然观察，不能据此宣称所有偶发问题已根治。固件仍为Firmware38，本次更新Windows公开MSI下载。
