# Listener v1.0.6-rc1 · Type build276 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI · build276](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| Firmware · USB flashing or Bluetooth OTA | [Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip) |

## What’s changed since build272

- **Use the current keyword’s source for computer-wake identity.** With reliable native first/last-token coordinates and at least one second of real source, owner verification uses the keyword window with 200 ms of preceding context and at most 250 ms after the keyword. Earlier speech in the native segment and later dictation no longer supply identity for this path. Original keyword-end body handoff is unchanged. Missing, expired or invalid coordinates cannot authorize an unrelated crop. Genuine short keywords retain the established short-input compatibility path.
- **Keep keyword length separate from speaker identity.** For this source-bound path, an enrolled owner’s unmatched score below the existing 0.40 fast-local owner floor is rejected even when the complete phrase spans 1.8 seconds or more. The older duration-based full-phrase fallback cannot override that rejection. Existing strict voiceprint matching, independent phrase confirmation, cancellation and session ownership remain required; saved voiceprints, model assets, global thresholds and device owner policy are unchanged.
- **Recover a reset keyword clock using measured speech.** Included from local build274, an unproven keyword clock can use the existing VAD’s measured source onset to propose a strict native re-decode. Only native word coordinates within actual retained PCM prove the keyword end. VAD does not choose that end or stop dictation; no new permanent standby inference loop or model download is added.
- **Recover confirmation within the same capture.** Included from local build273, temporary confirmation-helper readiness and one changed-context retry share the original four-second budget and retained-source limits. Complete-phrase-at-start, generation, cancellation and one-time recorder handoff protections remain; the first positive confirmation adds no wait.
- **Retain the delivered dictation behavior.** Build272 trailing-word streaming, the native Volcengine quiet-ending path, normal incremental paste and completion-only full copying remain. This update does not change firmware or board wake policy.

The old segment-wide input admitted the wrong synthetic voice in a two-voice native test. With the focused source and production admission, the keyword’s voice was accepted and the earlier voice rejected. A separate slower-phrase control exposed a remaining duration fallback in candidate275: its 2.370-second focused input scored 0.3676 for the wrong voice yet was still admitted. That completed candidate was withheld from installation and publication; build276 rejects that control and accepts its correct speaker. These are controlled synthetic source-mixing cases, not a measured population false-accept rate or replay/liveness test.

In one explicitly started, in-memory real microphone comparison, the same phrase’s old segment score was 0.3221 and its focused-source score was 0.4226, passing the existing strict 0.42 verifier. The old loose policy would also have admitted that particular trial; it did not reproduce the earlier rejected owner attempt. No diagnostic recording, transcript or voiceprint feature was saved. Build276 has been installed locally with its selected input, preferences and history preserved, and real microphone PCM/listening readiness checked. In the shared normal-use feedback popup, the user subsequently selected “Normal” for first-attempt capsule appearance and automatic ending. This is a user report, without an independent editor/clipboard read; all short-keyword cases, other rooms/speakers and long-term intermittent reliability remain open.

## Included from build272

### Trailing-word streaming since build271

- **Confirm an unchanged new prefix while later words keep arriving.** The pending-fragment worker now uses the existing rolling revision ledger to plan complete, mature words after the submitted source boundary. Appended live words no longer restart the age of that earlier prefix or block an already-confirmed same-source result. The existing one-second maturity/context, sixty-second clip bound, single in-flight request and cumulative two-times-PCM replay budget remain in force.
- **Accept a new-only decoder result at the correct audio occurrence.** A cropped decoder can legitimately omit the old, already-submitted context. For admitted ordinary capture, its first word must start after the submitted interval and match the first unsubmitted parent occurrence using the existing unique whole-token/retained-PCM midpoint matcher. A later repetition, unknown timing or overlapping old interval cannot skip words or repeat a submitted prefix. New words retain the child decoder’s own confirmation and coordinates and must match the current parent prefix. A conflicting child rendering stays pending. Identity-projected sources keep their existing strict seam/ownership checks.
- **Keep confirmation observable and source-bound.** Admitted capture uses the existing same-response native rendering certificate only when the rendering exactly maps to that decoder’s timed words. A rejected worker reports a reason and numeric lengths, never transcript content. Normal incremental paste, native quiet ending, history and final-copy-only behavior are retained.

