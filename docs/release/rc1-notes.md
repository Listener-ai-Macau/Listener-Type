# Listener v1.0.6-rc1 · Type build284 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI · build284](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| Firmware · USB flashing or Bluetooth OTA | [Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip) |

## What’s changed since build283

- **Reduce spurious activity at the end of computer-microphone dictation.** Active Windows computer-microphone recording now uses the pinned Silero VAD v5.0 model. A fixed gain of three applies only to its detector input, preserving low-volume continuation sensitivity; the audio sent to recognition remains byte-for-byte unchanged. The earlier detector remains bundled as a safe fallback if the new resource is unavailable or fails integrity validation.
- **Retain the established ending and output behavior.** The existing three-second quiet policy and current-session/recorder/epoch guards remain. Recording continues to insert text incrementally, including the ordinary trailing-word path, and completion still copies the full final text without an extra paste. Wake-word and speaker verification, device recording, saved settings/data and Firmware38 are unchanged.
- **Keep future endpoint evidence small and private.** Local detector state transitions include bounded numeric recent-level metadata. This change adds no audio recording, transcript logging or permanent helper process.

Controlled Windows checks passed for low-volume continuation after a louder phrase (gains 1, 0.1, 0.03 and 0.01), exact recognition-audio forwarding, STOP safety, shared endpoint timing and the ordinary trailing-word path. The matching MSI, installed executable, model/license hashes, preserved settings/history and restored microphone-wake capture were verified. Source CI passed. The optional new real-microphone comparison was not started and is not counted as acceptance; the original failing audio was not retained. These results do not establish that every intermittent or long-term field case is resolved.

## Included from build283

## What’s changed since build282

- **Keep the tray menu for everyday use.** Removed the internal acceptance-checklist entry and its window-opening handler. Existing style, theme, input source, microphone and quit actions keep their IDs and selection behavior.
- **Native menus follow your interface language.** Simplified Chinese, Traditional Chinese, English, Japanese and Korean now cover tray headings, styles, microphone status and network tooltips. The main window synchronizes the UI language through a separate display-only preference, retained across restarts; Follow system uses the system language. Secondary windows do not overwrite it, and queued older language changes cannot replace a later main-window choice. Native-menu synchronization writes only its display preference; the existing language-selection behavior is retained.
- **Complete device-maintenance translations.** Added the missing device-read/save and USB/Bluetooth maintenance messages in all four non-English locales. All five effective resource bundles have matching keys and interpolation variables. Common settings and maintenance pages, the actual language picker and narrow navigation were exercised in each locale. No firmware update or device write was performed by these checks.

Controlled native checks covered locale persistence/system resolution and the existing menu IDs and selection. Isolated frontend transport checks covered main-window synchronization, rapid choices and secondary-window exclusion. Recording, wake thresholds, automatic ending, streaming/final-copy implementation, providers, dependencies and Firmware38 are unchanged. These checks do not close the original intermittent wake or other real-use limits.

## Included from build282

## What’s changed since build281

- **Stream without a separate switch.** The recording settings no longer expose a streaming-insertion toggle. Recording-time incremental insertion and streaming polish had separate behavior, so the former could continue even when the old polish switch was off. Old disabled values, including already-migrated configurations, now normalize to enabled on load and settings IPC and are saved durably. Existing platform, provider and secure-target compatibility gates remain.
- **Device settings before computer shortcuts.** Settings navigation now starts Recording → Device → Shortcuts. Existing service and writing navigation from build281 is retained.

Controlled native checks covered defaults, old disk/IPC values, durable migration without other preference changes, stable rereads and existing streaming eligibility gates. Actual React and the final packaged frontend confirmed the new order and absent switch. The end-of-recording complete-copy choice and automatic-send settings remain independent and preserved. Recording/endpoint/insertion implementations, model thresholds, storage format and Firmware38 are unchanged. Original intermittent wake and other previously scoped real-use limits remain open.

## Included from build281

## What’s changed since build280

