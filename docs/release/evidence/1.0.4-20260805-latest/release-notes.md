# Listener Type 1.0.4 最新发布（2026-08-05 spike-fix）

> 状态：已装到 Program Files；版本身份 **1.0.4**  
> 说明：在今日 UX 包之上，再修 LLM 401 / TSF / 唤醒尖刺。

## 版本身份

| 项 | 值 |
|---|---|
| 产品版本 | `1.0.4` |
| MSI | `Listener/ListenerType_1.0.4_x64_en-US.msi` |
| MSI SHA256 | `D6D933C89B9559BF3DA194A26034108516FAC56E523FCBE5B2C04A9D56575502` |
| Program Files EXE SHA256 | `88293E1995FA964F459A451A7860E1419105461D1C8F903F3BD9F2728FCBF17B` |
| 安装路径 | `C:\Program Files\Listener Type\listener-type.exe` |
| 载荷一致 | MSI 提取 EXE = 安装路径 EXE **PASS** |

## 体验增量（相对早间 1.0.4）

1. 多人隔离（owner 已确认）  
2. 收尾跟手 / 上屏兜底  
3. **LLM 真润色路径恢复**：vault baseUrl 别名 + 切到 deepseek 正确 key/endpoint  
4. **TSF 激活更稳**：soft-fail + 三次重试 + 上屏时再 prepare  
5. **唤醒尖刺**：KWS immediate 地板 700ms  

## 机器门

- `node scripts/check-release-version.mjs` → 1.0.4 PASS  
- `node scripts/verify-listener-1.0.5-protected-contracts.mjs` → 14 gates PASS  

## 工程

- 本包对应工作树改动待 commit（owner 已要求入库）。
