# Evidence：正式 MSI（桌面快捷方式）供 owner 验收

日期：2026-07-28  
Type HEAD：`286783a`（含 TYPE:READY 稳定 + 假唤醒空 ASR 阻断 + reattach/ghost 前序）  
接手前会话未完成点：release MSI 构建 LLVM OOM；安装包停在 `464c8cf`。

## 交付

| 项 | 结果 |
|---|---|
| `windows-package-msvc.ps1`（IncrementalRelease + CargoBuildJobs 1 + InstallMsi + Launch） | **PASS** exit 0 |
| 安装 exe 与 MSI 载荷一致 | **PASS** |
| 桌面快捷方式 `C:\Users\Public\Desktop\Listener Type.lnk` | **PASS** → Program Files |
| `npm run check:release-root-artifacts` | **PASS** |
| 相关单测（cancel / foreground probe ready 路径） | **PASS** 5/5 |

## 指纹

| 文件 | 字节 | SHA256 |
|---|---:|---|
| MSI | 15 245 312 | `EE426E6DA8719E6CCEE7A4C0538E4016FBE937F3FDD317244C2AF4B23C0F4B4D` |
| 安装后 `listener-type.exe` | 38 921 728 | `344F46A8E9C2B0E240CF4A6BEB7E6F89AD1D8FD1C229AFC1A82C2AC1F24C01BA` |

路径：

- 构建：`Listener-Type/.artifacts/windows-msvc/ListenerType_1.0.3_x64_en-US.msi`
- Denzic 根 / `Listener/` / `Listener/releases/1.0.3/`

固件 OTA zip 未在本轮重打（仍为前序 `67A82229…` development 包）；本轮只刷新 Type 正式安装面。

## 本包相对已安装旧 MSI 的增量（`464c8cf` → `286783a`）

- 砍掉超敏 offline 唤醒档；offline 命中需整句确认
- 中途 local Absent / 过短会话跳过 offline，避免堵流式 actor
- 终端 accept 后 post-wake PCM &lt; 1s 直接丢弃，禁止空 ASR 胶囊
- reattach 死锁逃逸、ghost prune 冷却、TYPE:READY 不因 cancel 被拆（前序已在包内或同源）

## 限制

- 构建时为腾出 ~6GB 内存临时结束本机 Minecraft（GTNH / javaw）；可自行重开。
- Authenticode 未签。
- owner 真机体验验收仍为人工 gate（连接稳定性、唤醒灵敏度/误唤醒、胶囊时延）。
