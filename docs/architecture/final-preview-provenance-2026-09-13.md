# 最终输出的预览来源修复

问题证据：93CD 实机会话 899c3910-1c74-43f3-9b70-b74ce5bbd39a 的 ASR authoritative_preview_text 为完整 80 字本人正文，协调层却以 partial_preview_recovery 输出 110 字，重新带回旁人尾句。来源是 visible() 的显示高水位：该槽允许临时文字，并在后续权威更正缩短时保留旧显示。

修改：最终候选从同一 session_id 的 authoritative() 获取，继续执行唤醒口令清理；不回退到 visible()。保留已接受预览的终稿恢复、润色和插入路径。预览刷新、结束计时、声纹阈值、音频交接和 Firmware 不变。预览仍可能先显示随后被排除的文字，这一项不宣称完全解决显示侧抗干扰。

范围限制：上游若已把旁人或错误字送入 authoritative()，此修改不会修正；短句旁人、云端原文漏首字/重复仍需分别追踪。旧的“全部显示文字必须进入输出”规则与临时文字禁止进入输出的规则冲突，本次只保护有权威来源的正文，不把显示本身当作说话人证据。

修复前运行新的 product_final_ 回归：11 passed / 3 failed，失败包括长句旁人尾段、带口令尾段和仅临时文字。修改后结果与安装实测待补。新增测试还保护较长权威正文在 provider 短/空结果时保留，隔离旧 session。

用户认可的日常体验基线、保留 MSI、源文件快照见 experience-baseline-2026-09-13.md。验收使用既有固定文本/WAV，不根据实际输出改期望；输出证据在任务 outputs/final-preview-provenance-*。

源码验证：Rust 1343 passed / 0 failed / 31 ignored；cargo check、npm build、brand/cloud/traceability/asr-latency 通过。没有修改声纹模型或 Firmware。安装实测尚待完成，不能以该检查结果代替。

## 安装实测与交付状态

已构建并安装 Type exe E46A7EC5F746569C91F888B74361C8F351FE42EA2C0DA4F0F459542ED024234E，MSI SHA DF155D00378429B82225D6D8ADC872ABB277CC76FB0E4F84512DDA1629E4BE90。Firmware 仍为 cefb802eed6e36d1。正常重启校验通过，streamingInsert/fallback 恢复 true、debug recording=false、removeFillerWords=true、9222 关闭。

六次固定回放，自动报告严格逐字 2 pass / 3 fail / 1 unverified。五个完成会话全部音频交付完整。正常 80 字长句与停顿长句完整，末次预览增长到完成 2977 / 2824 ms；这不等于首字延迟测量或全部 A–G 通过。

长句旁人两次仍输出旁人；第一轮 provider 的 speaker_filtered_result 就含旁人，local_non_target_end_ms=None，部分尾窗被判 Target。不能声称本次取值修复解决了这个上游归属问题。短句会话 821bf512-fde3-4156-8384-8393d8d22fe7 在 speaker_filtered_result 已去旁人，但 final arbitration=provider_owner_recovery 又选回原文：explicit_non_owner_tail=false / owner_recovery=true。该轮不是最终 visible 槽恢复，也不是 last_preview 长度恢复；后者仅 20 个实质字符，恢复结果 38 个。其 provider final 稳定 speaker 0 本文到 5702 ms，speaker 1 新段从 6422 到 10242 ms。下一项需要复现 owner-continuation 例外为何放过这段，不直接删除全部本人恢复逻辑。

第三次长句旁人回放没有正式完成，summary 标记 unverified。人工对齐日志则证实设备采集，2961768792 的多次 local confirmation=Absent，KWS 未命中，最终 phrase_signal=None / Reject；因此这是带口令正例未唤醒的失败现象，不能当成抗干扰成功。没有该拒绝候选的原始 WAV，尚不能认定是首音节采集问题还是口令识别问题。保留现有报告原始分类及这项补充，不藏掉失败。

一次 pause 测试准备因前一捕获仍占 COM5，在外放前失败；未发生第二路同时外放，准备失败日志保留为 final-preview-provenance-pause-preflight-serial-busy.log，串口释放后才正式回放。

结论：仅能确认产品最终候选来源的确定性回归闭环；本轮实机未证明 G 得到改善，不能以单点源码修复宣称抗干扰完成。正常/停顿对照保留，本次不改变声纹阈值或发布稳定更新。后续先用短句实际时间线复现错误的 provider_owner_recovery，再同时保护正常本人跨云端 speaker-id 分段的用例；另需保存被拒绝的唤醒候选 PCM 才能闭环 A/B。
