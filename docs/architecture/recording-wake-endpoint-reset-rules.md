# Listener 录音、唤醒、自动结束与重启修复规则

## 目的

唤醒、自动结束和固件重启必须按真实证据修复。禁止在旧状态机上通过继续叠加阈值、延长看门狗或复制新的兜底分支来“碰运气”。

## 唯一职责边界

### 音频输入层（固件）

- AFE、VAD、AGC 和动态电平器只负责产生 PCM 和诊断遥测。
- 麦克风灯亮度只表示输入/输出能量，不表示主人声纹，也不表示应该结束录音。
- 动态电平器可以为安全目的衰减削波或过度放大的噪声，但不能成为录音结束条件。
- 固件保留硬件安全上限和隐藏唤醒候选的短时上限；不得用普通 VAD 静音作为可见主人录音的最终结束裁判。

### 唤醒层（主机）

- Wake KWS、短语确认和主人声纹确认只决定 `WakeCandidate` 是否升级为可见录音。
- 固件 `SessionStart/SessionStop` 是物理传输窗口，不等同于产品级
  `WakeCandidate`。固件必须在 STOP 中报告 `silence` 或 `max_duration`：静音
  结束销毁逻辑候选；最大时长只轮换有重叠预录音的物理窗口，不得误清空仍在
  计算的逻辑候选，也不得让旧窗口的异步结果控制新窗口。
- 候选真正失败时必须一次性销毁，不得把 PCM、预览、声纹结果或计时器带入
  下一逻辑候选；窗口轮换只允许保留有界 KWS/主人变化摘要，禁止重复拼接重叠
  PCM。
- 唤醒延迟问题必须区分：音频未到、候选窗口未满、模型推理慢、声纹不匹配、BLE 激活慢；禁止统一降低所有阈值。
- 强干扰恢复不得以“原始 KWS 或混音本地 ASR 已经命中”为唯一入口，否则最
  需要分离时恢复路径反而不可达。轻量路径全部未命中时，控制器必须能依据
  “已登记主人相对当前干扰底发生变化”的有界证据请求一次分离；分离后的音轨
  仍须独立通过短语和主人校验。

### 内容层（主机）

- 云端文字、预览修订和 pending 状态只负责内容合并。
- pending 只有在仍存在“未归属的主人尾音”时才可以阻止结束；pending 或预览修订本身不能续租录音。
- 预览准入和 endpoint 续租必须读取同一个 `OwnerContinuity` 结果。禁止出现
  “同一帧被内容层认作主人可显示、却被结束层认作 Quiet”的分裂判定。

### 结束层（主机）

可见录音只有一个结束裁判：主人活动时钟。状态必须单向经过：

`WakeCandidate -> OwnerActive -> OwnerEvidencePending -> QuietPending -> Stopping -> Closed`

- 只有正向主人声纹/主人归属边沿可以推进 `OwnerActive` 的时钟。
- `OwnerEvidencePending` 表示已经采集的 PCM/预览仍在等待同一套主人连续性裁决；
  它不是主人活动，不能自行刷新 watermark。确认主人后回到 `OwnerActive`，确认
  他人或到达有界裁决期限后进入 `QuietPending`。
- 普通能量、灯光变化、旁人语音、云端 pending、预览增长不能推进主人时钟。
- `QuietPending` 只允许有限的 provider catch-up 窗口；窗口到期后必须停止，不能无限 Hold。
- 所有停止请求必须经过同一个幂等出口；失败只能重新进入当前状态，不能创建第二套计时器。

## 重启调查规则

1. 每次先读取最新 `diag_log` 和不触发复位的串口现场。
2. 先按 `reset_reason` 分型：interrupt WDT、task WDT、USB/外部复位、异常或低功耗唤醒。
3. 只在对应触发路径上改代码；看门狗阈值保持产品值（task WDT 5 秒、interrupt WDT 300 ms）。
4. 任何 audio ringbuffer、I2S、BLE backpressure 或任务饥饿的结论都必须有对应日志/计数器支持。
5. 刷机后必须连续观察超过单次故障周期，并记录新旧 boot segment；一次 35 秒无故障不能宣称修复。

## 每次改动的验收门槛

- Type：`cargo test target_speaker_endpoint --lib`、`cargo test pending_ --lib`。
- Firmware：必要的静态 verifier；涉及固件代码时重新 build/flash。
- 运行时：`C:\Program Files\Listener Type\listener-type.exe` 必须与当前 release hash 一致，桌面/托盘不得运行旧副本。
- 日志：必须能回答“谁推进了主人时钟”“谁触发了停止”“是否有新 WDT”，否则不接受修复。
- 音频诊断：每个物理窗口必须持久记录 AFE 输入、AGC 输出、最终 BLE PCM 的
  会话级电平摘要以及动态电平器的 noise floor/allowed gain；LED 电平不得作为
  模型输入质量的替代证据。

