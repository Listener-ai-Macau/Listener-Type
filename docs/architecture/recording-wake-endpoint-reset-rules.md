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
- 候选失败必须一次性销毁，不得把候选的 PCM、预览、声纹结果或计时器带入下一候选。
- 唤醒延迟问题必须区分：音频未到、候选窗口未满、模型推理慢、声纹不匹配、BLE 激活慢；禁止统一降低所有阈值。

### 内容层（主机）

- 云端文字、预览修订和 pending 状态只负责内容合并。
- pending 只有在仍存在“未归属的主人尾音”时才可以阻止结束；pending 或预览修订本身不能续租录音。

### 结束层（主机）

可见录音只有一个结束裁判：主人活动时钟。状态必须单向经过：

`WakeCandidate -> OwnerActive -> QuietPending -> Stopping -> Closed`

- 只有正向主人声纹/主人归属边沿可以推进 `OwnerActive` 的时钟。
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

`Idle -> WakeCandidate -> OwnerActive -> QuietPending -> Stopping -> Closed`

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
