# Evidence：唤醒词灵敏度（KWS）优化

日期：2026-07-27  
树：工作区（commit 随 push）

## 根因

本机 `%AppData%\Listener Type\models\speaker-verification\keyword-spotting\calibration.json` 被声纹/唤醒录入流程写成 **score=1.5 / threshold=0.25**（候选列表里**最严**一档）。  
运行时 `configured_keyword_values` 优先读该文件，**压过**产品敏感默认 **3.0 / 0.08**，导致日常漏唤。

## 改动

`src-tauri/src/wake_phrase.rs`：

1. 运行时 KWS **固定** bootstrap `3.0 / 0.08`（不再用严格 enrollment pin）  
2. `persist_bootstrap_calibration_if_missing`：**升级**更严的旧校准文件  
3. `calibrate`：仍验证样本含唤醒词，但落盘固定 bootstrap  
4. `prepare` 打日志 `runtime KWS config … score/threshold`  
5. 改校准后 `clear_runtime_cache()` 强制重建 spotter  

## 机器结果

| 检查 | 结果 |
|---|---|
| `cargo test --lib wake_phrase` | **11 passed / 0 failed / 5 ignored** |
| 本机 calibration 升级 | **1.5/0.25 → 3.0/0.08** |
| 当前树 debug 启动日志 | `runtime KWS config phrase=开始录音 score=3.0 threshold=0.08` |

测试环境：`src-tauri/target/debug/listener-type.exe`（非旧 MSI）。

## Owner 体验验收（请你喊一次）

对着设备说 **「开始录音」+ 一句正文**。应比改前更容易出胶囊。  
若仍漏：把时段记一下，再查是否设备 VAD 未开 / BLE 断 / 环境极噪。

## 防回退

- 主路径仍 `StreamingDetector::new`  
- 无声纹 open-gate 未改  
- 误触仍靠：词本身 +（有声纹时）声纹 + 本地确认路径  
