# Evidence：开始录音唤醒召回修复

日期：2026-07-27

## 根因（有日志）

Owner 喊词后**不是“完全没听到”**，而是：

1. 设备 VAD 已开 `VoiceActivation` 候选  
2. 流式 KWS **未命中**（`phrase_signal=None`）  
3. 本地 Paraformer 确认也是 `Absent`  
4. `gate_decision=Reject` / `wake_phrase_non_match`（静默拒绝，所以无胶囊）

仅把校准从 1.5/0.25 → 3.0/0.08 **不够**；流式路径在前 800ms 锁增益，设备预录安静时后续词仍可能偏弱。

## 代码修复

| 改动 | 说明 |
|---|---|
| Bootstrap | **3.5 / 0.05**（更敏） |
| 终端二次检测 | 流式未中 → **整段 PCM 全缓冲增益** + 敏感级联 `detect_with_recall_cascade` |
| 诊断 | 默认写 `%LOCALAPPDATA%\Listener Type\Logs\wake-diag-live\`（无需环境变量） |

## 自测环境

- 当前运行：**debug** `src-tauri\target\debug\listener-type.exe`（含本修复）  
- 日志：`runtime KWS config phrase=开始录音 score=3.5 threshold=0.05`  
- 单元：`runtime_keyword_values_use_product_sensitive_bootstrap` PASS  

## Owner 下一步

用**这个 debug 进程**（不是旧 MSI）再清晰说一遍：**开始录音**。  

期望日志出现其一：

- `keyword audio boundary …` + `gate_decision=Accept`  
- 或 `terminal offline recall recovered`  

若仍 Reject，把 `wake-diag-live` 下新 wav 保留，继续收紧 ASR/增益。