- **Set up one service purpose at a time.** Provider settings separates speech-to-text from text processing, with plain descriptions of their roles. Both forms stay mounted when switching the visible purpose, so entered values and pending saves remain intact. Opening a purpose does not switch the actual provider, submit a connection check or change the recording source. Context-specific text/speech setup still opens its own existing form.
- **Return to the writing you started with.** Opening text-service setup or a key layout suspends the originating workspace dialog while retaining its input, edited result and scroll state. Saved text behaves the same way when opening a layout. The active child has an explicit return label; credential writes still finish before returning. Key layouts opened from a result are siblings of the suspended dialog, so they remain visible.
- **Choose fixed text or another key action explicitly.** The saved-text/result entry now says it sets a fixed key phrase. Inside the layout, fixed text and other key actions have separate views; the supplied text remains available when switching back. Action editing is directly visible, and an explicit purpose/action change reveals the editor in a narrow window. Existing save/revision/unsaved-change protection and device-application rules are retained.
- **Name the input for what it is.** An input draft no longer inherits an unexecuted style name. The style summary explicitly offers changing it; saved results and original text retain their existing storage rules.

Controlled React checks covered retained credential fields without provider/preference mutation, delayed credential saves before returning, one active editing dialog, retained input/edited result, fixed-text/action switching, layout saves and unsaved-return protection. The exact final packaging frontend was also exercised through unmodified browser mocks in all five UI languages and a narrow desktop view. These are scoped software-interface checks, not real cloud, hardware, microphone or clipboard acceptance. Recording, wake thresholds, streaming/final-copy logic, storage schema and Firmware38 are unchanged; the original intermittent wake issue remains open.

## Included from build280

## What’s changed since build279

- **Start with the task.** Tools opens with direct entries for text work and saved writing, followed by existing audio-file transcription and voice questions. Dictation styles, vocabulary and translation settings are grouped below; device keys and phrases stay folded. The three main navigation entries remain.
- **Choose a text task once.** The workspace has one visible choice for editing existing text, drafting from instructions or translating. Message versus email is a subordinate drafting choice. Writing-style options stay folded initially; translation reveals its existing language and engine controls. The previous duplicate drafting buttons and task/style choices are consolidated.
- **Keep the writing while changing tasks.** The existing input, exact-input result cache and edited results remain available when returning to a task. Choosing a task can prepare an existing trial-only style but does not activate a dictation style or submit a model request. Processing and any service costs still require explicit submission.
- **Find drafts and finished writing together.** The library is named Saved text, with corresponding Chinese labels and help links, since it includes both unprocessed input and completed writing. Continuing in the workspace and editing a separately saved copy retain the original storage rules.

The exact final packaging frontend was checked through normal isolated browser controls in all five UI languages, including task/format changes, retained input, explicit save/reopen/continue and narrow-window access. Separate controlled React checks exercised edited-result caching without real services. The independent customer report is separately scoped; no real audio, model output, system clipboard or hardware acceptance is claimed. Recording, wake thresholds, automatic ending, streaming/final-copy logic, storage schema and Firmware38 are unchanged. The original intermittent wake issue remains open.

## Included from build279

## What’s changed since build278

- **Keep everyday settings first.** The settings sidebar now follows Recording, Shortcuts, Device, Providers, Local recognition, Language, Permissions and Advanced. Recording opens with input and recording controls, followed by an expanded insertion-and-clipboard group. Capsule appearance/sound, history/context, startup and diagnostics are below; insertion compatibility stays folded.
- **One place for dictation output.** Streaming insertion moves from Advanced, while final-text copying, filler removal and the optional send key move from Device to Recording. These are the same saved preferences and handlers, available independently of a connected device. Advanced retains a direct link to their new location; moving a control does not alter its setting. The Windows streaming explanation now describes the user-visible behavior.
- **Open maintenance only when needed.** Device firmware/maintenance stays folded and its uploader is first mounted when explicitly opened. Folding it afterward retains the mounted operation. Device settings, key autosave, firmware protocols and firmware itself are unchanged.
- **Start each settings section at the top.** Changing categories or following the old Advanced link resets the content scroll. The active sidebar marker follows layout/language changes, and the microphone picker fits a narrow window.

The matching frontend was exercised in isolated browser conditions: unchanged preferences when opening settings, the moved copy toggle and its retained value, section navigation, deferred maintenance without flashing, and narrow-window access. Independent AI assumed-post-purchase use is recorded separately; no real microphone, device, clipboard or cloud-service result is claimed. The recording backend, wake/voiceprint thresholds, automatic endpoint, streaming finalizer, saved data/schema and Firmware38 are unchanged. The previously reported intermittent wake sensitivity remains open and is not fixed by this settings reorganization.

