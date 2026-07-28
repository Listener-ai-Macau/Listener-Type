# Goal：OTA BEGIN 加密失败 → 黄灯 error

## 现象

Owner OTA 后设备 **error 黄灯**。

## 日志（Type，2026-07-28 00:27）

1. preflight / prepare 成功（OTA service reachable）  
2. pause background listener，关掉 audio capture  
3. exclusive handoff 后立刻 `Denzic OTA v1 begin write protocol_error=14`  
4. 加密未就绪，重试 4 次仍 14  
5. `ota_gatt_transfer_failed`  

`protocol_error=14` = ATT **Insufficient Authentication**（Windows 侧写 OTA control 时加密会话未就绪）。  
固件 OTA 域会亮 **retryable/hard error LED（黄/琥珀）**（`ota_begin_*` / `ota_abort`）。

## 根因

Staged OTA：prepare 时在 **notify 仍活跃** 时打开 OTA GATT；随后关掉 notify 做 exclusive 传输 → 加密会话掉落 → BEGIN 全失败。

## 修复

1. exclusive handoff 后 **重新 open** 安全 OTA target 再 BEGIN  
2. BEGIN 遇 auth error：再 reopen + 整次 transfer 重试一次  
3. BEGIN 单次 auth 重试间隔 1s → 1.5s  

## 成功标准

- 编译/相关 OTA 单元测试 PASS  
- 再 OTA：日志出现 `reopened secure OTA target after exclusive handoff`，传输完成，黄灯不因 begin-14 常驻  
