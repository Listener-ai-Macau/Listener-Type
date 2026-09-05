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

可见录音只有一个结束裁判：主人活动时钟。身份边界与证据边界是两套正交 reducer，
但只有前者有权提交不可逆 STOP：

- 产品身份：`Idle -> WakeCandidate -> Active -> Stopping -> Closed`；
- endpoint 证据：`OwnerActive -> OwnerEvidencePending -> QuietPending -> Stop proposal`。

- 只有正向主人声纹/主人归属边沿可以推进 `OwnerActive` 的时钟。
- `OwnerEvidencePending` 表示已经采集的 PCM/预览仍在等待同一套主人连续性裁决；
  它不是主人活动，不能自行刷新 watermark。确认主人后回到 `OwnerActive`，确认
  他人或到达有界裁决期限后进入 `QuietPending`。
- 普通能量、灯光变化、旁人语音、云端 pending、预览增长不能推进主人时钟。
- `QuietPending` 只允许有限的 provider catch-up 窗口；窗口到期后必须停止，不能无限 Hold。
- 所有可见录音停止必须由产品身份 reducer 按 coordinator session ID 提交；endpoint
  只能提出 stop proposal。BLE STOP 成功后才能把前台切到 Processing 并关闭 ASR；
  写入失败必须保持 Listening、恢复 Active 并允许同一 endpoint 重试。

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

- 主机旧 endpoint helper 曾把 pending 预览修订当成主人尾音，导致 `arbiter_hold` 永久化；已由回归测试锁定。
- `diag_log` 环形区仍能读到连续 `boot_watchdog` / `reset_reason=5`，但
  2026-09-04 的最新 20 秒不触发复位串口现场从约 61,136,000 ms 连续到
  61,173,000 ms，未出现新的 WDT、I2S stall、ringbuffer full 或传输丢包。
  因此这些 boot segment 目前只能标为历史证据；再次出现问题时必须立即抓
  现场并按同一时间基线确认，不能把环形区旧记录当成新复位。
- 固件动态电平器确实在运行；它的衰减统计必须作为唤醒 A/B 输入证据，不能直接拿灯光或 raw level 推断声纹结果。

## 本轮落地

- `OwnerEndpointController` 是唯一 endpoint 证据类型，只公开
  `OwnerActive / OwnerEvidencePending / QuietPending`；它没有 `Stopping`，避免
  endpoint 与产品身份各自保存一个不可逆停止态。
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

当前不再保留独立 `WakeCandidateController` 或候选原子变量。候选阶段、设备段
ID、按键接管意图和产品 owner session 全部由 `Inner.recording_lifecycle` 中的
`RecordingLifecycleController` 原子变更。actor 内 `speaker_candidate` 只保存 PCM
和模型证据，不能改变产品生命周期。KWS、短语确认和声纹推理仍是候选的证据
提供者，不得各自改变可见录音或设备 STOP。

拒绝候选后的延迟物理 STOP 必须验证“原候选 Closed tombstone 仍拥有 transport
stop”，不能只检查“当前没有新候选”：这段延迟内若已经进入主人录音，旧 STOP
必须失效。终端唤醒跨物理段时保留同一个候选身份，下一段只能执行
`promote_candidate_to_owner`，不得伪装成 `begin_manual_owner`；六秒内没有收到
正文 transport 时按 candidate + coordinator 双身份清理 Starting 会话。

保留的本地确认、声纹预取和终端离线确认是同一候选控制器内的有界证据
级联，不是独立生命周期；每一条路径都必须最终调用同一个候选升级或拒绝
出口。任何新的唤醒延迟日志都要按“PCM 到达、候选窗口、推理耗时、声纹
结果、BLE 控制”五段定位，不能再通过全局阈值微调掩盖结构问题。

## 产品生命周期控制器（2026-09-04）

嵌入式录音现在由 `Inner.recording_lifecycle` 持有唯一的
`RecordingLifecycleController`。它跨 BLE actor、ASR 回调和 endpoint watchdog
共享同一个产品身份：

`Idle -> WakeCandidate -> Active -> Stopping -> Closed`