## Included from build278

## What’s changed since build277

- **Save input before processing.** The text workspace can explicitly save an input draft without a recognition or text-service request. It shows whether the current input is saved and links to that exact saved copy. Later edits save separately using the existing immutable draft IDs and original-text storage. Saved input is available from History → Saved drafts; it is separate from dictation history. The browser preview keeps it in the current page session only, while the installed app uses the existing local draft store.
- **Edit a copy from History.** Open a record in the text workspace, edit it and explicitly save a separate copy. The original history entry stays unchanged. Opening this workspace does not process, send, paste or copy the text. Closing it refreshes the saved-draft count. Entries over the existing4,000-character workspace limit stay complete in History; the copy action explains the limit instead of truncating them.
- **State the processing route accurately.** An unavailable or still-unconfirmed route no longer means that the selected style is local. The browser preview explicitly says it does not process or submit the body. A confirmed native local style, optional service-based shortening and text-service style keep separate notices. Text is still submitted only by an explicit processing action; no credentials, service routes or charges are added by this change.
- **Start settings with everyday recording.** Recording, providers and local recognition appear before device customization. Default settings opens Recording for both microphone and device users; explicit links to device/provider pages keep their destinations. Opening settings does not change the input-source choice. Hardware and firmware controls remain available in their existing sections.

The matching production frontend was exercised in isolated browser conditions. Input-save/reopen, immutable revisions, history-copy editing, the separate library/count, narrow-window controls and the existing automatic/fixed source controls were checked. Independent AI simulated feedback is recorded separately from real customer use. The preview has no real microphone, device, native insertion or configured service, so it does not establish recognition quality or the full real speech workflow. There is no recording/backend, firmware, model, voiceprint, preference or data-schema change in this batch. The prior intermittent acoustic, manual-stop/slow-target and long-term limits remain open.

## Included from build277

## What’s changed since build272

- **Use one native ending path for keyboard and voice starts.** Windows computer-microphone sessions using Volcengine now register the same session capability for speech activity, endpoint policy and recorder stop. Keyboard/UI starts no longer require ownership by the optional wake listener to auto-finish. The existing fresh, fully analyzed quiet interval, current recorder/session/epoch, ordinary trailing-word delivery and complete-copy finalizer remain. Other recognition providers keep their existing ending behavior.
- **Catch up voice input without discarding source audio.** If keyword timing recovery makes processing fall behind, the bounded wake notifications now coalesce into a retained-PCM snapshot through the current capture edge. Late notifications cannot rewind the decoder. Model thresholds, native word-boundary proof, independent complete-phrase confirmation and speaker checks are retained. A controlled blocked queue previously exposed only the first160ms notification while11720ms had arrived; the new path processes the available source and still confirms the full phrase and rejects a near phrase. This is a synthetic queue catch-up result, not a universal latency claim or proof of every real missed wake’s cause.

- **Automatically choose the available input.** Automatic mode prefers the configured Listener only after its recording notify channel is ready; an OTA/USB identity or saved pairing alone does not select it. Without a ready device, it uses the computer microphone. Automatic is the default for preferences without a fixed-source override. Existing explicit fixed-source choices remain fixed until Automatic is selected in Settings → Recording or the tray.
- **Finish active work before switching.** A link change waits for current dictation, question recording/processing and firmware OTA to finish. The current session keeps its original recorder and audio source. Device START/PCM cannot create a competing background session while computer input owns recording. Enabling Automatic also keeps an already attached device capture instead of restarting it.
- **Keep reconnect discovery independent of the selected recorder.** Automatic mode retains the existing bounded device connection supervision while computer input is selected. A restored notify link can therefore become the next source without restarting Type. Routing itself waits on events; only a pending change during active work uses a light bounded interval check. No new standing voice model or firmware change is added. Computer-microphone wake remains a separate optional switch, off by default.
- **Preserve automatic mode when settings are saved.** The automatic/fixed choice survives serialization. A later ordinary save cannot restore an older effective source from a stale UI snapshot. Settings and tray expose Automatic, computer microphone and Listener choices; narrow layouts retain visible source labels and controls.
- **Withdraw the build276 identity change.** In real use, complete wake phrases were confirmed but the new source-bound owner check rejected several owner attempts. A previous normal trial did not establish stability. Build276 was withdrawn, and build277 restores build274’s owner decision and keyword/clock rules while separately repairing queue scheduling. Existing voiceprints, model assets and global thresholds are retained. The earlier acoustic false/missed wake, short-phrase, TV/replay and long-term limits remain open.

