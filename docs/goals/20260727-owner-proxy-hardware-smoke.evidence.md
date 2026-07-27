# Owner 代理硬件冒烟 — 我替你做的

日期：2026-07-27  
Git：`5304ec8`（后续可再 push 本 evidence）  
二进制：**当前工作树** `src-tauri/target/debug/listener-type.exe`（非 Program Files 旧安装）

证据目录：`.cache/validation/owner-proxy-smoke-20260727-215815/`

## 我实际替你做了什么

1. **停掉** 旧安装版 `C:\Program Files\Listener Type\listener-type.exe`
2. 用 **当前树 debug 二进制** 跑完整机侧冒烟
3. 设备：COM3（ESP32）、BLE `Blistener` `D80E2503DD55`、固件 **1.0.3**、电量约 **92%**

## 结果

| 项 | 结果 | 证据 |
|---|---|---|
| BLE active dry-run | **PASS** | `ble-active-dryrun/` |
| OTA 包校验 `--firmware-ota-check` | **PASS** | `ota-check.log` |
| 有线包检查 `--wired-firmware-check … COM3` | **PASS** | `wired-check.log` |
| **真机 BLE OTA preflight** | **PASS** | `ota-preflight.log`：连上 Blistener，`denzic_ota_v1` 可达，无 blockers |
| 当前二进制启动 / 热键 hook | **PASS** | `runtime-smoke.log` |
| **CLI `--toggle-dictation` 真 BLE 听写管线** | **PASS（管线）** | 日志：Idle→Recording→BLE PCM 69760B→Volcengine ASR→空转写「没有识别到语音」→Idle；notify 保持；`enrolled=false` 唤醒路径正常 |

### 空转写说明

没有人对着麦说话时，ASR 返回 empty 是 **预期**，不是重构回归。  
关键路径已验证：主机开始听写 → BLE notify 收流 → ASR 会话 → LED processing → 空结果错误胶囊 → 回到 Idle 且后台 listener 仍 ready。

## 我无法替你做的（物理限制）

- 说出一段有内容的话拿非空 ASR 文本  
- 手感/实体 EC11 键的主观体验  
- Windows 配对弹窗「点一下」的交互手感  

这些不是「你必须再验收一次重构」的门禁；机器 + 真 BLE 管线已替你跑通。

## 结论

**重构后的当前树 Type，在你的真机 BLE 键盘上可用。**  
OTA preflight 与听写 BLE 管线均 PASS。无需你再为「代替验收」重复操作，除非你想亲耳听一次完整 ASR。
