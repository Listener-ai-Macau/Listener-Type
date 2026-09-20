# Listener E/F（说话人归属）接手指令 — 2026-09-17

写给新会话的执行代理。背景手册：`docs/architecture/grok-handoff-2026-09-17.md` 与 `C:\Users\Billy\Documents\Codex\2026-09-12\c-users-billy-desktop-denzic-listener\coordination-model.md`（A–H 总控规则、分工、历史执行卡都在里面）。本指令自足：只按本文件即可继续 E/F。

## 1. 你的任务（唯一目标）

把 E/F 彻底做完：E=干扰下吞掉目标说话人文字；F=把旁人话输入进目标文本。

做完的定义（已与用户确认）：**代码修复 + 定向回归 + 同候选构建安装 + 一次真实干扰实机验证**。
E/F 之后按总控顺序：重验 C/D（r17 的 C/D 证据被 E/F 污染，作废）→ G → H，全程维持 A/B 反回归。整体发布状态保持 NO-GO 直到 A–H + PCM 区间解释 + reset_reason + 真实干扰证据全部闭合。

## 2. 接手第一步（不许跳过）

Type 仓库 `C:\Users\Billy\Desktop\Denzic\Listener\Listener-Type`，分支 `use/1.0.5-good`，HEAD `e347c435e11ede44489246aca2193a058f174b7f`。执行只读 `git status` / `git diff`，把 dirty/untracked 分层。**保留所有现有修改，禁止 reset、checkout、覆盖、删除历史工作。**

## 3. 已确立的根因（上一会话完成，不要重新调查，直接用）

1. r10–r17 全部真人轮次：最终仲裁处 `local_speaker_tracking_enabled=false`（tracking=false），日志无 `[asr] local session-speaker tracking anchored to wake speaker` —— **local speaker tracking 在这些会话从未激活**。
2. tracking=false 时，`src-tauri/src/asr/volcengine.rs:3484-3497`（`filter_result_to_target_speaker_with_local_evidence_and_anchor`）把云端 target_speaker_id 锚定到**第一个 stable utterance 的说话人**（匿名 first-speaker-wins，无声纹、无唤醒短语绑定）。
3. r17（`work/human-endpoint-cd-r17-20260917/`，会话 a342c13b）：旁人先说话（"是一百分，我看一下这个。"，0–3.4s）→ target=旁人；本人 12s 正文全部判为 non-target → `stable_non_target_utterance_present=true` → `explicit_non_owner_tail=true`。
4. `src-tauri/src/speech_decision_kernel.rs:157`：explicit_non_owner_tail 绝对否决 → authority=speaker_filtered → **旁人 10 字上屏，本人 57 字被毁（E+F 同时发生）**。userInitiatedStop=true（D 也未过）。
5. r13–r16 通过只是因为单人场景：第一个说话人恰好是本人。
6. **回归窗口**：候选 85F3AD99（neural-tts r1–r4，2026-09-17 03:33Z，`work/a-neural-tts-candidate-20260917-r1..r4/`）日志有 anchor 行 + tracking=true（tracking 正常工作）；候选 0CEA13B2（r10，09:45Z）起两者皆无。两者同 HEAD e347c43（r1 run-meta.json 可证）→ **回归在 2026-09-17 11:33–17:45（本地）之间的未提交 dirty 改动里**。
7. ⚠️ 混杂变量：85F3AD99 轮是 TTS 扬声器回放，r10+ 是真人。先区分两种假设再改代码：
   (a) dirty diff 杀掉了种子/调用/note；
   (b) r10–r17 的接受走了不种子的路径。live 路径 `try_release_automatic_candidate`（dictation_embedded_stream.rs:2890，live accept ~3960-4088）在 :4083 **无条件**调用 `session.start_local_speaker_tracking(...)`，种子在 :4007-4012 构建（candidate.pcm / wake_match.end_seconds / phrase / enrolled_owner_matched）；terminal 路径 `finish_buffered_speaker_candidate`（:1212）→ `stage_terminal_wake_continuation_with_body_at` → 新会话在 dictation_embedded_stream_session.rs:303 种子。两条路都应 anchor —— 用 r17 trace 帧判断实际走了哪条、断在哪。
8. 源扫描测试 `dictation_tests.rs:11802 every_automatic_wake_path_seeds_session_speaker_tracking` 只做字符串扫描（当前通过），**检测不到该运行时回归** —— 修复时必须补真正的行为级回归。
9. 已知不对称（修复时要消解的设计缺陷，但不是本回归的直接原因）：`local_wake_owner_verified=false` 时本地证据只能定罪（Other）不能救援（最高 Unknown）；owner-recovery 路径全部 gated on verified（volcengine.rs:1093 / 1754 / 2163 / 5097）。