候选升级、停止提交、停止失败重开和取消/完成清理都必须通过该控制器。
主人活动、待裁决声纹和静音只进入 `OwnerEndpointController`，不能复制进产品
身份状态。endpoint clock 只计算“是否到期”的证据，不能绕过控制器直接把录音
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
  `QuietPending -> Stop proposal`，再由产品生命周期提交 `Stopping`，不允许回调层另设旁路。

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

session `777911d9-1698-4cc8-bac2-42243c0fb1ce` 还暴露了胶囊可见性的时序
漏洞：本地短语先显示了早期 Recording 胶囊，主人门稍后接受时才安装
automatic wake guard。此时前端已可见，不会再发第二次 visible ACK，旧 guard
因此永久停在 `automatic_body_initial_wait`；日志即使已记录正文开始，endpoint
仍被这个无界等待拦截。现在 accepted-session guard 必须继承同 session 的早期
胶囊可见证据；即使 ACK 丢失，guard 也从安装时开始三秒墙钟上限，不再
存在永久 Hold。

真实会话 `0abcb532-2ebe-484f-ae49-0343e0ee45bc`（设备段 2898）进一步证明，
有界 guard 本身还不等于有界 endpoint：主人唤醒已经通过、早期胶囊已经显示，
但唤醒前物理 BLE 段随后轮换停止，且没有正文、provider preview 或新的物理段。
旧 endpoint 只会被正文或 speaker callback 启动，因此日志只记录一次
`automatic_body_initial_wait`，三秒 guard 到期后仍没有可供唯一裁决器提交的
endpoint candidate，最终拖到 Volcengine 八秒传输超时。

现在无正文是 `OwnerEndpointController` 的明确会话模式，而不是另一个超时旁路：

- `TargetSpeakerEndpointPolicy` 从 automatic guard 一次性读取 `active + started_at`；
- watchdog 无条件把当前 ASR 快照送入同一个 reducer，不能再依赖 preview 或
  diarization callback 才创建 endpoint；
- no-body candidate 使用 guard 的原始 `started_at`，禁止在三秒到期时重新计时；
- 唤醒短语尾音、普通能量、provider pending 和旧的声纹计算都不能延长无正文
  会话；它们不是正文；
- 一旦第一段正文到达，no-body candidate 原子退出，并由新的主人正文 watermark
  重新启动普通 endpoint，旧三秒截止不能截断正文；
- 测试中的 deadline helper 只封装生产 `is_due`，不再保留第二份 endpoint 判定。

同一录音在首轮修复后的自动重放又暴露了物理层反向控制产品生命周期的问题：
endpoint 已在三秒提交 STOP，固件 STOP 写入也成功，但 BLE actor 仍保留
`activation_segment_race_guard` 并等待一个不存在的下一物理段。42 秒后的新唤醒段
2900 被绑定给旧产品 session，旧胶囊直到该段 STOP 才回到 Idle。这解释了“上一轮
看似结束后，下一次间歇性唤醒不了”。

新的边界使用 actor 内单次 hand-off 表达“正在等待可选的 post-activation 段”：

- 旧段在竞态窗口内 STOP 时，actor 记录对应的产品 session；
- 真正的新物理段先到时会清除此 hand-off，并继续正常正文捕获；
- no-body endpoint 完成固件 STOP 与 provider final-frame 后，只有仍持有 hand-off
  才能直接完成产品 session；
- 产品 session 到达 Idle 后，后台 actor 只释放本地传输壳，不再次取消 ASR、发布
  错误或等待物理 STOP；
- hand-off 只能被消费一次，未来设备段必须进入新的 `WakeCandidate`，不得附着到
  已结束的 session。

因此物理 BLE 段现在只决定 PCM 的接入与排空，不能再阻塞已经由唯一 endpoint
裁决为结束的逻辑会话。

### 主人连续性跨传输重置（2026-09-04，session 2595）

现场会话 `df92eee8-09c5-463e-a48c-34df98e2b2a2` 在 1.9 秒音频处已经由本地
声纹明确确认主人；Volcengine WebSocket 随后完成建链并执行内部 stream reset。
旧 reset 只保留 `local_wake_owner_verified/local_speaker_stable_target` 等布尔标记，
却清空 `local_target_confirmed/local_target_speech_end_ms` 和声纹证据序列。结果是
后续窗口仍记录 `stable_target=true`、语音和预览仍在增长，结束器看到的主人
watermark 却为 `None`，最终在 STOP 后仍到达新语音和新文字，构成确定的中途截断。

