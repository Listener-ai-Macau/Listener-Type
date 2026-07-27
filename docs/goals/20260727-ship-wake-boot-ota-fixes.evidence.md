# Evidence：唤醒 / Boot / OTA 修进可装包（收口）

日期：2026-07-27  
树：`bf93764`（含 KWS 敏感 + 有线 auto-boot + OTA 长等待）

## 卡点与解决

| 现象 | 处理 |
|---|---|
| 代码已修但 owner 仍用旧 MSI | **重打 MSI + 安装 + 根目录 + GH 资产覆盖** |
| 首次 `tauri build` 失败（无 rustc 文案，疑锁/资源争用） | 先 `cargo build --release -j2` 成功，再 `-ReuseExistingExe` 打 MSI |

## 包

| 项 | 值 |
|---|---|
| MSI | `ListenerType_1.0.3_x64_en-US.msi` 13 950 976 bytes |
| SHA256 | `7B03E50D3D97EDF51D7752E49AAB6D75161F3BA34CE3DA96354F9DDD1BFE2F05` |
| 安装 exe | `C:\Program Files\Listener Type\listener-type.exe` |
| 安装 exe SHA256 | `3FAE172A7F6904B347EAA8F961F3F76534E0623F426D668AB4053B4E11978BE2` |
| Denzic 根 | `check:release-root-artifacts` **PASS** |
| GitHub | `v1.0.3` 资产已更新（同文件名） |

## 安装包内代码证据（ASCII 字符串）

| 字符串 | 命中 |
|---|---|
| `runtime KWS config` | True |
| `upgraded less-sensitive calibration` | True |
| `Boot check:` | True |
| `auto-repairing via full factory flash` | True |
| `post-OTA Listener notify not ready` | True |

## 运行时

- calibration.json：**score=3.0 threshold=0.08**
- 安装版启动日志：`runtime KWS config phrase=开始录音 score=3.0 threshold=0.08`（15:16:54Z）
- Type 已从 Program Files 启动

## Goal 结果：**PASS**

真机喊词 / 真 OTA：请 owner 用**本安装版**验收；机器侧包与关键修复字符串已闭环。
