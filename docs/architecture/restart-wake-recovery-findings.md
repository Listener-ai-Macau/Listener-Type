# 重启后唤醒不可用缺陷记录

记录日期：2026-09-05  
影响版本：Listener Type 1.0.5 / 固件 1.0.5（build `798383b80eee997a`）

## 现象

设备或 Type 重启后，麦克风仍有输入，但唤醒会变成“偶尔无反应”。重启后的日志出现大量 `actor_restart`，新候选没有进入统一唤醒状态机。

## 已确认的日志证据

在 2026-09-05 12:28:58Z 附近，BLE 音频通知重复携带 `embedded_session_id=3`：

- `SessionStart` 与同一 session 的 PCM/重放包重复到达；
- `packet_sequence=3` 被当成 notify reopen 后的 orphan PCM，启动隐藏 VoiceActivation 恢复；
- 生命周期随后拒绝同一 session 的新候选：`录音生命周期拒绝新的唤醒候选`；
- Type 记录 `discarded background stream session after error while keeping notify open`，紧接着又 `actor_restart`，形成恢复风暴；
- 同期 `VREC:STOP` 写入反复出现 `GattWriteOption(0) ... protocol_error=14`，切换 option 1 后仍持续重试，队列等待时间从几十毫秒升到超过 1 秒。

这组证据说明：当前首要根因是“重启/notify reopen 后 BLE 会话代际和停止命令没有一次性收敛”，导致主机协调器被旧音频段与停止重试占满；不是先验地归因于声纹模型。`session_id=3` 本身可以是固件重启后的合法新计数，问题在于旧通知/控制写入与新连接没有被代际隔离。

## 修复验收要求

1. 每次 BLE 连接/notify 代际只允许一个活动音频 session；旧代际的 `SessionStart`、PCM、STOP/ERROR 必须被丢弃或一次性收敛，不能触发无限 `actor_restart`。
2. `VREC:STOP` 在同一候选上必须幂等且有界；`protocol_error=14` 不能形成无界重试/队列堆积。
3. stream error 后必须回到 `TYPE:READY`，随后新的唤醒候选能在同一 notify 连接上建立；不得要求用户手动重新配对或重启 Type。
4. 验证重启后连续 5 次唤醒：无新的 `interrupt_wdt`/`task_wdt`、无 AFE ringbuffer full/I2S stall、无 actor restart storm，且每次都有 capsule 与自动结束。

## 当前状态

该缺陷已定位到 BLE 会话恢复/控制写入边界，尚未宣称修复完成。后续代码修改必须先覆盖上述代际隔离、幂等 STOP 和恢复收敛，再运行规定的 Rust 测试、固件静态检查、build/flash 与重启后运行日志验收。

## Type 重启后的新增证据（2026-09-05）

主机端 Type 使用最新 1.0.5 release 重启后，`%LOCALAPPDATA%\Listener Type\Logs\listener-type.log` 仍观察到固件 session 计数回退：最近 504 个 `VoiceActivation` `SessionStart` 中有 15 次从 `7/8/9/10...` 回到 `1`，例如：

```text
13:29:04  session 9 -> 1
13:30:07  session 8 -> 1
13:31:12  session 7 -> 1
13:32:15  session 7 -> 1
13:33:20  session 7 -> 1
13:34:25  session 8 -> 1
```

这些回退大约每 63–75 秒发生，并伴随 GATT Inactive → Active。由于固件 `s_session_id_counter` 只在设备启动时归零，这强烈指向固件重启或 BLE 链路重建后的固件状态重置，而不是声纹判定问题。当前 Type 日志尾部没有新的 `protocol_error=14`，但没有 COM5 就无法读取对应的 `reset_reason`；必须在设备重新枚举后抓取新鲜固件诊断日志完成分型。
