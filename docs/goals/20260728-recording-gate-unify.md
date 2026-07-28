# Goal：录音准入统一（RecordingGate）

## 背景

录音有多条入口（桌面热键、设备键、BLE Started/PCM、声纹候选 promote、胶囊 UI），
OTA 屏蔽原先散落在各处 `embedded_ble_ota_active` 判断里。Owner 要求统一入口。

## 目标

1. 抽出单一 **RecordingGate**：`RecordIntent → decide → Allow | Deny`
2. 所有录音相关入口只走 Gate，不再各自读 OTA flag
3. 当前策略 v1：OTA 独占时拒绝全部录音意图；允许 Idle 类胶囊以便 suppress 清 UI
4. 行为与 OTA 屏蔽合同一致（防回退），后续策略只改 Gate

## 成功标准（机器闭环）

1. `cargo test recording_gate` 与相关 coordinator 编译通过  
2. 源码契约：hotkey / device-key / BLE start-pcm-begin / capsule / background listener  
   不再直接 `embedded_ble_ota_active.load` 做准入（仅 Gate + OTA 状态机置位）  
3. 单元测试覆盖：OTA on 挡全部 start 意图；OTA on 只挡 Recording 类胶囊；OTA off 全放行  

## 人工边界

无强制 owner 硬件验收；若需回归，用 debug Type 在 OTA 中按设备键确认无胶囊。