## 架构完成定义（2026-09-04）

只有以下条件同时成立才允许宣称“统一架构完成”：

1. `RecordingLifecycleController` 是唯一产品生命周期，物理 BLE 窗口不能重置或
   越权结束逻辑生命周期。
2. 唤醒、预览、声纹过滤和 endpoint 共享同一个 `OwnerContinuity` 裁决；生产
   路径中不存在第二套“主人仍在说话”的布尔条件。
3. 原始 KWS、混音本地 ASR、声纹和分离模型只是证据生产者，全部只能通过统一
   reducer 升级/拒绝候选，不能直接显示、停止或激活。
   `begin_manual_owner` 与 `promote_candidate_to_owner` 是互斥入口：隐藏候选不能
   伪装成手动录音绕过 reducer，手动录音也不能继承候选证据。
4. 所有异步任务携带逻辑 candidate/session identity；过期结果必须被拒绝。
5. STOP 原因和三点 PCM 质量可以从持久日志还原；没有这份证据不能把间歇性
   故障归因于模型、增益、BLE 或状态机。
6. 规定测试、固件静态验证、build/flash、1.0.5 最新运行时 hash 校验和真实
   干扰验收全部通过。只完成其中一部分必须明确标记为“未完成”。

## 当前已确认的问题

- 主机 EndpointArbiter 曾把 pending 预览修订当成主人尾音，导致 `arbiter_hold` 永久化；已由回归测试锁定。
- `diag_log` 环形区仍能读到连续 `boot_watchdog` / `reset_reason=5`，但
  2026-09-04 的最新 20 秒不触发复位串口现场从约 61,136,000 ms 连续到
  61,173,000 ms，未出现新的 WDT、I2S stall、ringbuffer full 或传输丢包。
  因此这些 boot segment 目前只能标为历史证据；再次出现问题时必须立即抓
  现场并按同一时间基线确认，不能把环形区旧记录当成新复位。
- 固件动态电平器确实在运行；它的衰减统计必须作为唤醒 A/B 输入证据，不能直接拿灯光或 raw level 推断声纹结果。

## 本轮落地

- `OwnerEndpointController` 是 `EndpointArbiter` 的唯一语义入口，公开生命周期
  `OwnerActive -> QuietPending -> Stopping`；旧类型名只保留为兼容别名，避免
  其他适配器偷偷创建第二套计时器。
- 所有 endpoint hold/stop 日志都会记录 `lifecycle` 和具体 `reason`，可回答谁
  推进主人时钟、谁触发停止，以及是否只是 provider stall。
- 健康 ASR 路径不再使用原始能量 trailing-silence 作为第二个停止裁判；仅在
  ASR 投递失败时保留有界安全回退。固件的 `VREC:SPEECH` 仍是独立安全租约，
  不改变可见录音的主人活动权威。

## 唤醒候选的旧结构清理（2026-09-04）

唤醒候选过去由 actor 内的 `speaker_candidate`、全局隐藏候选原子状态、
独立的隐藏 session 原子以及终端确认级联共同维护。它们会在候选轮换或
设备按键接管时产生跨段竞态：旧候选的延迟 STOP 可能截断新候选，旧的
promotion 标志也可能被新候选继承。

当前统一为 `WakeCandidateController`：阶段、设备段 ID 和接管意图在同一
个串行控制器内变更；新设备段注册时先清空上一候选阶段，再允许进入
`ACTIVE`。KWS、短语确认和声纹推理仍是候选的证据提供者，不得各自改变
可见录音或设备 STOP。旧的 endpoint policy 辅助函数仅在测试编译，正式
程序不存在第二个 endpoint 裁判。

保留的本地确认、声纹预取和终端离线确认是同一候选控制器内的有界证据
级联，不是独立生命周期；每一条路径都必须最终调用同一个候选升级或拒绝
出口。任何新的唤醒延迟日志都要按“PCM 到达、候选窗口、推理耗时、声纹
结果、BLE 控制”五段定位，不能再通过全局阈值微调掩盖结构问题。

## 产品生命周期控制器（2026-09-04）

嵌入式录音现在由 `Inner.recording_lifecycle` 持有唯一的
`RecordingLifecycleController`。它跨 BLE actor、ASR 回调和 endpoint watchdog
共享同一个状态与 session 身份：

`Idle -> WakeCandidate -> OwnerActive -> OwnerEvidencePending -> QuietPending -> Stopping -> Closed`

候选升级、主人活动、停止提交、停止失败重开和取消/完成清理都必须通过该
控制器。endpoint clock 只计算“是否到期”的证据，不能绕过控制器直接把录音
标成停止；重复 callback/watchdog STOP 会被 session ID + 幂等转换拒绝。
固件 VAD、灯光、云端 pending 和预览仍然只是证据，不会创建第二个生命周期。