现在本地身份状态只有一个 `OwnerContinuitySnapshot`。它同时用于 provider 建链
reset 和 retained-audio 恢复重放，原子保存本地音频/语音/主人/他人 watermark、
当前分类、去抖计数、主人缺席状态和证据序列。只有开始新的产品会话才允许建立
新的连续性；网络建链、重连和恢复重放不得把其中一部分恢复成默认值。

同时，新的声纹/归属 observation 只进入唯一 endpoint reducer；
`RecordingLifecycleController` 不再复制 `OwnerActive/QuietPending`，只验证候选和
owner session 身份。停止 handler 只负责按相同 session ID 幂等提交 STOP，不再
兼任主人活动和固件续租处理。

### STOP 两阶段提交与过期回调隔离（2026-09-05）

最后一轮结构审计确认了三个会制造间歇性故障的残留：旧 BLE actor 能用
`close(None)` 关闭当前任意会话；endpoint 自己先进入 `StopCommitted`，但产品
生命周期可能拒绝同一次停止；BLE STOP 尚未成功时前台已进入 Processing，ASR
也已并发收到 final frame。它们分别会误杀下一次唤醒、造成永不结束，以及在
瞬时 BLE 写失败后留下“有声音但无预览”的半关闭会话。

当前硬约束是：

1. actor 只能 `close_candidate(embedded_session_id)` 或
   `close_owner(coordinator_session_id)`，不存在无身份 reset/close；
2. endpoint 的 Stop 是可重复 proposal，只有 `RecordingLifecycleController` 能把
   exact owner 从 Active 提交到 Stopping；
3. `request_embedded_ble_recording_stop_from_host` 是唯一 owner STOP 事务入口，在
   同一调用内完成 exact session 提交、物理 BLE STOP 和失败回滚；endpoint、设备键、
   provider 故障兜底与胶囊停止都不能预提交或绕过它；
4. BLE STOP 写成功后才发布 Transcribing，再发送 provider final frame；失败时
   lifecycle 回到 Active，前台和 ASR 始终保持 Listening；
5. candidate capsule 使用独立 UI token，绝不创建 `SessionState::Starting`，候选
   拒绝也绝不直接写 `SessionPhase::Idle`；
6. 生产路径不提供 lifecycle `reset()`，只能用带身份的 close 留下 stale callback
   tombstone。

### 预览单一 reducer（2026-09-05）

旧实现把 authoritative partial 与 capsule-only provisional text 放在两个独立 Mutex，
普通 partial 和 final supplement 又各自复制了一套“锁定、改写、发布”流程。actor
dispatch 只负责串行记录事件，并不验证 session identity，因此旧 provider callback
可以先改写共享槽，再由前台状态机拒绝显示；下一会话仍可能读到这次越权写入。

当前只保留一个 session-scoped `RecordingPreviewController`：

- provider partial、two-pass supplement 和 provisional diarization tail 都必须向同一
  reducer 提交带 coordinator session ID 的 evidence；
- authoritative 与 visible 仍是不同语义，但在同一把锁中原子变化，provisional
  text 永远不能进入 endpoint、恢复或最终插入；
- provisional 已显示、随后 authoritative 得到相同文字时只升级权威，不重复发布；
- final supplement 可以收回未确认 provisional tail；旧 session callback 不能改写
  新 session；
- 已删除不在生产路径中的旧 preview stabilization/stitching 算法和对应“自证测试”。

### BLE 连接状态与录音会话边界（2026-09-05）

Windows `BluetoothLEDevice.ConnectionStatusChanged(Disconnected)` 是异步状态通知，
可能在同一物理链路已经恢复、GATT 仍为 Active、且音频通知仍在到达之后才投递。它
不能单独作为录音会话的断链裁决，否则一次状态抖动会启动 5 秒恢复超时并拆掉正常
notify，表现为一段时间完全无法唤醒。

- 活动录音中，只有“GATT 非 Active 且没有继续收包”或 heartbeat/通知 watchdog
  超时，才可以进入 link recovery；单个延迟的设备 Disconnected 回调只能记录为
  advisory；
