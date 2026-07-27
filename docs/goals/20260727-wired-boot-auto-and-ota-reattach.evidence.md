# Evidence：有线自动 Boot + OTA 重连

日期：2026-07-27

## 有线 Boot

### 之前
- UI 单独「Boot 修复」按钮 + 全量「刷入」  
- 全量刷入其实本来就会写 bootloader，但用户要自己判断是否先修 boot  

### 现在
- **去掉** 设置页 Boot 修复按钮  
- 全量 `flash` 连接后：`read_flash(0x0)` 探测 magic `0xe9`  
  - Present → 日志记录，仍全量刷新 boot+分区+app  
  - Missing/Corrupt → 日志 + 阶段提示「auto-repairing via full factory flash」，随后全量写入（含 bootloader）  
  - Probe 失败 → 仍写入 bootloader（不挡刷写）  
- CLI `repair_wired_firmware_bootloader` **保留**（高级/脚本用）

### 测试
- `cargo test --lib boot_probe_tests` → 2 passed  
- `load_wired_firmware_package*` → 3 passed  
- `tsc --noEmit` → PASS  

## OTA 后连不上 Type

### 根因（机制）
- OTA 成功后设备会 reboot；Type 要重新开 notify（尽量复用同一 BLE 地址，**不是**强制整机重新配对）  
- 旧等待窗口仅 **8s**，Windows 蓝牙重新枚举/GATT 恢复经常更久 → 报「notify 未恢复」  
- 用户体感像「升级后要重新配对却配不上」  

### 现在
- 首次等待 **28s**  
- 超时：`request_listener_ota_post_confirm_notify_fast_retry` + `refresh_embedded_ble_listener_after_firmware_ota` 后再等 **20s**  
- 仍失败：错误文案提示「一键修复 / Windows 里重新配对」  

### 说明
- 若 Windows 绑定密钥损坏，仍可能需要 **一键修复** 或系统蓝牙重新配对；本次修的是「等太短 + 只试一次」导致的假失败  

## 文件
- `src-tauri/src/commands/device/firmware.rs`  
- `src/pages/settings/FirmwareOtaPanel.tsx`  
