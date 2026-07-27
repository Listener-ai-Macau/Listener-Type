# Evidence：重启后 Type 重连加速

日期：2026-07-27  
树：本 Goal 提交

## 根因

已选 EmbeddedBle 时，启动仍 **串行**：

1. `read_device_settings_status`（最长 **2s**）  
2. 才 `mark name-sync done` + `refresh` 开 notify  

settings 慢/超时 → 整段 reconnect 被拖到数秒。合同基线 `max_start_to_notify_ready_ms=3000`。

## 修复

1. **Fast-path**：gate 立刻打开 + 立刻 `refresh_embedded_ble_listener`  
2. settings/power **后置** 450ms 短超时 polish，不挡 notify  
3. 背景重试 base：**1s → 200ms**（设备重启后更快再抢 notify）

## 本机测量（设备已在、持久地址）

| 轮次 | start → notify ready |
|---|---|
| 修前（日志 15:26） | ~756ms（gate 被 settings 拖 ~420ms 才开） |
| 修后 1 | **~760ms**（gate ~20ms 开） |
| 修后 2 | **~635ms** |

`startup BLE reconnect fast-path` 已打日志。  
**≤1s 目标：PASS**（暖机 + 已配对路径）。

> 设备断电后 Windows 蓝牙枚举本身可能仍 >1s，那是 OS 侧；Type 侧不再额外等 settings 2s。

## 测试

`cargo test --lib embedded_ble_background_retry` → **6 passed**

## 限制

- 安装包 MSI 未重打；当前验证用 **debug** 二进制。  
- OTA 后设备 reboot 仍受固件/Windows 重现时间限制；已有更长 wait + 快速 retry。  