The complete real-service synthetic replay produced an eligible new fragment at 6.330 seconds while the parent was still changing; the next parent batch arrived at 10.039 seconds, and the 42-character final was retained. This compares the same synthetic input against the first unreleased candidate’s blocked path, not a fixed build271 or whole-app performance claim. Captured synthetic dropped-context responses, later-repeat/unknown-source counterexamples and the native automatic-ending/receipt guards are covered. Existing configured recognition usage can increase through bounded fragment requests. In one installed build272 natural dictation, the user reported more timely trailing words and automatic ending. Numeric logs show three ordinary paste dispatches totaling 54 display characters before native STOP, followed by a 54-character full-copy Stored result and standby restoration, with no final-remainder paste. The editor and clipboard contents were not independently read, and this single use does not establish all intermittent cases resolved.

## Included from build271

### Complete native endpoint repair since build270

- **Close native input without waiting for the final recognition that requires its EOF.** The first build270 physical trial still required a checkmark: native speech detection was working, but provisional body and owner-continuation holds from the device controller prevented a normal stop. Build270 was not promoted to the public MSI download. Accepted native wake sessions without a body speaker tracker now explicitly reuse the existing continuously confirmed-silence reducer before these owner-only waits. The shared speech interval, analysis coverage, freshness and active-speech protections remain unchanged. This is source capability routing, not permission to treat a pending transcript as silence.
- **Preserve delivery and finalization.** The ordinary streaming delivery pass still precedes the endpoint decision. In-flight editor dispatch and receipts retain their common stop guard. A qualified native stop enters normal recorder drain and ASR finalization, so pending words can finish and the complete history text is copied without another paste.
- **Verify the complete controller path.** An isolated synthetic PCM replay runs the real VAD through the common automatic no-body/body controller, provisional recognition hold, endpoint proposal and matching native Recorder stop. The unchanged legacy branch is retained as a failing control for that provisional-body scenario. New speech, unknown/stale evidence, old session/epoch, device transactions and final-clause checks remain covered. After installation, one real microphone-wake dictation completed through the native automatic stop, full final copy and resumed idle listening. The user reported that it appeared normal. This establishes that trial, not all rooms or long-term intermittent behavior.

## Included native adapter from build270 (controller gap corrected above)

### Changes implemented since build269

- **Finish computer wake dictation automatically with Volcengine.** Build269 could start a real computer-microphone recording, but its existing endpoint requests still reached a BLE-only stop dispatcher. Accepted microphone-wake sessions now stop their own native recorder and enter the ordinary finalization pipeline. Device STOP writes and manual microphone sessions keep their existing paths.
- **Use the existing speech and ending rules.** Native PCM now feeds the same bounded local VAD and source-sample clock as the endpoint requires. The common automatic body/no-body lifecycle uses the already-proved keyword trim; it does not remove a later literal wake phrase from body text. No new silence timeout is introduced. Native stop admission requires fresh analyzed silence, the current speech epoch, and the matching session and recorder. Continuing or newly started speech, unknown/stale analysis and replacement sessions reject an old proposal.
- **Keep the final clause on the ordinary stream.** Ready words retain their normal incremental pass before endpoint reduction. Normal native finalization then saves history and copies that same complete final text without sending another paste. The microphone returns to idle listening after completion. Hotkey/checkmark ending remains available; other recognition providers retain manual ending.

The native capture adapter was checked with the existing real VAD on isolated synthetic speech, silence and renewed speech. Shared endpoint, stale-stop, device-stop transaction and final-clause regressions passed. These checks are not a claim that all real-room intermittent ending cases are resolved; the fresh physical automatic-end result remains under natural use.

## Included from build269

### Owner and short-phrase repair since build268

