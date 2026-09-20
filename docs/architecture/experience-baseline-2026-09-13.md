# 2026-09-13 用户体验保留基线

用户原话：“但我觉得现在这个体验是比之前昨天那个好很多，你可以先记录一下”。这是日常体验的正向反馈，不是 A–G 全部通过或同意发布。

当前安装 Type 1.0.5，exe SHA-256：93CDDDF618EF54C301CBEA7073E4774B399063D44B12CA380C4E1B66AF4266DE。
保留安装包：.artifacts/repair-20260913-pending-wake-confirmation/ListenerType_1.0.5_x64_en-US.msi，SHA-256：D4D8580AAAD36EA9F73C18D2927616CCD0FE8940C88F3EA9A31A3D9A014DABEE。
Firmware 1.0.5 build cefb802eed6e36d1，bin SHA-256：5920782CF962B4D40E73CB02120583D27E95AA668D2884A6CA409E7C60521B86。
日常设置：streamingInsert=true、allowNonTsfInsertionFallback=true、removeFillerWords=true、recordAudioForDebug=false。

必须保住：正常唤醒、本人正文完整、句中停顿、预览逐步显示、说完自动结束及最终输出。不因修抗干扰而收紧声纹阈值、吞掉本人正文，或为单条测试增加文字特例。

下一项只追踪已保存会话 899c3910-1c74-43f3-9b70-b74ce5bbd39a：ASR 过滤后的终稿正确，但协调层通过旧预览重新带回旁人尾句。先复现最终文本的来源错误；再验证干净长句、停顿和旁人尾句。若候选退步，撤回该候选，保留此体验基线。源代码检查与实机验收分开报告。

源码快照位于任务目录 work/before-final-preview-provenance，包含本次编辑前关键文件及全部已跟踪修改的补丁。历史问题、严格逐字失败和发布缺口继续记录在 release-repair-plan-2026-09-13.md。
