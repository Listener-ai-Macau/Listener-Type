# P13 嵌入式音频软件对接 — 参考文档

日期：2026-05-18
协议：各仓库 `!docs/ai_collaboration_protocol.md`
归档状态：`docs/features/p13_status.json`（P13 已完成，JSON 保留执行证据）

## 目标

完成嵌入式 BLE 音频从设备到桌面端听写的完整产品化，包括 batch 基线跑通和流式 ASR ingest 改造。

## 推荐架构

```
ESP32-S3 firmware
  -> BLE GATT Notify: VKA1 session_start / audio_data / session_stop
  -> Listener-Type embedded audio source
  -> Listener-Type existing ASR providers
  -> polish / style / history / insertion
  -> optional text handoff to Listener-ai-agent
```

### Batch 路径（已实现）

```
capture_notifications_once -> Vec<Vec<u8>> -> collect_notifications
  -> reconstructed_pcm -> submit_embedded_pcm_for_dictation
```

录完完整 session 后才开始识别。当前可工作。

### 流式路径（已实现）

```
session_start -> 创建 Listener-Type 听写 session 和 ASR consumer
audio_data     -> 到一包就推一包 PCM 给 ASR
session_stop   -> send last frame / end_session / await final
```

录音期间持续送 ASR，录完后只等 finalization、润色和插入。本轮未要求显示 partial transcript，最终输出以 final text 为准。

## 非目标

- 不改固件 `VKA1` 包语义
- 不把 `audio_data` 重新解释成旧的 `chunk + fragment`
- 不改变 host BLE 订阅顺序
- 不在本轮做 Agent handoff

## 步骤定义

### Batch 基线（步骤 1-12）

| # | 步骤 | 涉及仓库 | 验收标准 |
|---|---|---|---|
| 1 | 固件工具链增强 | 固件 | smoke 脚本可用 |
| 2 | VKA1 协议解析 + BLE 捕获 | Listener-Type | parser + collector 单测通过 |
| 3 | CLI/IPC 入口 + cancel/error 处理 | Listener-Type | `--submit-embedded-audio-*` 可用，coordinator 回 Idle |
| 4 | S3 前端 UI | Listener-Type | source 选择 + BLE 状态面板 + build 通过 |
| 5 | ASR 链路跑通（火山引擎） | Listener-Type | 软件 baseline 转写通过 |
| 6 | P1-P10 固件回归 | 固件 | `--fail-on-warning` 10/10 pass |
| 7 | Listener-Type 前端改动收口 | Listener-Type | `npm run build` + `cargo check` 通过 |
| 8 | 端到端集成验证 | 固件 + Listener-Type | KEY1 → BLE → ASR → 文本完整闭环 |
| 9 | 识别准确度验证 | Listener-Type | seeded 多句 CER 统计，简繁归一 |
| 10 | 固件 master 推送 origin | 固件 | 推送前审核 gate 通过后 push |
| 11 | Listener-Type 分支推送 origin | Listener-Type | 推送前审核 gate 通过后 push |
| 12 | Codex BLE 工具改动集成审查 | 固件 | 审查 `758f296`，决定是否合入 master |

### 流式 ASR 改造（并行拆分）

阶段 3 不等待 `2.4` 推送 origin。`2.4` 是发布/同步 gate，不是本地软件开发阻塞。
`2.3` 进行期间可以先做不改准确率统计逻辑、不占用硬件的流式前置工作。

