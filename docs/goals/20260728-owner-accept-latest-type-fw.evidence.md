# Evidence：最新 Type + 固件已开，供 owner 验收

日期：2026-07-28

## 交付

| 项 | 结果 |
|---|---|
| Type MSI 构建+安装+启动 | **PASS**（含 `fa764fc` 连接假 hold 修） |
| 桌面快捷方式 | Program Files |
| 固件 USB flash COM3（erase-otadata + flash） | **PASS** |
| 设备 `~OTA:STATUS` | 1.0.3 / ota_ready / blocker=none / hid_ready / audio_ready |
| OTA zip 落盘 | **PASS** |
| `check:release-root-artifacts` | **PASS** |
| 安装后启动 `notify ready` | **PASS**（UTC 14:20:41 同秒 ready） |

## 指纹

| 文件 | SHA256 |
|---|---|
| MSI | `0689459BA96221D7E145478D9EEA699F6A145AF9D0B58B36A6139076B3D34C87` |
| 安装 exe | `326E9B47D76053B24D08E790EB2086F6A325046E0EF9D0AB41E5B3016E067193` |
| OTA zip | `2DC484793A410156F2CFE7C7304CA1C3B1959447E1DAFE3F45F87AC2F85958CD` |
| app bin | `35504060DB3B523BF3B513785632E191164514B8FAE57E1D6DDEB15B5AF402B3` |

路径：桌面快捷方式 / `Listener/` / `Listener/releases/1.0.3/` / Denzic 根。

Type HEAD：`fa764fc`  
Firmware HEAD：`f678103`（已刷 USB）

## Owner 建议看

1. 桌面 **Listener Type** 是否已接上（不要旧 debug 快捷方式）
2. 双击 EC11 重配 / 单击录音 / 唤醒（按你的习惯路径）
3. OTA 若试：选根目录 `ListenerFirmware_1.0.3_ota.zip`（SHA 见上）