Isolated production checks cover both availability directions, active dictation/QA/OTA deferral, a device START during computer capture, an attached link when Automatic is enabled, stale settings, preference migration and the existing cancellation/handoff/reconnect guards. Real React settings save the automatic/fixed choice correctly; the production frontend builds. The installed selected source and listening readiness are checked separately. These are not a complete physical unplug/reconnect cycle or a claim of long-term reliability. After restoring274 from the276 regression, the user reported normal operation; that feedback predates277 and is not presented as acceptance of its new automatic switching.

Build273’s same-capture confirmation recovery and build274’s measured-onset native clock recovery are included. They keep the complete-phrase, retained-source, cancellation and one-time handoff rules. Build272 trailing-word delivery, the native Volcengine quiet-ending path and completion-only full copying remain below.

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

- Windows runtime source: `ba07449998bdcaa447c962ec9516350be95ffc8a` · build284.
- Windows MSI: 307716096 bytes · SHA256 `da1ec359abe415c2b448ac73f2cf7fa2ae3aa9189f87cfaf94aef1f4af296732`.
- The release tag includes a documentation-only follow-up that aligns the download pages; application/build inputs match the runtime source above.
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`.
- Firmware ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`.

## 中文说明

本次Type284：针对电脑麦克风停说后检测仍反复活跃的问题，活动录音改用固定版本Silero VAD v5.0，并仅对检测器输入做3倍增益，实际识别音频逐字节保留。原3秒安静规则、会话与录音器保护、尾句正常流式、完成仅复制、唤醒和声纹、设备录音及FW38不变；模型与完整许可随包，新资源失效时保留旧检测器回退。低声续说／停止安全／共享端点／普通尾句和安装完整性检查已通过，偏好与历史保留。新同源真人对照未开始，不计通过；原偶发问题的长期现场范围继续开放。

此前Type283：托盘移除内部验收清单入口，日常菜单与网络悬停提示跟随简中／繁中／英文／日文／韩文；界面语言独立保存，主窗口串行同步，其它窗口不覆盖。补齐设备和固件维护页漏译，五套资源键及插值变量一致。录音、声纹、结束、流式、完整复制、设备写入实现及FW38不变，原真实使用范围继续保留。

此前Type282：设备移到快捷键上方；去掉流式输入开关，旧配置里已经迁移过的关闭值也统一启用并持久化。录音逐段上屏、润色流式与原平台/提供方兼容保护保留；结束后复制完整终稿、自动发送等独立选项不变，不重新粘贴整稿。录音/收尾/粘贴实现、声纹门限、模型和FW38未改，旧间歇唤醒仍开放。

此前Type281：服务按声音转文字／文字处理分开查看，隐藏时仍保留表单和保存状态；文字工作区、已保存文字进入子设置／按键布局时暂停父窗口显示，返回保留原内容，凭据待保存完成再返回。固定短语和其它按键动作分开选择，窄窗主动切换会显示编辑区；输入草稿不以尚未应用的风格命名，风格摘要明确可更改。保存格式、原文、布局权限、真实提供商、录音、唤醒、流式／完整复制及FW38不变；原间歇唤醒仍开放。

此前Type280：工具页前置文字处理和已保存文字；一处选择整理／起草／翻译，邮件类型在起草下选择，风格选项默认收起、翻译才展开语言与方式。输入和已有编辑结果保留，切换不自动处理、不激活日常听写风格；已保存文字同时容纳输入草稿及处理结果，保存格式和原件规则不变。本机／公开／桌面同步同包；录音后端、唤醒门限、自动结束、流式／完整复制与Firmware38未改，原间歇唤醒仍开放。

