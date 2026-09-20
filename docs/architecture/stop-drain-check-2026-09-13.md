# 停止后的音频交付修复（2026-09-13）

状态：源检查通过，安装实机复测进行中；不是发布通过。

## 已证明的问题与修改

旧安装 Type 73D0869D + 固件 cefb802eed6e36d1 的会话 74b7ab15：主机于 08:55:03.527Z 提前发送云端 final，云端 captured PCM 为 192000 bytes；设备 08:55:04.714Z 才传完，归档与 normalized PCM 为 237664 bytes，差 45664 bytes / 1427 ms。STOP 写成功不等于键盘 FIFO 已传完。移除 endpoint 回调中的提前 final，保留即时处理中反馈，由现有 physical completion -> flush_streaming_pcm -> end_session 顺序结算。无正文且旧段已结束的逻辑结算路径保留。

增加 opt-in audioDelivery 数值诊断：retained bytes、queued captured bytes/frames、provider reported duration。队列计数不冒充网络收到证明，正常模式不落盘。

结构回归旧代码 RED；新增实际 PCM consumer 测试覆盖即时停止反馈后到达的 1427 ms 数据及末尾非整帧。全量 Rust 1330 passed、31 ignored；cargo check、npm build、brand/cloud/traceability 通过。全量检查发现两处旧测试仍要求声纹匹配但无口令也 Accept，按用户 9/13 明确要求改为 Reject（产品 gate 本轮未再改变）。

## 另一个问题没有被掩盖

同一失败录音 0–6 秒本地 ASR 可辨认完整正文；仅最后 1427 ms 主要识别到“啊”，所以提前 final 不能解释整句只得到“1”。失败全录音直接重送生产云端：两种加速配置均空；降至四分之一音量仍空；80 Hz 二阶高通未恢复（空/儿）。上述处理未加入产品。

手动对照 20f55711 的原录音云端两轮均完整。自动与手动高相关窗口（r >= .7）全部相差 -12 ms；正文 2600–4600 ms 连续，无重排证据。低相关静音窗口的随机峰不能算音频回跳。

交叉诊断：把自动录音 2400–4900 ms 正文放入手动录音相应位置，云端两轮完整；将手动正文放入自动录音，其余保留，两轮识别正文但缺“你”。这提示整段声学上下文与正文质量共同影响云识别，尚未证明具体降噪/端点根因。实验拼接仅用于定位，不算实机验收，也没有修改固定答案。

