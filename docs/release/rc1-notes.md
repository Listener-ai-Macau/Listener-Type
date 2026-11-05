# Listener v1.0.6-rc1 · Type build264 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI · build264](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| Firmware · USB flashing or Bluetooth OTA | [Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip) |

## What's changed since build258

- **Less repeated background computation.** Retired wake-recovery jobs check cancellation before heavy inference. Model worker threads park between runs. Personal activity and identity checks coalesce adjacent updates while retaining every source chunk and draining the final tail.
- **Release temporary memory after dictation analysis.** Once the final personal-activity worker exits, Type reclaims unused working buffers while keeping its model loaded for reuse. Cleanup does not publish recognition, speaker or stop evidence.
- **Clear completion feedback.** Successful dictation uses the short “Completed” status. Routine completion and other nonblocking status messages no longer look like errors; actual failure states retain their handling. History and numeric diagnostics retain the underlying delivery facts.
- **Better evidence for intermittent problems.** Bounded timing, signal-health and resource counters help distinguish early ending and delayed delivery. Raw audio recording remains off by default.

Controlled local tests of the real detection models reduced CPU time for an eight-second synthetic input by about 42%; this is not a fixed whole-app reduction. The production memory/reuse test released about 63 MiB, and a separate maximum-window-first allocation layout released about 139 MiB. Cleanup took about 12 ms in the production test, and five source-window lengths produced identical scores before and after cleanup in the same loaded model. Memory savings vary with allocation history. Background wake checks still have CPU and temporary-memory peaks; long-term whole-app RAM reduction has not been established across all workloads.

## Dictation and clipboard

Windows retains incremental on-screen insertion. At completion, Type copies the complete finalized text from the same session's history to the clipboard. This final copy does not send another paste. Completion-first copying is on by default on Windows; an existing saved choice is respected.

If a target application is still reading the last temporary clipboard fragment when the complete final is copied, it may read the complete text in place of that fragment. Review the inserted result. A newer user clipboard copy is protected. Windows clipboard history is not required for ordinary copying and pasting. The experimental clipboard-free streaming route remains disabled.

Build258's background wake-recovery receiver fix, guarded final-sentence drain and stale wake-control protections are included. Source/identity guards, live audio delivery and the configured stop wait remain in place.

## Current limits and setup

**This remains a prerelease. Ongoing dictation can still end prematurely.** Capture-continuation confirmation failures and host quiet-stop cases reproduced on November 3 remain under investigation. Resource and presentation improvements do not establish a complete fix for intermittent early ending, tail delivery or acoustic wake misses.

Intermittent Bluetooth timeouts, other-computer behavior and long-term physical reliability remain open. Recognition and optional wording/translation services require setup and may incur separate usage charges. Review translations and wording against the original because meaning or numbers can change. Closing file transcription loses unsaved audio and resume progress.

US$49 is the hardware price. Type is free and open source. No ready-to-use official cloud account or allowance is included. Firmware38 is unchanged; USB flashing and Bluetooth OTA use the same firmware ZIP. This publication updates the Windows MSI download; it does not create an automatic-updater manifest or a macOS installer.

## File identity

- Windows runtime source: `73d8f10005cb098a7c069ab8e6a8e9c7e64b20a0` · build264.
- Windows MSI: 306823168 bytes · SHA256 `728c43d621243d0e0b4465c584a9da9463b67284217d9a3ceea16a412af2a2d6`.
- The release tag includes a documentation-only follow-up that aligns the download pages; application/build inputs match the runtime source above.
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`.
- Firmware ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`.

## 中文说明

本次集中发布已在本机安装验证的 Type264，包含减少重复模型计算、旧唤醒任务取消、线程闲时休眠，以及检测任务退出后的临时内存回收。模型继续保留以供下一次复用。实际模型代码测试释放约63 MiB，另一种受控分配布局约139 MiB；不同音频和缓存历史的收益会变化，不能当作整套软件的固定降幅。后台唤醒计算仍有短时CPU和内存峰值。

正常完成统一显示“已完成”，普通状态说明不再呈现得像报错。实际失败处理、历史记录和诊断事实保留。实时分段上屏继续，结束时仅复制与历史一致的完整终稿，不再额外粘贴；Windows默认开启完整复制，已有用户选择继续尊重。目标软件若读取最后片段特别慢，仍可能读到完整终稿，请留意实际输入结果。普通复制不需要开启Windows剪贴板历史。

RC1仍为预发布，持续讲话时偶发提前结束、部分尾句交付、噪声下唤醒和蓝牙间歇故障尚未整体解决。此次资源优化不宣称这些问题已经根治。固件仍为Firmware38，本次更新Windows公开MSI下载。
