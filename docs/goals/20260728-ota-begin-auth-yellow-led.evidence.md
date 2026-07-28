# Evidence：OTA BEGIN auth 14 / 黄灯

## 日志摘录（用户本次 OTA）

```
00:27:32.282  Listener OTA v1 prepare: ready
00:27:32.282  paused background listener (firmware OTA transfer)
00:27:32.320  capture #3 released
00:27:32.671  capture #4 opening serialized BLE notify session
00:27:32.793  Denzic OTA v1 begin write protocol_error=14
00:27:32–35   begin 加密/绑定未就绪 重试 attempt=1..4
00:29:09      ota_gatt_transfer_failed
```

## 修复点

`PreparedListenerOtaV1Transfer::transfer`（`windows_ble/mod.rs`）：

- Staged 路径 exclusive 后：`open_listener_ota_v1_target_after_active_link_handoff()` 再 BEGIN  
- 仍 auth 失败：reopen + 整次 transfer 再试  
- BEGIN auth 重试 delay 1.5s  

## 机器

- `cargo test --lib ota_finish_reboot_handoff` PASS  
- `cargo build --bin listener-type` PASS  

## Owner

用当前 debug 二进制再试一次同版本 OTA；成功日志应含  
`reopened secure OTA target after exclusive handoff before BEGIN round=1/3`。  
不得再出现「using prepare-time target」回退。  
黄灯若仍亮：点一下设备或等 TYPE:READY 清 retryable OTA error。

## Follow-up (2026-07-28 09:24)

一键升级仍失败的根因：secure reopen 失败时**回退到 prepare 句柄**，BEGIN 仍 14。  
已改为：exclusive 后 `drop(prepare target)`，最多 3 轮 reopen+transfer，**绝不**用旧句柄。  
debug exe `target/debug/listener-type.exe` mtime 2026-07-28 9:24。  
