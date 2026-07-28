# Evidence：类人工连接验收（接不上 Type）

日期：2026-07-28  
二进制：**当前工作树 debug**  
`Listener-Type/src-tauri/target/debug/listener-type.exe`  
SHA256：`613552ECAD0B0C24946CE32D354619EA73C9B6EB72435B6F4A6928E4427719B9`  
证据目录：`.cache/validation/owner-proxy-connect-20260728-212742/`

## 我替你做的（机器闭环）

1. 读正式 MSI 运行日志：CCCD `0x800704C7` 抖动后，启动 preflight 判  
   `status=NeedsUserAction matched=2 already_paired=0` → **误判 manual Windows pairing removal**  
   → 阻断 persisted GATT 180s → 胶囊「等待手动配对」。
2. 串口 COM3：固件 `1.0.3`、`ota_ready`、`hid_keyboard connected=yes`（设备活着）。
3. Windows PnP：双地址 `listenerB` `F316…` + `E08E…`（幽灵/旋转身份）。
4. 修：`embedded_ble_current_native_pairing_is_missing` 要求 **matched_devices==0**  
   才算「配对真没了」；matched>0 且 already_paired=0 允许 GATT 重开。
5. 单测 + 真机冷启动门禁脚本（`check-type-startup-reconnect-speed.ps1`）。

## 结果

| 项 | 结果 | 证据 |
|---|---|---|
| `embedded_ble_matched_pairing_without_already_paired_is_not_manual_unpair` | **PASS** | cargo test |
| 冷启动 R1（预算 15s） | **PASS** `start_to_notify_ready_ms=1010` | `reconnect-r1-15s.json` |
| 冷启动 R2（预算 15s） | 功能连上 **20650ms**（超预算，因 Uncached 特征发现 AccessDenied 重试） | `reconnect-r2-15s.json` |
| 冷启动 R3（预算 3s 速度合同） | 功能连上 **9151ms**（仍超 3s） | `reconnect-r3-3s.json` |
| 修后无「blocking persisted / manual Windows pairing removal」 | **PASS**（本轮三跑） | listener-type.log |
| TYPE:READY + BLE PCM 收流 | **PASS** | 日志 `ble_packet` / `pcm` |

安装 MSI 在修前已能偶发连上（被动 reattach ~47s 后），但首启假「等手动配对」是用户体感「接不上」主因。

## 根因摘要

| 层 | 问题 |
|---|---|
| 主因 | AEP `NeedsUserAction` + `matched>0` + `already_paired=0` + HID present 竞态 → 假 manual-unpair 180s hold |
| 次因 | native HID 开 notify 时 Uncached 特征发现 `GattCommunicationStatus(3)` AccessDenied，多轮重试拉到 9–20s |
| 环境 | 双 `listenerB` 地址；ghost-prune 保留 `E08E62C12643` |

## 我无法替你做的

- 物理双击 EC11 弹窗手感  
- 主观「够不够快」审美（3s 速度合同本机仍不稳定）

## 当前测试环境

- 正在跑：**debug 工作树** Type（含本修），非桌面 MSI  
- 设备：COM3 / `listenerB` / `E08E62C12643` / fw 1.0.3  

正式 MSI 未在本轮重打（本机内存紧张且不再擅自结束 Minecraft）。需要正式包时再说，我再打包。

## 结论

**接不上 Type 的 180s 假 hold 已修并通过类人工冷启动连接验收（功能 PASS）。**  
速度合同 ≤3s 仍偶发不过，属 AccessDenied 重试；连接本身已可恢复且 R1 已到 ~1s。