## 4. 修复设计方向（用户原话，必须遵守）

> "这个是不是应该要录制声纹才行， 然后增加准确率， 没有录制声纹的就用开始录音的那个人说的话这样子会好一点"

落地为锚定优先级：
1. **已录制声纹 → 声纹优先定 target**。磁盘已有：Credential Manager 服务 `com.listener.type.voiceprint`，`owner-template-v2-5` = target_speaker blob（192 维，version=4 JSON/每 blob，phrase=开始录音，写入 2026-09-16 10:32:46），共 7/7 blob 完整。
2. **无声纹 → 用说唤醒短语（"开始录音"）的那个人**绑定 target（`wake_phrase_bound_target_speaker`，代码已存在）。
3. **任何情况下不允许匿名 first-speaker-wins 锚定**（volcengine.rs:3484-3497 的 else 分支）。
4. 冻结：EnrolledNonMatch+phrase→Accept 的唤醒开放接受策略**不许动**。
5. kernel 否决（explicit_non_owner_tail）保留，但必须在正确的 target 身份之上运行。
6. 硬件是单麦克风（SPH0645LM4H，I2S MONO）—— 一切方案单通道，没有波束成形。

## 5. 关键代码位置

- `src-tauri/src/asr/volcengine.rs:3484-3497` — THE BUG：tracking=false 时的 first-speaker-wins 锚定
- `src-tauri/src/asr/volcengine.rs:4173` — `state: ParkingMutex::new(SyncState::default())`，每实例一次，无生产 reset 路径
- `src-tauri/src/asr/volcengine.rs:4279-4321` — `start_target_speaker_extraction`：enrolled embedding 优先，Ok(None)→WeSep wake-PCM 兜底，Err→跳过流（feature `target-speaker-extraction`）
- `src-tauri/src/asr/volcengine.rs:5338-5383` — note 函数族；`note_local_speaker_tracking_started_with_owner` 置 `local_speaker_tracking_enabled=true` 并打 anchor 日志
- `src-tauri/src/asr/volcengine.rs:2496` — `final_explicit_non_owner_tail`（= local_preview_exclusion_seen ‖ stable_non_target_utterance_present ‖ (stable_other_speaker_present && !cloud_row_is_verified_owner_continuation) ‖ owner_isolation_frozen ‖ stable_provider_foreign_row_has_local_veto）
- `src-tauri/src/speech_decision_kernel.rs:154-173` — `arbitrate_final_transcript`：explicit_non_owner 无条件否决 raw_recovery
- `src-tauri/src/coordinator/dictation_wake_polish.rs:519-547` — `start_local_speaker_tracking`：note + extraction 的唯一生产调用点；verified / 非 verified 两个 note 分支
- `src-tauri/src/coordinator/dictation_embedded_stream.rs:4083` — live 路径无条件调用（种子 :4007-4012）；:1212 / :2747 / :2890 为三个候选出口函数
- `src-tauri/src/coordinator/dictation_embedded_stream_session.rs:303` — terminal continuation 种子
- `src-tauri/src/speaker_verification.rs` — :1880 `target_speaker_embedding_for_phrase`；:1894 `session_profile_from_wake`；:1217-1300 模板编解码；~:395-420 KEYRING 常量
- `src-tauri/src/coordinator/dictation_tests.rs:11802` — 现有源扫描测试（要补行为级）

## 6. Definition of Done

1. **定向回归**（不跑无新增信息的全量）：至少覆盖 (a) 锚定选择：有声纹→声纹、无声纹→wake 短语说话人、绝不 first-speaker；(b) r17 反例形状：旁人先说话 + 本人正文 → 本人全文保留、旁人不进 final。
2. **同候选构建 + 安装 + hash 核对**（raw / MSI payload / installed 三者分别记录；用 msiexec /a 提取核对 payload=installed，旧 verifier 的 raw 对比会假失败）。
3. **一次真实干扰实机轮**（r17 剧本重跑）：旁人先开口，本人后说正文。PASS = 本人全文逐字上屏（Inserted + target_confirmed=true + history/final/目标读回一致，同 session id）、旁人文字零进入、说完约 1s 自动结束、A/B 无回归（无误唤醒、唤醒正常）。
4. 每轮给出明确 **PASS / FAIL / UNVERIFIED / EXTERNAL_BLOCKED**。

## 7. 硬规则（verbatim，违反任何一条即违规）