此前Type279：录音、快捷键和设备在前，服务准备与语言权限在后；流式输入、完成后复制、去语气词和发送按键集中到录音页，原设置值与实际行为不变。胶囊/声音、历史、启动与诊断下移；固件维护默认收起，首次展开才加载，之后折叠不销毁进行中的操作。切换分类从顶部阅读，窄窗麦克风选择器与选中标记跟随布局。没有更改录音后端、声纹/唤醒门限、自动结束、最终复制、数据格式或Firmware38；唤醒不灵敏的原间歇问题仍开放。界面工程核验与隔离AI模拟使用不冒充真实语音、实物或长期通过。

此前Type278：文字尚未处理、服务尚未配置时，也可明确保存输入草稿，再从「历史 → 已保存成稿」找回；后续修改另存副本，不覆盖旧保存。历史详情新增「编辑副本」，在文字工作区修改、另存，原历史完整保留，打开不自动发送／处理／复制／粘贴；超出现有4,000字符上限时明确说明，不截断。成稿库和听写历史仍分别存放，预览仅当前页面会话，安装版沿原本机保存。

同时修正处理位置的误导：预览／不可用／未确认不再显示成已确认本机处理，原本机风格、服务风格及可选缩短动作各有相应说明，真实提交仍由用户明确点击。设置默认先打开录音，服务／本地识别在设备定制前，设备／提供商深链保持，不修改输入源和已有偏好。本批实际React保存／找回／原件保持／计数／窄窗／输入来源检查与独立AI模拟反馈分开记录；未把预览当真人、真实语音、硬件或全部兼容验收。录音后端／尾句finalizer／完成完整复制／模型声纹／数据schema及Firmware38均未改；原间歇范围继续开放。

包含此前Type277：本次发布已安装并核对实际所选来源启动的 Type277。Windows电脑麦克风使用火山识别时，快捷键／界面开始与语音开始统一注册本会话录音能力，复用说话检测、原3秒新鲜安静证明、同recorder／session／epoch防过期、尾句普通流式及共同finalizer；快捷键自动结束不再要求开启电脑待机唤醒，也不再借用wake runtime归属。其它识别提供方保留原结束方式，手动结束仍可用。

语音定位恢复较慢时，原FIFO可能仍逐个消费旧通知；现在合并有界通知，读到原ring中当前已收到的真实PCM，旧通知不得倒退解码时钟，中间音频不丢、不当静音。原模型门限、native词尾、完整词确认及本人校验保持。受控阻塞对照旧通知160ms／实际11720ms，新路径可处理当前来源，完整词通过而近词继续拒绝、正文音频保持；不是所有场景的固定延迟收益，也未据此关闭全部真实间歇原因。

新增自动选择：Listener 的实际录音通知通道就绪时优先使用设备，断开后回到电脑麦克风；只有USB／OTA可读或历史已配对不算录音就绪。听写、问答录音／处理和OTA进行中先保留原来源，收尾后再切，板子后台新START／PCM不能抢入电脑录音。自动模式下即使当前用电脑，也保留原设备连接监督以发现重连；切换不重开同一条已附着连接，正常等待由事件唤醒，不新增常驻声音模型。设置页和托盘可选自动、固定电脑、固定板子，旧明确手动选择不被更新强改；无固定标记的新设置默认为自动。电脑待机唤醒仍受原独立开关控制，默认关闭。

276已因现场本人唤醒被拒撤回：完整词确认通过后，新身份窗／底线拒绝多次，旧一次“正常”不能抵消后失败。本版恢复274本人判定与关键词／时钟规则，另修通知队列，不靠降低底线掩盖已知错误声音控制，声纹资料、模型与全局门限保持；老间歇漏唤醒／旁人、电视和回放误开等限制继续开放。Owner恢复274后回传“恢复正常”，该回传发生在277安装前，不冒充新自动切换的真人验收。

隔离真实生产路由已覆盖设备可用双向变化、听写／问答／OTA延后、后台START不得抢电脑、启用自动保留现有采集、旧设置保存不得撤回新来源、偏好迁移及原取消／交接／快速重连保护。真实React设置点击和请求标记、窄内容区布局以及前端构建通过；安装后另外核对实际来源和就绪日志。没有据此宣称实物拔插整轮、所有房间／机器及长期表现全通过。包含273同次确认恢复、274可靠源词时钟，原3秒收尾和完成后仅复制保持。

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
