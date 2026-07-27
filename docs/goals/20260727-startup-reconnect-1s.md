# Goal：重启/更新后 Type 重连 ≤1s（notify ready）

## 现象

Owner：重启或更新后恢复 Type 连接很慢；以前约 3s 内，希望 **1s 内**。

## 根因

已选 `EmbeddedBle` 时，启动仍 **先** `read_device_settings_status(2s)` 做 name/power 同步，**后** `refresh_embedded_ble_listener`。  
name sync gate 未开时 refresh 直接 deferred → notify 被 settings GATT 拖住。

合同：`max_start_to_notify_ready_ms = 3000`（基线实测 ~1.2s）；目标收紧到 **≤1000ms**（有持久地址/HID 路径）。

## 修复

1. EmbeddedBle 已选：先 open name-sync gate + 立刻 refresh  
2. settings/power 并行短超时（~450ms）润色，不挡 notify  
3. 证据：重启测 start→notify_ready  

## 成功标准

- 本机重启：`background listener notify ready` 距 `=== Listener Type 启动 ===` **≤1000ms**（设备已在、持久路径）  
- 单元/编译通过；证据落盘  