- 空闲 notify 目标没有可恢复录音时，设备断开仍立即进入普通恢复路径；
- 连接恢复不得改变当前逻辑 candidate/session identity，也不得清理已提交的主人
  连续性或预览；恢复失败后只重建物理 notify 壳；
- BLE recovery 与录音 endpoint 完全正交：它不能推进主人时钟、提交 STOP 或改写
  final transcript。

### 最终文本只有一个原子仲裁器（2026-09-04，session 2744）

现场中文干扰会话 `a86a4b77-c764-404e-8121-6a7f6ba6091e` 中，Provider 已经把
主人 44 字和 speaker 1 的 7 字尾巴分成两个稳定行。本地声纹在同一尾段连续给出
transcript-grade 极低分证据，但旧代码把它降级成 advisory `Uncertain` 后，另一个
“防吞字”分支又按锁存的 `stable_target=true` 把 Provider 51 字原文全部恢复，最终
漏入“会持续一段时”。这不是 endpoint 阈值错误，而是多个 final 恢复分支互相绕过。

现在 Provider 原文、主人过滤结果和 optimistic 文本不能各自决定提交。
`speech_decision_kernel::arbitrate_final_transcript` 是唯一的协议终帧文本裁判，并且
一次只读取一个不可变的 owner-continuity 快照。它按固定优先级选择一个 authority：

1. Provider 稳定外来 speaker 与本地连续极低声纹证据时间重合时，必须选择
   `speaker_filtered`；这项证据只否决尾巴恢复，不反向删除已确认主人正文。
2. 没有明确旁人证据时，才允许 `provider_raw_recovery` 或
   `provider_owner_recovery` 防止云端归属回退造成吞字。
3. 防吞字的 session ledger 只是 `session_ledger_recovery` 候选，必须在相同证据
   快照内通过仲裁，不能在仲裁之后按“最长文本”覆盖结果。
4. `optimistic_owner_recovery` 只能作为更低优先级的已验证主人恢复；胶囊视觉预览
   本身永远不是最终文本 authority。

Provider adapter 只负责产生上述证据，不得再添加独立的 `prefer_final_*` 选择器。
每次终帧必须记录唯一 `authority` 和所有候选安全事实，便于以后直接从日志复现决定。

后续现场会话 `a12c722f-858f-4198-a4e6-39fd08c2254e` 暴露了两个更隐蔽的遗留点：
本地旁人尾段被切成 500 ms 与 400 ms 两个高能量低分窗口，旧的单窗 600 ms 布尔门
把二者都丢成无证据；即使仲裁器选中 speaker-filtered，通用流式合并器仍在之后用
最长 session ledger 把旁人文本重新补回。新契约将声纹证据改为
`Inconclusive / ForeignTailHint / HardNonTarget`，两个连续 `ForeignTailHint` 只能与云端
稳定的不同 speaker 行交叉确认，不能单独删除同 speaker 主人文字，也不能改变 endpoint。
协议终帧经唯一仲裁后成为 sealed transcript；其后只允许标点、空格和重复尾巴规范化，
不允许流式 merge、ledger fallback 或 provider fallback 再增加内容。

### 产品终稿边界也只有一个裁决者（2026-09-04）

Provider 内部封口并不等于产品终稿已经统一。继续审计发现 coordinator 在收到
sealed provider final 后，仍会依次执行四条可写路径：owner-only 分离结果直接替换、
空结果 retained-audio replay 直接替换、胶囊 partial preview 直接恢复、local shadow
直接补字；最后 preview hotword 又单独改写一次。每条路径单独看都有用途，但串联后
后执行的“防吞字”恢复可以绕过前面的干扰过滤，这正是四种体验反复回归的外层根因。

现在停止流程必须先收集不可变的 `ProductFinalCandidates`，再且仅再调用一次
`arbitrate_product_final_transcript`。产品级优先级由
`speech_decision_kernel::arbitrate_product_final` 固定：

1. 有可用的 owner-only 分离结果时选择 `separated_owner`；
2. 否则选择已经通过 Provider 协议终帧仲裁并封口的 `provider_primary`；
3. 明确要求干扰过滤且上述两项均为空时返回空，不得用 replay、preview 或 shadow
   恢复未经身份验证的文字；
4. 无干扰的空终稿才依次允许 `retained_audio_replay`、debug override 和
   `partial_preview_recovery`；