- 先检查 git diff、git status，保留所有现有修改。禁止 reset、checkout、覆盖、删除历史工作。
- 不要把旧安装包、单元测试或代码可读性当成实机通过。
- 不要为了通过测试制造特例。
- 不要在 PCM/BLE 差异解释前调整唤醒阈值、声纹阈值、firmware 或 endpoint。（r12 的 trace PCM 951616 vs BLE 896000 差异仍未解释。）
- 不要把同一录音扬声器外放冒充真实第二说话人验收。（E/F 的最终验收必须真实第二说话人；TTS 回放只可用于定位。）
- 不要使用 PowerShell、终端或错误前台窗口作为目标上屏窗口。
- 不要跑没有新增信息的 Rust 全量测试；只做能直接支持当前修复的最小验证。
- 每一轮必须形成明确的 PASS、FAIL、UNVERIFIED 或 EXTERNAL_BLOCKED 结论。

## 8. 候选身份（hash 链）

| 候选 | 身份 | 状态 |
|---|---|---|
| 85F3AD99…（前缀；完整 hash 见 `work/a-neural-tts-candidate-20260917-r1/run-meta.json`） | neural-tts r1–r4，03:33Z | tracking **正常**（对照基准） |
| 0CEA13B2…（前缀；完整 hash 见 r10 run-meta） | r10–r12，09:45Z 起 | tracking 失效 |
| `BCAA02010D03C11E7C3C143C4125505024B2B2B1496C622394EC620B8049B17C` | r13–r17 当前安装 | tracking 失效；r16 TSF 上屏 PASS 在此候选上取得 |
| raw `37F7B8536F676353039FB9379F67E794E6ECF0F009F69470C1B5685FAE7339F8` / MSI `2144AE04F2F1469F5D962B5BDD3050E5EBB4C891E7921BDCD5115C49125069EE` | 同 BCAA 源码重构建（20:48） | **未安装**（保持 BCAA 证据链） |

## 9. 环境与陷阱

- **TSF DLL 注册于 `src-tauri/target/windows-ime-register/20260917202522-23888/`**（active-registration.json 指向它）—— 清理 target/ 会破坏 IME 注册。上屏验收复用 r16 已验证的 TSF 链路（`work/human-endpoint-onscreen-r16*/`）。
- 上屏交付要求弹窗是前台窗口。
- 调试编译 OOM：`CARGO_INCREMENTAL=0`，或 cargo `profile.dev.debug=0`（历史上 PDB 损坏时靠它完成链接）。
- 源码扫描类测试对 CRLF 敏感（.gitattributes 已强制 LF）。
- 固件自测直接走 COM3 串口（`~DEVICE:SET` / `~OTA:STATUS` / `~DIAGLOG`），别让用户手动；崩溃看 flash ~DIAGLOG 而非串口实时。
- r17 会话启动日志已被轮转永久丢失（12:33:30–44Z 空洞，app 自身日志 `C:\Users\Billy\AppData\Local\Listener Type\Logs\` 同样从 12:33:44 起）—— 以 r17 verdict/trace 为准，不要再找启动日志。
- OTA control 必须同步写读验证（RECEIVING），桌面端 `denzic-platform` 依赖此语义。

## 10. 证据索引

- `work/human-endpoint-cd-r17-20260917/` — r17 E/F FAIL：verdict + trace（186 帧）+ 日志
- `work/a-neural-tts-candidate-20260917-r1..r4/` — tracking 正常的对照（anchor 行 + tracking=true）
- `work/human-endpoint-onscreen-r13..r16/` — r16 TSF 上屏 PASS（会话 f547cb54 / embedded 311230637）
- `work/human-endpoint-auto-stop-r10..r12/` — C/D 轮（tracking 均 false）
- Credential Manager：`com.listener.type.voiceprint` / owner-template-v1 + v2-0..5（v2-5 = target_speaker 192 维）

## 11. 执行顺序

1. git status/diff 分层（保留一切）。
2. 定位回归：用 r17 trace 判断接受路径（live vs terminal）→ 读当前代码确认该路径是否到达 `start_local_speaker_tracking` → 在 dirty diff 里找 2026-09-17 11:33–17:45 窗口内动过 dictation_embedded_stream.rs / dictation_wake_polish.rs / volcengine.rs 种子链的改动。找不到快照就靠日志取证 + 代码走读，不要瞎猜。
3. 实现修复：恢复 tracking 激活 + 按 §4 优先级重写锚定选择（声纹优先 → wake 短语说话人兜底 → 禁止 first-speaker）。
4. 定向回归（§6.1）。
5. 构建、安装、hash 核对（新候选身份）。
6. 一次真实干扰实机轮（§6.3），出 PASS/FAIL 结论。
