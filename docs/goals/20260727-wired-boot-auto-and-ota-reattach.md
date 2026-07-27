# Goal：有线刷入自动 Boot + OTA 后重连 Type

## 目标

1. 有线刷入：去掉单独「Boot 修复」按钮；按下刷入时自动检查 0x0 boot，没有/损坏则随全量刷写修复。  
2. OTA 升级后：加长 Type notify 恢复等待，并在超时后自动再 refresh + 重试，减少「升完不能配对/连不上 Type」。

## 成功标准

- UI 无 Boot 修复按钮；文案说明自动检查  
- `run_wired_firmware_flash` 日志含 Boot check  
- `FIRMWARE_OTA_POST_READY_TIMEOUT` ≥ 20s，失败后有二次 retry  
- boot_probe unit tests PASS；tsc PASS  
