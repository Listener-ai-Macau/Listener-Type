# Owner notes triage — 1.0.5 recording UX

> 日期：2026-08-05  
> 来源：`operator-result.json`（PASS_WITH_NOTES）+ 同日会话口述  
> 接手：Grok section 三次接管（本会话复验后落盘）

## 机器基线（本会话复验）

| 项 | 结果 |
|---|---|
| `verify-listener-1.0.5-protected-contracts.mjs` | **PASS**（1.0.4 baseline + 10 个 1.0.5 门） |
| Program Files 版本 | `1.0.5` |
| EXE SHA256 | `51B1A80EAEDB8C5DD5C61A575F3AA464F60F7BA998EF37AFB5ED35D8D618B594` |
| 进程 | 自 Program Files 运行中 |

## 备注 triage

| ID | 来源 | 摘要 | 判定 | 动作 |
|---|---|---|---|---|
| N1 | step-00 note | 验收指引不清，不知道要读什么 | **fix** | 下一轮验收改为「唤醒词 + 下面这句话」；弹窗去掉模糊 Scope |
| N2 | step-03 note + 会话 | 不懂「长文」；可不可以不开；不要搞这么多设置 | **fix**（产品） | 默认保持关（已是）；弱化/隐藏主路径上的「长文听写」开关，或改成更白话文案；**禁止**再加同类开关堆设置 |
| N3 | step-04 note | 现场有干扰，他人说话暂不测 | **defer** | 明确延期；不挡 1.0.5 机器闭环；他人不拖结束仍靠既有契约门 |
| N4 | 会话更早 | 不要「正在听」「处理中」这类文案打扰 | **accept / 已收敛** | 胶囊空录音仍走 AudioBars；有正文才显示预览；「处理中」仅无文案占位，非强制空态大字 |
| N5 | 会话更早 | 中途被掐很难受 | **fix 已落地** | pending 挡结束 + provisional 刷新时钟；契约门已绿；owner 体感「还行」 |
| N6 | machine | 1 次 wake→胶囊 1680ms 超 ceiling | **watch** | 4/5 达标；继续观察日志，不单独开大改除非再复现 |
| N7 | machine | `polish_failed=true` 但仍原文粘贴 | **accept 路径 / 环境查** | 产品合同（失败可感知）PASS；润色凭证属环境，可选另查 |
| N8 | 工作树 | 1.0.5 大量未 commit | **hold** | 等 owner 明示再 commit；禁止擅自 reset |
| N9 | 日志 18:14–18:15 | 唤醒后空体 ~1.2s 自动结束 →「没有识别到语音」；owner 口述「又自动结束了」 | **fix 已落地并装包** | 正文未开始用 3.0s 放弃阈；正文后仍 1.0s。证据 `.artifacts/listener-ux-wake-no-body-20260805/` |

## 完成态判断

- **机器闭环**：PASS（含 N9 新契约门）  
- **Owner 正式静默 PASS**：未落定（PASS_WITH_NOTES + 需复测 N9）  
- **Grok section 当前责任**：N9 已装到 Program Files；等 owner 体感；N2/N7/N8 仍待拍板

## 建议下一刀（需 owner 选）

1. **先复测 N9**（唤醒后想 2 秒再说话 + 正常短句 1s 收）  
2. **收设置（N2）**— 藏/改名长文开关  
3. **查润色失败（N7）**— 环境/凭证  
4. **commit 1.0.5（N8）**— 你确认后再动  