| ID | 步骤 | 涉及仓库 | parallel_group | 验收标准 | 主要文件 | 可并行性 |
|---|---|---|---|---|---|---|
| 3.1 | 增量 VKA1 collector | Listener-Type | S1 | collector 可逐包接收 `session_start/audio_data/session_stop`，每个 `audio_data` 产出 PCM chunk，不改变 batch `collect_notifications` 行为 | `embedded_audio.rs` | 可与 2.3 并行；只做协议/collector |
| 3.2 | BLE notification 事件流 API | Listener-Type | S1 | BLE host 可通过回调/通道逐包吐 notification，不再只暴露完整 `Vec<Vec<u8>>`；保留 `capture_notifications_once` 兼容入口 | `embedded_ble.rs` | 可与 2.3 并行；不需要硬件，用类型/编译验证 |
| 3.3 | 流式 replay 测试夹具 | Listener-Type | S2 | 用软件 replay 驱动 start/audio/stop 事件流，可断言 chunk 顺序、stop、cancel/error；不依赖 COM3/BLE | `tools/embedded_audio_replay`, 测试 | 可与 3.1/3.2 并行，但最终验收依赖其 API |
| 3.4 | coordinator 流式嵌入式 dictation | Listener-Type | S3 | `session_start` 创建 ASR consumer，`audio_data` 到一包推一包，`session_stop` 后等待 final text | `coordinator/dictation.rs` | 等 3.1/3.2 接口稳定；避免与 2.3 同改 `dictation.rs` |
| 3.5 | cancel/error/link_lost 流式收尾 | Listener-Type | S4 | cancel/error/link_lost 后 coordinator 回 Idle，不留下 Recording/ASR 资源 | `dictation.rs`, `embedded_ble.rs` | 等 3.4 主路径后接 |
| 3.6 | batch/debug 入口兼容回归 + 文档 | 固件 + Listener-Type | S5 | `submit_embedded_audio_file`、`submit_embedded_audio_notifications`、batch BLE once 不回归；`!docs/features/` 反映流式架构 | 测试、文档 | 可在 3.1/3.2 后部分启动，最终等 3.4/3.5 |
| 3.7 | 真实设备流式 smoke | 固件 + Listener-Type | HW | KEY1 录音期间 audio_data 持续送 ASR，停止后输出 final text | COM3/BLE 资源锁 | 后置；需要硬件锁，不和软件并行段抢资源 |

推荐认领顺序：

1. 空闲 agent 先认领 `3.1` 或 `3.2`，两者文件边界独立，均不需要硬件。
2. 另一个空闲 agent 可准备 `3.3` 的 replay 测试骨架，但不要阻塞 `3.1/3.2` 的接口落地。
3. `3.4` 等 `2.3` 不再改 `coordinator/dictation.rs`，且 `3.1/3.2` 接口稳定后再开始。
4. `3.7` 只在软件路径通过后执行，并按协议获取 `COM3` / `BLE` 资源锁。

**历史步骤状态、执行者、验收证据保留在 `p13_status.json` 中；P13 不再作为活跃计划留在 `docs/plans/`。**

## 验收矩阵

### Batch 基线

| 编号 | 场景 | 通过标准 |
|---|---|---|
| P13.1 | BLE host adapter smoke | KEY1 触发后软件端收到完整 session |
| P13.2 | ASR 转写 | 嵌入式 PCM → raw transcript |
| P13.3 | 产品链路 | 润色/插入/历史记录，记录 session stats |
| P13.4 | cancel/error | session_cancel/error/断链后软件回到 Idle |
| P13.5 | Agent handoff | 最终文本进入 Agent 当前会话 |
| P13.6 | 固件回归 | P1-P10 realistic `--fail-on-warning` 通过 |

### 流式 ASR

| 编号 | 场景 | 通过标准 |
|---|---|---|
| S-BLE-1 | 软件 replay 流式路径 | replay start/audio/stop 可进入 ASR，最终文本与 batch 路径一致或接近 |
| S-BLE-2 | cancel/error | cancel/error 后 coordinator 回 Idle，不留下 Recording |
| S-BLE-3 | batch 兼容 | 现有 batch 入口不回归 |
| S-BLE-4 | 真实 KEY1 smoke | 设备 KEY1 开始后 audio_data 持续送 ASR；停止后输出 final text |
| S-BLE-5 | 固件回归 | P1-P10 realistic `--fail-on-warning` 不回归 |

## 已验证记录

- P13.2 软件 baseline：Foundry whisper-small 转写通过，简繁差异需归一
- P13.2 火山 ASR baseline：直连火山 provider 通过
- P13.2 真实 BLE + 火山：407/407 包零丢包，声学削顶导致空 transcript
- P13.6 固件回归：realistic + `--fail-on-warning`，10/10 pass
- P13.3.7 真实 KEY1 流式 smoke：BLE notify subscription 成功，KEY1 启动 embedded session，Volcengine ASR 返回 final text `这是一段新的合成语音，用来检查火山识别和蓝牙。`
- P13.3.7 CLI 收口：`pcm_bytes=262080`，`missing_packets=138`；下游 LLM 401 属于 polish/agent 凭据问题，不计入 BLE -> ASR 验收。

## 完成结论

- P13 batch 基线和流式 ASR ingest 均已完成。
- P13 当前无活跃阻塞项。
- `missing_packets=138` 是真实流式 smoke 中记录到的尾段 gap 注意事项；验收口径是 BLE notify 订阅、真实 KEY1 session、ASR final text 和 CLI 正常完成，均已满足。
