# Listener v1.0.6-rc1 · Type build153 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this update

Routine audio diagnostics now retain one-second liveness with a small reference to the last complete counter snapshot when facts are unchanged. Changed facts and forced lifecycle/error/end records remain complete, and a complete baseline refreshes every 30 seconds. The full bounded diagnostic export is preserved.

A fixed-counter Windows Rust comparison produced 3 full records and 58 short heartbeats instead of 61 repeated full records: 228,872 to 18,876 bytes. This measures repeated diagnostic metadata, not recognition latency or Bluetooth throughput. The installed app's new log records and exact baseline references were checked during natural use; that window contained genuine changing activity, so it is not an overall old/new byte benchmark.

The build retains build152's per-device automatic save, pending-layout protection, clearer fixed-text editor and factual connection-readiness classification. Recognition/ending/insertion policies and radio parameters are unchanged. Firmware37 is the same verified binary ZIP.

## Before using

US$49 is the hardware price; Type is free and open source. Recognition and optional text services require preparation and separate usage charges. This release does not provide a ready-to-use official cloud account or allowance.

Strong-interference exclusion remains incomplete. Prior USB ordinary UI and Bluetooth selected-device backend/readback evidence keep their original scopes; physical HID across computers/boards and full RF following are not comprehensively qualified. This maintenance batch is not a new hardware, purchase, arbitrary-editor or OTA-speed certification. Some remote marketplace/non-English factory content remains untranslated.

## File identity

- Type source:`fd7aa9d4e764e909998b9aba059f7ac40d0c1b6b` · `build-source/type153-20261023`
- MSI:299204608 bytes · SHA256 `d0cb6e5773b88eda449afb89fc80d695df1c78796b550bf80e5998837da77c1c`
- Firmware37 source:`87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · `build-source/firmware37-20261023` · runtime `9d7fbbe67ed0223d`
- ZIP:1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

本批修复重复音频诊断计数挤占日志：相同状态只记录带完整快照编号的短心跳，仍保留每秒时间线；每30秒补完整基线，有状态变化、错误、开始/结束及迟到尾段时保留完整记录，完整诊断导出不变。固定计数对照减少约92%的重复元数据字节，不代表识别或蓝牙提速；实际安装版已观察到新格式和有效引用。

保留152的设备自动保存、连续修改不丢布局、固定文字编辑和连接超时分类修复。固件37未改。真实输入、硬件及购买结论继续沿各自原证据，不把本次维护算作整体售前验收完成。
