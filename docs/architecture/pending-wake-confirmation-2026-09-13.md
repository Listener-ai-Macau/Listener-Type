# 未完成口令确认不能触发正式录音

状态：修复候选，尚未联合验收或放行发布。

用户再次反馈“总是误唤醒”，已核对本机日志。2026-09-13 19:04（UTC 11:04），固件候选 2961768663 在独立口令确认仍运行时，因为 `max(kws_waited_ms=0, secondary_attempt_ms=351) >= 60`，进入 `stage2 timeout fail-open KeywordModel`。下一行 `local_confirmation_ms=0`、`owner_policy=enrolled_non_match`、`gate_decision=Accept`，随后绑定正式会话 5d097ce1-f9a9-41a0-b598-196dd4d99ba1。

当时运行 Type 9A3147CB，Firmware cefb802eed6e36d1。原日志范围保存在 task work/pending-wake-confirmation-incident.log；task outputs/pending-wake-confirmation-incident.md 记录来源和范围。不能用仅截至 10:50 的旧 clean-1 快照来否认 11:04 的日志。该会话没有调试 WAV，因此此证据能证明确认旁路，不能独立证明用户当时具体说了什么。

旧逻辑混淆了“唤醒响应时间目标”和“口令证据”。模型越忙、命中前的探索确认越早启动，越可能在 KWS 命中的同一回调立即放行。源码里的历史 “XiaoAi-style” 注释不是成熟产品如此实现的证据。

本次移除 `PendingSecondaryDecision::AcceptKeywordModel` 和对应运行分支。未完成确认继续等待并保留录音；完成的本地口令、已有明确 Absent、终段重试与手动入口保持既有路径。没有提高声纹阈值、禁用自动唤醒、改变去语气词或缩短停顿保护。独立确认运行失败/不可用的 fallback 是另一条既有路径，本次没有声称所有误唤醒入口都已关闭。

行为回归在旧代码上失败：60/351/1000/4000 ms 及极大等待值都不能把 pending 变为正例。补充检查 KWS 配合 ExactStart/PresentLater，以及无需 KWS 的 ExactStart 仍可确认；既有终段/声纹/手动测试继续运行。第一次补充测试误假设“无 KWS 的 PresentLater 可激活”，经现有实现核对后修正测试，未放宽产品规则。

当前候选也包含上一步覆盖范围合并修复（1195771C 包已构建安装但未独立执行完整七例，不能给它套用 9A 的结果）。最终 pending-wake-confirmation 包需核对安装哈希后，统一复测固定七正例和无口令负例。A 正常唤醒不退化与 B 误唤醒减少必须同时满足；单个负例通过不代表全部 B 修复。真实当前误唤醒缺失音频，后续仅在明确诊断期间留存新样本；结束后关闭录音和调试端口。

## 本轮安装验收结果

最终安装 Type `93CDDDF618EF54C301CBEA7073E4774B399063D44B12CA380C4E1B66AF4266DE`，源码检查 1338 passed / 0 failed / 31 ignored，cargo check 及 Web/brand/cloud/traceability 通过。七个实机正例全部产生完成会话、7/7 音频交付完整，但严格逐字只有停顿长句通过；其他六例保留失败，不代表本次代码必然使错误率升高（回放环境及云端修订有波动，不能据单轮推断）。六次自动唤醒可进入正式录音，手动入口也可用；正常案例走完成的 LocalTranscript 确认，未靠旧 pending 超时放行。

无口令正文约 46 秒有明确 Reject、无正式录音/输出；背景素材同样无输出，却没有门控记录，只能说本次未触发，不能视作该分支通过。登记本人匹配的无口令、原现场误唤醒音频仍未覆盖。B 不得标为彻底解决。

E/G 分层结果在 task outputs/pending-wake-confirmation-final-stage-evidence.json：正常长句云端终稿正确，最终保留旧预览多余字；自动开头漏字、手动“你你”在云端原文中已存在；两个旁人样本过滤后去掉旁人，最终又带回旧预览。覆盖范围修复在 long-other 命中一次，说明进入了新路径，但最终旁人泄漏仍失败。下一步必须修提交/预览归属及首音节证据，不把新增条件的命中当成产品修好。

已恢复 streamingInsert=true、allowNonTsfInsertionFallback=true、recordAudioForDebug=false、removeFillerWords=true，正常重启并验证相同哈希，9222 无监听。整体 releaseReady=false。