证据位于任务 work/stop-drain-*、work/*comparison-provider-ab.json、work/owner-failed-full-provider-ab.json、work/auto-highpass-provider-ab.json、work/*body*surround-provider-ab.json。

## 9196F1FD 安装后七轮实机

Type SHA256 9196F1FD0A4804351A3DFF28C99B6FEE45AF9E873341141AE6ED0960757CEA2F，MSI FF58EE64FBF6B595BC60EC85C2C0907120D63686E3172BDDFCAFB78690EBD9A7，固件 cefb802eed6e36d1。

七轮固定正文检查 2 通过、5 失败：停顿长句和自动本人短句通过（后者原失败为“1”）；普通长句在/再，手动短句增 1/漏 是，背景开头混入且尾巴截断，短旁人场景漏 你，长旁人场景全部泄漏。不能扩大为修好这些场景。归档/normalized/retained/queued 字节数相等，但这些队列计数不是云端最终收到证据。

进一步找到自动结束的另一条捷径：request_embedded_ble_recording_stop_from_host 对 2500ms 自动结束设置 auto_end_commit_preview_session；end_session 提交预览并调用 release_active_asr_without_sealed_final，发 final 后立即 cancel，跳过 await_final_result 和 await_target_speaker_final。会话 6395b329 云端中间结果已含“追溯原因”，过滤后的预览只到“可”，该捷径直接提交了被截断的预览。长旁人场景也没有执行 final owner-only 仲裁。准备移除此捷径，仍保持即时处理中反馈；随后实机核对真实 final frame 和额外耗时。

## 第二次修改：完整终稿与声纹结果交接

已移除自动结束专用的预览提交标记、绕过 ASR 的分支以及只发 final 随即 cancel 的函数。自动和手动都走现有 send_last_frame -> await_final_result + await_target_speaker_final 并发路径，再做最终文本仲裁。未调整唤醒/声纹阈值、未加入声学高通或音量实验。

源验证：新结构回归在旧分支 RED；修复后全量 Rust 1331 passed、31 ignored，cargo check 与 web build、brand/cloud/traceability 全过。Type 安装 SHA256 14D3D84E25B54DD56ACE023DE25E42EC4A385D4B8CD0B9C3328C083CB917804D；同一固件 cefb802eed6e36d1。当前实机复测进行中，固定答案来自前几轮保留的 oracle。

新增通用 work/audit-audio-handoff.py，将最终历史 UUID、trace UUID、非空归档/normalized/retained/queued 数量、真实 provider final 标记及报告时长对齐；缺终稿/字节缺口/时长不足/错会话的自检全部拒绝。9196F1FD 的 7 轮中只有手动对照具备完整交付证据，6 轮自动缺 provider final。此审计不替代最终文字验收。

## 14D3D84E 实机结论与下一处身份缺陷

七轮全部归档/normalized/retained/queued 相等，七轮都有真实 provider final 且报告时长覆盖全部音频；上一版仅手动一轮满足。最终正文仍是 2 通过、5 失败：正常长句、停顿长句完整；历史自动短句误识别，手动短句重复“你”，背景唤醒长句漏“今天的”，短/长旁人场景仍泄漏。无口令本人素材另观察 46.02 秒无正式启动/输出（本轮不覆盖 owner_matched=true 阴性分支）。末次出字到完成需保留逐轮数值，普通长句 4866 ms，其中 physical completion -> done 2185 ms 来自结束时目标说话人检查，不能宣称速度解决。

继续定位发现：start_local_speaker_tracking 收到 enrolled_owner_matched=false 时明确调用普通本轮跟踪，注释要求 adaptive session profile；但 LocalSessionSpeakerTracker::from_wake 未传此信息，session_profile_from_wake 只要存在登记就混入登记 bank。TSE 同样无条件优先保存的登记 embedding。于是一次按口令允许进入的会话，其 local 与 separator 仍可跟随另一个已登记人，与接受的本轮讲话人不一致。

第三次修改将实际 enrolled_owner_matched 传到两个会话身份准备函数。仅通过登记人校验时使用保存的 bank；否则从本次已接受的唤醒音频构造临时 profile/WeSep embedding。没有写入或删除登记模板，没有改变任何唤醒或相似度阈值。其收益与对低分本人语音的风险需实机复测，不能把 API 接线修正等同于 G 已解决。

## 身份实验撤回与研究转向（2026-09-13）

06F5BC71 候选九轮固定正例为 4 pass / 5 fail，全部九轮音频交付与终帧覆盖完整；46.05 秒无口令负例未正式开录，但未覆盖 enrolled-owner-match 无口令分支。会话 59e1f309-f136-47b8-9acf-6b0ca07cd0fa 的 provider raw 含本人正文，speaker_filtered_result 仅“开始录音。”，merged_candidate 却保留旁人尾句。实验违反不吞本人优先级，已从源码精确撤回，备份在 task work/session-identity-experimental-sources；保留两个 STOP 排空/真实终稿修复及其他原有修改。

重新构建安装 Type SHA 35307913B1E7EB1C7D368633445A10689EAD893966FB843E27072FF943018803，运行身份通过；新二进制不能沿用旧 14 候选实测数量。1331 Rust 通过、31 ignored，cargo check 与前端 build/brand/cloud/traceability 通过。新长句与停顿对照见 task outputs/identity-withdrawn-acceptance.json（执行完成后生成）。用户要求先研究成熟产品，调查见 mature-voice-systems-2026-09-13.md；当前未决定换模型，未调整阈值。

35307913 新安装复测：长句 215bacee-5037-491d-b813-71a3bf5e2978、停顿 ee4dba0a-76b9-4463-9b50-503e7036d2a6 两轮正文与交付均完整，末次预览到完成 6696/4057 ms；没有消除等待波动。日常流式输入/回退恢复，调试录音关闭，去语气词保留开启；正常重启运行身份 PASS。