- **Use the established owner decision for computer wake.** Build268 could recognize and independently confirm the complete phrase but still reject an existing owner because its new microphone entry interpreted the embedding verifier’s raw flag as the final decision. Computer wake now carries the confirmed phrase relation into the same owner/phrase evaluator and decision kernel used by the device path. Existing thresholds, protected voiceprints and device decisions are unchanged.
- **Retain short wake audio through the original retry ladder.** A short or initially weak owner window can continue receiving the same microphone source through the established bounded verification windows. Cancellation and source expiry stop the retry; following dictation audio stays in the handoff buffer and is delivered once. Unconfirmed phrases, low scores and verification errors still cannot authorize a wake.

Regression checks use the observed rejection’s numeric score and duration, compare computer decisions with the unchanged device policy, cover short-window continuation/cancellation, and rerun full/near phrase and long-idle synthetic model cases. The first build269 spoken-wake trial was reported able to record; its automatic-end failure is addressed above. The standby capture, clipboard, recognition services and firmware from build268 are retained.

## Included from build268

- **Optional computer microphone wake.** In Settings → Recording, select Microphone and enable “Wake with computer microphone”. This independent switch is off by default. It uses the shared Chinese wake phrase and existing voiceprint policy; a strict local keyword hit also needs independent local phrase confirmation. Recognition after wake uses your configured service or model.
- **Keep the same live microphone capture.** A bounded in-memory source buffer retains words spoken after the verified keyword boundary while ordinary dictation starts. Cancel, input/device changes, manual dictation, question recording, microphone tests and firmware OTA keep their ownership guards. Idle listening resumes after recording ends. Build270 adds the shared automatic-end path for Volcengine as described above; other recognition providers use the hotkey or checkmark.
- **Handle long-idle keyword timing.** Ambiguous native keyword timestamps are checked by re-decoding short windows from the same retained source. An early unproven label can receive a bounded amount of subsequent audio; an estimated observation time never becomes permission to discard audio.
- **Bound standby work.** The PCM buffer retains at most 12 seconds, decoder history is renewed with overlap, and callback logging is suppressed during standby. Phrase confirmation reuses the existing helper and transfers PCM in memory without a temporary recording file or ambient transcript response. Standby audio is not uploaded or archived; normal dictation history/debug choices still apply after wake.

Offline synthetic tests cover the full phrase and three similar phrases after 0, 34 and 95 seconds of silence, including dropped inference-queue chunks. Capture handoff, cancellation and existing recorder/helper behavior were checked separately. These tests do not establish physical wake success in every room or long-term reliability. Earlier intermittent stop and target-application limits remain below.

## Included from build267


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

- Windows runtime source: `c8dd8a5d590770980a4ec31c73416da977a6da82` · build276.
- Windows MSI: 305733632 bytes · SHA256 `55ba0499cd7a4bc81801efa72cb1c153bd5993668ca37b08731bf30f395dddc2`.
- The release tag includes a documentation-only follow-up that aligns the download pages; application/build inputs match the runtime source above.
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`.
- Firmware ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`.

## 中文说明

本次发布已在本机安装并核对启动的 Type276，集中修复电脑麦克风唤醒的源时间恢复、同次确认恢复与身份取样。原生segment起点可能包含唤醒词之前的讲话，旧补窗也可能引入后面正文。对有可靠首末token坐标且真实长度足够的口令，本版只取当前词附近前200ms／后最多250ms做声纹比对，正文仍按原词尾交接。缺失、过期、未来或损坏坐标不能借其它片段授权；真实不足模型1秒输入的短口令保留既有兼容路径，该范围未冒称全部修复。新source-bound路径中，已登记本人未匹配且低于现有0.40快速本人底线时，即使完整词达到1.8秒也不得由旧时长回退放行；未改已有声纹、模型、全局门限或板子判定。

受控双声音真实模型对照中，原宽输入会接受前面错误声音，新生产入口拒绝错误者、接受当前词说话人。较慢口令另暴露候选275仍被旧时长回退放行：2.370秒窗口错误者0.3676仍通过；275虽已打包，但未安装／未公开，失败保留。276补齐源身份拒绝，慢口令错误者被拒、真正说话人通过。一次经明确开始的真人同源内存对照，旧窗口0.3221、新窗口0.4226通过原严格0.42；该次旧宽松策略本来也会放行，不能称复现或根治了此前0.18拒绝。未保存诊断音频、文字或声纹特征。已验证276安装文件、实际电脑源PCM／待机启动、偏好和历史保留。随后Owner在原共用正常使用反馈弹窗，对“第一遍出现胶囊并正常自动结束”选择“正常”；这是本人使用回传，未独立读取编辑框或剪贴板，未把启动就绪冒充实际验收。电视、本人录音回放、所有短口令及长期／跨房间表现仍待实际使用判断。

