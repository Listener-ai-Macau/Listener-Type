# Evidence：OTA 失败原文日志（Type + 固件）

日期：2026-07-28

## 问题

UI 显示「设备拒绝升级」时，Type 日志只有 `ota_gatt_transfer_failed` / `error_category=transport`，**没有**完整 host error 字符串；固件端 BEGIN/拒收日志不够醒目，难对照串口。

## 改动

### Type

- `firmware.rs`：prepare/start/transfer/version-confirm 失败均 `log::error!("[firmware-ota] …: {error}")`
- `observability.rs`：`record_transfer_failed` / handoff failed 额外打 `detail={message}`
- `windows_ble/mod.rs`：非 auth transfer 失败与 secure reopen 耗尽打 ERROR
- `embedded_ble_runtime.rs`：OTA 独占期间跳过 deferred ghost-prune（避免 dual-lane 中途 prune）
- `FirmwareOtaPanel.tsx`：按 timeout/begin 细化 next-step，并继续展示 raw message

### Firmware

- `ble_firmware_ota_esp32.c`：control RX BEGIN 打 size/chunk/window；REJECTED 打 error 名；storage begin reject 打 blocker
- `firmware_ota.c`：begin REJECTED 文案含 blocker + image_size + running version

## 机器检查

| 项 | 结果 |
|---|---|
| `cargo test --lib ota_transfer_failure_restores_the_background_listener` | **PASS** |
| `idf.py -B build app`（含 ble_firmware_ota / firmware_ota） | **PASS** exit 0 |

## 使用说明

- Type：需装**本改动后**构建的 MSI/debug；日志路径 `%LOCALAPPDATA%\Listener Type\Logs\listener-type.log`，搜 `[firmware-ota] transfer failed`
- 固件：需刷**本改动后**的 bin/OTA；串口搜 `Denzic OTA v1 control` / `storage begin REJECTED` / `OTA begin REJECTED`
