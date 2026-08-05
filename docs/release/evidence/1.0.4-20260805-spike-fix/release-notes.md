# Listener Type 1.0.4 spike-fix (2026-08-05)

## 版本身份
| 项 | 值 |
|---|---|
| 版本 | 1.0.4 |
| MSI | Listener/ListenerType_1.0.4_x64_en-US.msi |
| MSI SHA256 | D6D933C89B9559BF3DA194A26034108516FAC56E523FCBE5B2C04A9D56575502 |
| Program Files EXE SHA256 | 88293E1995FA964F459A451A7860E1419105461D1C8F903F3BD9F2728FCBF17B |
| 载荷一致 | PASS |

## 本刀修的尖刺
1. **LLM 401「API key format is incorrect」**
   - 根因：vault 里 active 是 `ark`，key 却是 ASR accessKey；DeepSeek 有正确 `sk-` key 但字段写成 `baseUrl`，Rust 只认 `baseURL` → 端点回落到 ARK 默认。
   - 修复：`serde(alias = "baseUrl")`；sanitize key；endpoint/key 形状对账；本机 vault 切到 deepseek 并规范化 `baseURL`。
2. **TSF 未激活 → 粘贴兜底**
   - Activate 时 soft-fail ChangeCurrentLanguage；3 次 backoff 重试；录音起点失败后在上屏时再 prepare 一次。
3. **唤醒→胶囊 KeywordModel 尖刺**
   - KWS immediate stage-2 地板 800→700ms，减少 fail-open 后 phrase_tail 空等。

## 机器门
- check-release-version 1.0.4 PASS
- verify-listener-1.0.5-protected-contracts 14 gates PASS
- unit: kws_hit_schedules_immediate_local_confirmation PASS
- unit: creds_llm_entry_accepts_legacy_base_url_alias PASS
