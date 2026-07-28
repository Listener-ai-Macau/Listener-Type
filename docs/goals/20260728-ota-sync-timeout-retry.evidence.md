# Evidence：OTA 首窗 SYNC 超时 → 传输失败（设备拒绝升级误报）

日期：2026-07-28

## 现象

Owner 用桌面正式 Type OTA 时 UI 显示「设备拒绝升级」。

## 日志（Type，UTC 12:58）

| 时间 | 事件 |
|---|---|
| 12:58:37 | preflight / TYPE:OTA handoff 成功 |
| 12:58:40 | prepare 成功 `denzic_ota_v1` dual_lane=true |
| 12:58:41 | exclusive handoff 后 reopen secure target；BEGIN 进入 dual-lane |
| 12:58:52 | **`OTA sync failed: BLE Denzic OTA v1 sync write timed out after 8000 ms`** |
| 12:58:52 | `ota_gatt_transfer_failed` transport ~11.8s |

无 `protocol_error=14`；**BEGIN 已成功**，卡在**首窗 bulk 后的 SYNC** WriteWithResponse。

## 根因

1. dual-lane 首窗可到 ~400×500B；设备在 SYNC 路径上 `ble_firmware_ota_drain_queued_data_locked()` 持锁刷 flash 才回 ATT。
2. Host 旧逻辑：SYNC 仅 8s、超时不重试、整次 transfer 也不按 timeout reopen → 一次挂死即失败。
3. UI 把 transport 失败归到 `deviceRejected`，看起来像「设备拒绝」。

## 修复（Type）

| 项 | 内容 |
|---|---|
| SYNC settle | WWR flush 后 quiet **150ms** |
| SYNC timeout | **25s**（覆盖 erase-heavy 首窗 drain） |
| SYNC 重试 | timeout/auth 最多 **4** 次，间隔 300ms，重试前再 flush |
| 整次 transfer | timeout 与 auth 一样 reopen secure target，最多 3 轮 |
| 日志 | transfer 失败 ERROR 带原文；`detail=` 进 obs |
| UI next-step | timeout/begin 分文案，不再只写「设备拒绝」 |

## 机器证据

| 项 | 结果 |
|---|---|
| `cargo test --lib ota` | **PASS** 53/53 |
| `windows-package-msvc.ps1` IncrementalRelease + Jobs1 + InstallMsi + Launch | **PASS** |
| 安装 exe ≡ MSI 载荷 | **PASS** |
| exe 含 `transfer auth/timeout error` / reopen / `[firmware-ota] transfer failed` | **PASS** |
| `check:release-root-artifacts` | **PASS** |

## 指纹

| 文件 | SHA256 | 字节 |
|---|---|---:|
| `ListenerType_1.0.3_x64_en-US.msi` | `BEE4735D189FC4CE1E2BEF331539563DE8BF582DCD11E66BD679AF518B2F25F2` | 15245312 |
| 安装 `listener-type.exe` | `45693BA42C738F4C5A8B16A2BCF3EEEE35F104DD2A6F46B762FC030DBA9801E2` | 38909440 |
| OTA zip（未改，仍 development 含日志） | `E537577DCC85E1FE9D659B682A0112F2EBFB1F966461956E64C5BD02227DE691` | 1540617 |

路径：`.artifacts/windows-msvc/`、`Listener/`、`Listener/releases/1.0.3/`、Denzic 根。  
桌面快捷方式 → Program Files。

## Owner 复测

1. 只用桌面「Listener Type」（不要旧 dev）。
2. OTA 选根目录/releases 的 zip（SHA 见上）。
3. 成功：日志有 `transferred … bulk_kb_s=`；失败：搜 `[firmware-ota] transfer failed` 与 `sync failed` / `auth/timeout error round=`。

## 限制

- 构建前临时结束本机 `javaw`（Minecraft）腾内存；可自行重开。
- 本轮未重刷固件；若设备仍是旧镜像，可继续用现有 1.0.3 OTA zip。
- Authenticode 未签。