这次改动解决的是状态所有权和竞态根因，不是把超时继续调小。模型命中率、
BLE 延迟和音频质量仍需分别从诊断日志验证；若日志显示模型未命中，不能把它
误报成状态机已经修复。

## 声纹计算与结束顺序（2026-09-04）

真实会话 `c5f6bdc7-e8f2-4649-9349-b5809b201608` 证明旧顺序存在竞态：
端点在 5.0 秒音频处提交停止，而已经采集到的 5.2 秒和 5.9 秒本地声纹结果
随后才返回。旧实现把“本地声纹任务还在计算”误当成“没有主人活动”，因此
会在用户讲话中途提前结束。

修复后的约束如下：

- 本地声纹层只发布 `analysis pending/completed` 证据，不拥有任何停止计时器。
- `OwnerEndpointController` 是唯一保存声纹等待期限并提交停止的组件。
- 已经开始的单飞声纹任务未完成时，控制器保持 `QuietPending`，不得提交停止。
- 声纹计算本身不算主人活动，不能刷新主人 watermark，也不能重新开始普通
  静音倒计时。
- 连续声纹任务共享原始等待期限；达到有界故障期限后必须放行普通端点判断，
  避免模型或线程池异常造成永久录音。
- 声纹结果返回后，主人证据走 `OwnerActive`，他人/静音证据继续走
  `QuietPending -> Stopping`，不允许回调层另设旁路。

## Endpoint 会话策略快照（2026-09-04，session 1896）

真实会话 `01d999ab-bbd3-4e47-895b-5f0159d09c06` 暴露了此前“统一状态机”仍未
统一策略输入的问题：自动唤醒正文尚未开始时，callback/watchdog 用普通正文的
900 ms 墙钟提交停止；停止处理层随后重新读取 wake guard，并把同一决定记录成
`target_speaker_inactive_no_body_3000ms`。因此日志看似执行了 3 秒规则，实际胶囊
显示 1,945 ms 后已经进入 Transcribing；停止后到达的 2/4 字预览又被当成迟到
结果，最终 13 字被整段唤醒前缀规则清空。

修复后的硬约束：

- `TargetSpeakerEndpointPolicy` 是一次解析的不可变会话策略快照，包含
  `body_started`、正文初始等待、产品 endpoint timeout、调度墙钟 timeout 和
  stop reason。
- provider callback、endpoint watchdog 和 stop dispatch 必须消费同一份快照；
  stop dispatch 禁止再次读取 preview/wake guard 推导另一套 timeout 或 reason。
- 胶囊可见后的初始正文等待现在进入生产决策路径，不再只是测试辅助函数；它
  同时受音频进度和 3 秒墙钟约束，provider 覆盖停止时也不能永久 Hold。
- 回归测试必须静态确认 callback/watchdog 不再直接调用 preview timeout helper，
  并复现“无正文会话不能按 900 ms 提交、日志理由必须与真实决定一致”。

同一批日志还确认了第二个旧旁路：固件 `VoiceActivation` 实际只表示 VAD 打开了
隐藏 PCM 传输窗口，不表示唤醒短语已经命中。旧代码在 host KWS/本地短语均未
命中时，只要主人声纹通过，就把这项 VAD 证据伪装成 `KeywordModel` 后放行。
session 1930 因此在候选结束后才以 `host_phrase_detectors=none` 被接受：它同时
制造误唤醒和约 5 秒的慢唤醒。该旁路已删除。声纹预取仍可并行降低延迟，但
只能与真实 KWS、本地短语近似或分离后短语证据一起交给 wake reducer，不能
单独构造 phrase evidence。

### 四种循环症状的架构归因

| 用户症状 | 真实架构冲突 | 统一后的权威 |
| --- | --- | --- |
| 唤醒不了 | KWS/本地短语没有产生真实 phrase evidence | wake reducer 只接收可标注的短语证据；模型召回率单独 A/B |
| 误唤醒或终端慢唤醒 | 固件 VAD 和主人声纹被伪装成 `KeywordModel` | VAD 只开 PCM 窗口，不再制造短语命中 |
| 说话中途提前结束、吞字 | callback/watchdog 按短时钟提交，stop handler 又按长时钟重算并记录 | 一次解析的 `TargetSpeakerEndpointPolicy` 贯穿判定和停止 |
| 不自动结束 | pending/provider stall/异步声纹任务曾各自续租 | `OwnerEndpointController` 是唯一结束裁判，所有 Hold 有有界退出 |

这四种症状之所以会交替出现，不是四个超时值恰好都不对，而是旧代码
允许“证据生产者”自己做产品级状态转移。以后改模型、增益或供应商时，
只能改变证据的质量和到达时间，不允许新增第二个唤醒或停止出口。