包含273同次采集的本地词确认短暂失败恢复和274安静后词时间恢复：仍受原4秒预算、真实保留音频、完整词开头确认与取消／一次交接保护；可靠词尾由原生KWS提供，VAD只建议裁片，不决定词尾或自动结束，不新增常驻模型。273、274本机仍有漏唤醒的旧反馈继续保留，两者未单独公开。原3秒安静收尾、272尾句普通流式、历史和完成后仅复制完整终稿保持；固件仍为38。

包含已交付的Type272尾句流式修复：尾句补确认改为复用原滚动稳定记录，先处理已提交源边界之后的完整稳定新词，后续加字不再把前面稳定部分一起重等；同源child已确认结果也不再被父候选的新增长挡住。裁片识别若省略已上屏旧上下文，只有其第一个新词确实对应父流第一个未提交音频发生，才沿原whole-token／PCM midpoint匹配接续；不能跳到后面的重复词、借用父词时间或覆盖已提交内容。子结果与当前父文本冲突的部分继续待确认。保留原成熟／上下文时长、60秒单片／累计2倍PCM、单请求、防过期与身份边界，严格身份投影路径保持。

完整非私人合成语音实测中，父流仍在加字时，新片段在6.330秒可交付，较等待下一主批10.039秒约早3.709秒，42字完整终稿保留。该对照是本批首个未发布候选的受阻路径，不是所有输入、271或整机CPU的固定降幅；本机272实际听写中，Owner回传“尾句更及时，也能自动结束”；数字记录为20／13／21字三次普通流式分批，全部54字在自动STOP前约2.88秒已派发，随后54字完整复制Stored、恢复待机，无收尾补粘贴。未独立读取输入框或剪贴板正文，单次正常不外推全部间歇场景。片段确认使用已配置识别服务，可能增加有界用量。当前自动结束、原流式粘贴、历史以及完成后仅完整复制保持。

包含已验证的Type271电脑自动收尾修复。270真实回传仍需点勾，未提升为公开MSI；当时已经接通电脑VAD和停止出口，却仍被设备控制器的正文待确认／机主续句义务挡住。271明确电脑原生录音能力：先给尾句普通流式交付机会，再复用已有连续安静确认器关闭输入，让需要结束输入才能完成的末句继续进入正常识别与历史保存。正在进行的输入提交／回执、VAD完整静音／覆盖／新讲话、会话与录音归属保护保持，不增设静音时长、不把文字等待当作安静证据。已新增真实模型＋合成音频从共用控制器到native Stop的完整回放，并用旧分支对照证明原等待问题；安装后一次本人真实电脑唤醒听写，日志确认连续安静约3秒后自动停止、完整终稿复制及恢复待机，Owner回传“看着好像没啥问题”。此结论覆盖本次实际使用，不外推全部噪声及长期间歇表现。以下270接入及既有修复包含在本版。

269真实回传已能唤醒录音，但停止请求仍被只接受板子的分发挡回；电脑PCM也未接入原本地说话检测。270把已接受的电脑唤醒会话接到同一VAD、源采样时钟及自动正文／无正文规则，匹配当前会话和录音后停止电脑采集，再进入普通收尾。没有新增静音时长；讲话、待定／落后检测、新会话或旧停止请求不能越过保护。该自动结束目前适用于火山识别，其它识别方式继续快捷键／勾号结束。尾句先正常流式，收尾后保存历史并只复制完整终稿，不再额外粘贴；完成后恢复待机。定向离线真VAD与合成音频、停止归属、原板子事务和尾句检查通过，实际房间的新自动结束表现继续按自然使用记录。

