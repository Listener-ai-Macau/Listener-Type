# Evidence：全量刷新 Type 正式包 + 固件 USB 刷写（含 OTA 失败日志）

日期：2026-07-28

## 交付

| 项 | 结果 |
|---|---|
| 固件 USB 全量 flash COM3（erase-otadata + flash） | **PASS** |
| 设备 `~OTA:STATUS` | 1.0.3 / ota_ready / blocker=none |
| OTA zip development（含日志改动，git dirty） | **PASS** 落盘根目录 |
| Type 正式 MSI（含 OTA 失败原文日志 + ghost-prune 跳过） | **PASS** 安装 + 启动 |
| 桌面快捷方式 | Program Files |
| `check:release-root-artifacts` | **PASS** |

## 指纹

| 文件 | SHA256 | 字节 |
|---|---|---:|
| `ListenerType_1.0.3_x64_en-US.msi` | `1576A9FEDE03C1D8BAF3EB544539CEB4BF8A6215BBB6E90F71140E820B9FD3A5` | 15245312 |
| 安装 `listener-type.exe` | `F0917AE1F470A3E6477EF003E1AB321DCB79935223E69BB3D477082C6CFE511C` | （Program Files） |
| `ListenerFirmware_1.0.3_ota.zip` | `E537577DCC85E1FE9D659B682A0112F2EBFB1F966461956E64C5BD02227DE691` | 1540617 |
| OTA bin | `35504060db3b523bf3b513785632e191164514b8fae57e1d6ddeb15b5af402b3` | 1385872 |

源 OTA 目录：`Listener-Firmware/.cache/ota_firmware/listener-ota-1.0.3-20260728-204934`

## 验收用法

1. 桌面「Listener Type」= 本包（不要用旧快捷方式/旧 dev）。
2. OTA 选根目录或 `releases/1.0.3` 的 **新** zip（SHA 见上）。
3. 失败时 Type 日志搜：`[firmware-ota] transfer failed`
4. 串口搜：`Denzic OTA v1 control RX` / `REJECTED` / `storage begin`
