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
- 固件最新现场出现连续 `boot_watchdog` / `reset_reason=5`，说明 interrupt WDT 仍需独立抓现场定位，不能用历史日志或短 soak 代替。
- 固件动态电平器确实在运行；它的衰减统计必须作为唤醒 A/B 输入证据，不能直接拿灯光或 raw level 推断声纹结果。