包含269的机主与短词修复。修复268电脑唤醒已识别完整词，却因直接采用底层声纹标志而误拒绝既有机主的接入缺口：携带真实完整词确认关系，复用设备路径正在使用的综合判断与仲裁。短音频窗口沿原有有界重试继续接收同源音频，正文保留给一次正常交接；取消、来源过期、未确认词、低分及验证异常的拒绝保持。没有修改声纹阈值、保护资料或设备原有判定。回归覆盖现场数字证据、与原策略的一致性、短窗／取消及完整词／近似词／长静音合成模型案例；269首次自然回传已能录音，自动结束缺口由上文270处理。包含以下268功能与既有限制。

Type268 新增电脑待机唤醒。新增独立的电脑麦克风待机唤醒开关，默认关闭；在「设置 → 录音」选择麦克风后可启用。沿用共用中文唤醒词及原声纹策略，关键词命中还需现有本地助手确认完整词，近似词不能仅凭关键词标签开始听写。同一条录音流按可靠唤醒词边界交接正文，避免重开麦克风的空档；用快捷键或勾号结束后恢复待机。270已在上文补齐火山识别的共用自动结束路径，其它识别方式仍手动结束。待机内存音频最多12秒，确认通过内存传输，不上传、不保存临时录音，也不回传环境转写。唤醒后的识别仍用已配置服务或模型，历史与调试录音沿原设置。

本地合成音覆盖0、34和95秒静音后的完整词、三个近似词及推理队列缺块，长静音边界不可靠时只用同源短窗重新取证，不能以估算位置裁掉开头。取消、替代会话、录音资源与原助手行为相关检查通过；不据此宣称所有真实房间、噪声或长期实物唤醒均已通过。保留以下既有修复与限制。

包含 Type267 的剪贴板修复。针对最后一次短片段粘贴尚未确认、完整终稿却立即覆盖剪贴板的竞争：先保留原短片段，收到真实回执可提前复制完整终稿，否则沿原有观察窗等到该次粘贴发出后5秒，再按完整复制优先设置复制。此步只更新剪贴板，不额外粘贴，也不拖住“已完成”界面。等待期间用户的新复制（含同文新版本）和新会话所有权继续受保护。观察窗到期仍记未确认；极慢目标超过该窗口仍可能读到整稿，不能宣称所有重复均已消除。

包含之前Type266的改进。修复普通流式路径中尾句只能等待整段结束确认的缺口：已提交边界后的稳定片段可以使用现有识别服务做有界确认，合格新词沿同一增量账本上屏；取消、换会话或来源不匹配的旧结果不能继续写入。片段确认可能增加提供方用量，片段长度与累计回看量受限。识别回调只发布观察，由唯一端点循环先给就绪文字正常交付机会，再判定自动停止。已满足音频覆盖、持续安静等保护的尾句，也不再因句号或确认状态被额外拖住；未确认文字继续保留给正常最终识别。

同时包含Type264的减少重复模型计算、旧唤醒任务取消、线程闲时休眠和检测任务退出后的临时内存回收。模型继续保留以供下一次复用。实际模型代码测试释放约63 MiB，另一种受控分配布局约139 MiB；不同音频和缓存历史的收益会变化，不能当作整套软件的固定降幅。后台唤醒计算仍有短时CPU和内存峰值。

正常完成统一显示“已完成”，普通状态说明不再呈现得像报错。实际失败处理、历史记录和诊断事实保留。实时分段上屏继续，结束后仅复制与历史一致的完整终稿，不再额外粘贴；Windows默认开启完整复制，已有用户选择继续尊重。末段未确认且内容不同时，完整复制可能稍后完成；目标软件若超过上述窗口仍未读取，仍可能读到完整终稿，请留意实际输入结果。普通复制不需要开启Windows剪贴板历史。

RC1仍为预发布。此次包含明确的尾句等待、交付先后、安静收尾与末段剪贴板过早覆盖修复；持续讲话时偶发提前结束、其它应用中的尾句交付、噪声下唤醒和蓝牙间歇故障仍保留自然观察，不能据此宣称所有偶发问题已根治。固件仍为Firmware38，本次更新Windows公开MSI下载。