5. local shadow 不是独立 authority，只能在无干扰、主人结束时钟对齐且基础候选来自
   Provider/replay 时，执行严格有界的 omission repair；
6. preview hotword 与 filler 删除在一次裁决函数内部完成，随后记录唯一
   `product final sealed authority=...`。之后 correction rule/LLM 只属于用户显式文本
   后处理，不能重新读取 ASR、预览或声纹证据恢复录音内容。

结构回归会拒绝旧 `select_target_speaker_final`、`raw = replayed`、
`raw.text = recovered` 和 partial-preview 直接恢复语句重新出现。以后增加任何模型或
fallback，都只能增加 `ProductFinalCandidates` 的证据字段，禁止新增终稿写出口。

### 干扰基线必须绑定唤醒候选（2026-09-05）

旧实现把唤醒干扰基线放在进程级 `OnceLock<Mutex<...>>` 中。每个物理窗口结束时
仍会把主人分数写入同一份状态，因此上一段录音的房间/旁人分数会改变下一段候选
是否请求分离验证；这会表现为同一句话有时灵敏、有时完全不唤醒。

现在 `WakeInterferenceBaseline` 是 `BufferedSpeakerCandidate` 的字段，只能由当前
候选携带和销毁；源码结构测试禁止重新引入进程级基线或旁路 helper。它仍然只是
分离验证的证据触发器，不能直接激活、拒绝、停止录音，也不能修改终稿。后续若要
扩展基线采样，必须继续写入候选 reducer，不能恢复跨 session 的隐式状态。

候选可能在第二个声纹窗口完成前就进入终端裁决，因此首个达到有界主人分数下限的
样本可以请求一次分离验证；这不等于接受唤醒，分离音轨仍必须独立通过短语和主人
声纹校验。这样既不会因候选级状态而关闭强干扰恢复，也不会重新引入跨录音污染。

### 终端与实时唤醒共用同一个候选仲裁入口（2026-09-05）

唤醒候选有两个时间边界：滚动窗口中的实时释放，以及设备 STOP 到达后的终端
收尾。两条路径可以并行产生 KWS、本地短语和声纹证据，但不能各自实现一套
“主人是否允许通过”的布尔分支；否则同一候选会因收尾时序不同而出现间歇性唤醒。

现在两条路径都必须调用 `arbitrate_candidate_wake`，由它一次性完成 owner gate
和 `speech_decision_kernel::arbitrate_wake`。声纹分离只作为 owner evidence，不能
制造 phrase evidence；`terminal` 只描述生命周期边界，不改变证据优先级。结构测试
会拒绝 coordinator 直接调用底层 `arbitrate_wake`，避免未来再长出第三条旁路。

### 隐藏候选停止也走同一所有权调度器（2026-09-05）

隐藏候选被拒绝、或唤醒后续接管在 TTL 内未建立时，都需要让固件结束当前物理
窗口。这两个安全分支以前各自延迟、检查 session、发送 `VREC:STOP`，容易在新
候选已经开始时把旧 STOP 发出去。

现在二者统一调用 `dispatch_owned_candidate_transport_stop`：发送前只接受仍由
该 candidate 持有的 transport ownership，发送动作和 stale 检查在同一个调度器内
完成。正常主人录音仍由 `request_embedded_ble_recording_stop_from_host` 的生命周期
事务负责；隐藏候选不能伪造产品 session，也不能直接改写终稿。

### 已确认主人后的 Uncertain 不能伪装成静音（2026-09-05，session 1226）

声纹连续窗口不是每次都能给出 Target：低音量音节、重叠边界或短暂的模型抖动会
落在 `Uncertain`，但这并不等价于“主人停止”。旧 endpoint 策略只检查“本地主人
水位是否领先云端”，当两者相等时会把同一段 Uncertain 尾音当作静音，触发
`inactive_1000ms`，造成说话中途截断。

统一状态机现在把“已建立主人 + 未出现明确 NonTarget + 最近仍有本地语音边缘”归为
有界 `UncertainOwnerTail` hold。它只延迟当前端点，不推进主人水位、不提交文本；
明确 NonTarget 立即解除 hold，尾音超过两秒上限也自动回到正常一秒静音端点。因此
这条路径不会把房间噪声永久续命，也不会再把一次低分窗口当成停止证据。
