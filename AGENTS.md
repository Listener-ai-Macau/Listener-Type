# Listener Type - Agent Operating Guide

Listener Type is a local-first desktop voice input app. It records speech, transcribes it, optionally rewrites it with a user-selected style pack, and inserts the result at the current cursor. Keep that core path intact.

## 开发优先硬约束（2026-10-07）

- 功能开发和修复用户实际遇到的问题是主线。不得用冒烟测试、全量检查、验收框架、目标台账、审批表或文档整理替代功能开发，也不得把它们单独报为修复进度。
- 每次修复都从实际现象追查相关完整数据流、状态流和职责边界，找到首次错误决策及其上游成因，确认根因和受影响范围，再修产生问题的源头。不得止于现象位置，也不得在根因未明时靠阈值、延时、特例或回退遮住症状。
- 修复范围由根因决定，不限制为最小补丁、最少文件或最少代码。根因涉及架构、跨模块状态所有权、生命周期、接口契约或数据模型时，允许并应当进行必要的架构修复；范围须与根因有直接关系。
- 验证必须足以证明根因被消除，并覆盖原始现象、同源触发条件、受影响链路及必须保住的正常行为；不得只验证单个样例、局部补丁或为了通过而写的断言。架构修复按其影响范围验证。每项检查必须回答具体根因问题或交付风险，无关检查不运行。
- 已通过且仍适用于当前实现的检查结果复用；后续改动使结果失效、新故障出现或上次失败需要复核时，再做对应验证。禁止每轮、每次启动或换任务名就重复冒烟、全套测试、品牌/云边界审计和验收弹窗。
- 有历史失败录音及原生返回时，先用它们定位并修复源头，验证受影响的实际链路及正常行为；缺什么才补什么。对应历史片段通过后，才请用户用原弹窗补一次真机测试，不反复让用户念同一场景。
- 编译、打包、安装及功能实机验证按修复范围执行。全量验收在用户明确要求全面验收、准备正式发布或架构修复确实影响整体行为时按需要执行，不把固定行政清单当作开发前提。纯文档、规则或进度记录修改不触发编译、重装或产品测试。
- AIW 合同、归档、目标管理和行政验收不是开始开发或继续修复的前提。只记录必要的实际完成时间、功能变化、验证结果和剩余问题，复用既有进度文档，不另造管理流程。
- 不把检查通过说成功能已修好；修复结论必须与实际证据范围一致。未复现的问题如实保留，不能靠增加无关测试宣称完成。
- 编辑器回执缺失或超时、光标查询失败、输入范围丢失均不代表本人停说，不得直接授权物理STOP。输入事务和录音结束权必须分开；初次未取得写入权限与已写范围被撤销必须分开。可修订输入须在真实编辑器用保存数据验证完整链路后才默认启用，源码状态检查不能替代该证据。
- 本节优先于本文及引用文档中的旧最小补丁限制和验证流程。下文检查按根因和实际影响范围选择，不是每轮必须整套执行的门槛。

## Stack

| Layer | Technology |
| --- | --- |
| Desktop | Tauri v2, Rust |
| Frontend | React 18, TypeScript, Vite |
| Native input | macOS accessibility insertion, Windows hooks and TSF IME |
| ASR | Volcengine, OpenAI-compatible batch ASR, Apple Speech, local Qwen ASR, Windows Foundry Local |
| Polish | Ark, DeepSeek/OpenAI-compatible, Anthropic-compatible, custom OpenAI-compatible endpoints |

## Project Map

- `src/` - React app, pages, shared IPC wrappers, styles and UI state.
- `src-tauri/src/` - Rust backend: recording, ASR, polish, preferences, insertion, updater, IME bridge.
- `windows-ime/` - Native Windows TSF text service used for reliable insertion.
- `scripts/` - Build, packaging, smoke-test and audit scripts.
- `docs/` - User-facing and operational documentation.
- `specs/` - Architecture, design and code traceability.

## Required Reading

- Frontend or visual work: read `specs/DESIGN.md`.
- Backend/session/persistence/provider work: read `specs/ARCHITECTURE.md`.
- Windows packaging/IME work: read `docs/platform/windows-ime.md` and `specs/tech_docs/windows-platform.md`.
- Release/updater work: read `docs/release/updater.md`.
- Any broad change: update `specs/traceability/files.md` or run `npm run check:traceability -- --write`.

## Invariants

- Local dictation, style packs, history, settings and insertion must work without any Listener Type backend.
- Remote marketplace and GitHub OAuth are disabled unless explicitly configured for Listener Type.
- Updater metadata points only at `Listener-ai-Macau/Listener-Type`.
- Do not reintroduce upstream product names, bundle ids, service domains, OAuth client ids or TSF GUIDs.
- Preserve user data boundaries: app data is under `Listener Type`; credential service is `com.listener.type`.
- Keep the legacy `--ol-*` CSS token namespace unless a deliberate full UI refactor updates every consumer and traceability doc.
- Keep dictation decisions and text reconciliation shared across Windows and macOS. Put OS input and accessibility calls behind platform adapters; a Windows-only workaround does not establish product-wide acceptance.

## Verification

### Root-cause repair rule

- For a recording, wake, streaming, speaker, endpoint, preview, or insertion defect, trace the relevant end-to-end audio, provider, local-state, UI and insertion paths. Identify the first incorrect decision, its upstream cause and the violated invariant before changing production logic; locating the surface symptom alone is insufficient.
- Fix the shared decision or state ownership that caused the defect. Keep distinct states such as confirmed owner, confirmed other, and unobserved/pending evidence distinct; missing or delayed evidence is not proof of another speaker or of silence. Do not add a session-specific text, duration, score, or last-row exception as the sole fix.
- Choose repair scope from the root cause. Refactor architecture, state ownership, lifecycle, interfaces or data models when those responsibilities cause the defect; do not cap the fix at a small diff or local workaround.
- Add regressions that establish removal of the root cause and cover the original failure, related triggers, affected paths and preserved normal behavior. Architectural changes require validation across their affected boundaries. Unrelated matrices are not automatic prerequisites.
- Compare the installed executable hash with the current build and validate on new sessions from that build. A source test, an old-session log, or a narrow symptom fix alone is not grounds to claim the issue is root-fixed or ready to release. Keep the release gate until runtime evidence supports it.

Select checks sufficient to validate the root cause and the affected behavior from the following list. Run `build` for frontend changes or packaging, `cargo check` for affected Rust compilation, `check:brand` for identity/branding changes, `check:cloud` for cloud boundary changes, and `check:traceability` for broad changes or file-map changes. A successful build with the same configuration can supply the corresponding compilation evidence; reuse applicable evidence and do not repeat it just to fill a checklist.

```bash
npm run build
npm run check:brand
npm run check:cloud
npm run check:traceability
cargo check --manifest-path src-tauri/Cargo.toml
```

For Windows IME edits, select the affected static scripts under `scripts/windows-*.test.mjs` and document any Windows runtime gap if you are not on Windows.

## Product delivery

- Follow the user's current scope and the product behavior in this repository. AIW Goal contracts, archives and blocking acceptance popups are not development prerequisites.
- Preserve existing uncommitted work. Use focused behavioral regressions while iterating; run release checks once a candidate is ready.
- 实际功能与用户报告的故障优先；遇到具体问题才补对应回归并记录已确认的约束，不把额外行政验收、目标台账或重复冒烟当成修复进度。当前先修间歇唤醒、尾句流式和实际提交后三秒结束，严格旁人文字过滤暂缓；不得把“保字优先模式已关闭严格过滤”说成仍在稳定排除旁人。
- 二遍响应的正文修订与确认状态必须绑定同一响应/音频范围；旧文字较长不能使已确认的新尾句一直等到 STOP。整段识别确认不得代替逐词本人身份，未确认尾句与已经提交的编辑器正文仍须分别处理。
- 有正文响应里的明确空时段与整帧空封口分开处理；只撤回该来源时段内已观察但未确认的精确词，不得让较长旧草稿经预览、兜底或终稿仲裁重新补出。已确认正文、缺失元数据及编辑器实际回执不因此撤销。
- 整句识别确认与逐词声源权限分别持有证据。已确认二遍的同源小幅文字修订、合法明确空时段不得让其它已确认正文一直待确认；整句确认不能补造缺失的逐词时间或本人身份。
- ASR 到提交层必须保留同源识别确认依据。已确认且获准提交的完整词或整句不得再被旧的字数、标点和预览静音门槛扣住；旧协议草稿仍须保留其稳定性保护，确认状态不得代替编辑器实际回执或本人身份。
- 提交事务分别持有原始识别消费位置和显示文字位置；删口令、填充词、转繁简及纠错不能使显示字串充当原音消费坐标。待提交预留不能推进成功消费位置，失败回滚保留前次来源；回调、看门狗及物理 STOP 复核共用该原始位置。无法对应准入正文不是已经交付或静音的证据；续句先在同源原文/原音定位，再做显示变换，不能凭不同捕获的同字同时间借用边界。
- 普通识别中的声纹身份绑定与本人正活动的声道保字保护分开；保字保护不授予本人身份。只排除稳定不同声道、逐词双路他人证据完整且无本人/保护声道重叠的尾部；缺证据仍保留。合法确认空时段须复用识别确认验证器，不能因缺少文本使整帧旁人判断失效。
- 同源普通识别已经作出的旁人排除须传入唯一终稿仲裁及产品恢复边界；本人编号尚未命名、旧预览或旧账本较长，均不能重新放入该次已排除的词。
- 终稿补交须区分原生词的来源权限与同源正文排版；只有完整对齐的提供方排版可用于新添词段，保留既有上屏字节，不能把词列表拼接后丢掉新正文的标点。
- For delivery, build and install the candidate MSI, verify its payload against the installed executable, and identify the matching firmware. Distinguish source tests, installed-app tests and real-device evidence.
- Do not claim hardware or end-to-end acceptance from static checks. Report an unavailable device or external service precisely and continue independent work.
- Recording priorities and accepted behavior: `docs/architecture/repair-order-a-g.md` and `docs/architecture/known-good-2026-09-11.md`.
- 用户要求人工验收复用原版 `C:/Users/Billy/Desktop/Denzic/ai-collaboration-workflow/scripts/operator_prompt.ps1` 及其 `operator_prompt_ui.ps1`。原运行器和场景记录位于 `C:/Users/Billy/Documents/Codex/2026-09-23/new-chat/outputs/listener-acceptance/`；新增场景只修改 guide 数据，保持原渲染器与布局。当前已验证可见的进程宿主记录在 `C:/Users/Billy/Documents/Codex/2026-10-04/listener-repair-handoff/work/original-popup-beta36/launch-manifest.json`。
- 原文使用原弹窗的只读朗读区域；实际输入框与备注分开，真实结果、运行版身份和回执分别留证。弹窗选择“通过”本身不放行产品或售卖。

### Voice wake / voiceprint (anti-regression)

- Persistent voiceprint enrollment uses three guided samples of the current wake phrase
  (about 9 seconds total), Xiaomi-style. Do not add a fourth free-speech prompt. The
  three samples feed both wake and in-session speaker banks; raw enrollment audio is
  discarded after protected templates are saved.
- Deleting the voiceprint must **not** disable automatic wake. No-template path open-gates
  on phrase hit (`speaker_verification::verify` returns match when unenrolled).
- `buffered_speaker_candidate_kind(VoiceActivation, _, enrolled=false)` must still be
  `Verification`, never `Rejected`.
- Primary automatic wake uses `StreamingDetector::new` (sensitive). Reserve `new_strict`
  for in-session diagnostic reactivation only.
- 完整口令与缺一字召回候选必须保留实际命中类型；弱候选不得锁死同一录音的后续完整检测。完整关键词证据不得被二次转写未命中或未完成抹掉；缺字候选、计时和声纹身份本身不能补成完整口令。实时与录音结束路径共用意图/身份仲裁。
- 缺字候选的时间戳及经观察时钟修正的解码时间不能冒充完整口令音频范围，不得据它裁正文或复用本人身份。结束路径须完成完整关键词流，不能因先有弱候选跳过 finish。
- 实时默认检测与未命中后的录音召回须保留实际来源；召回返回完整字符串仍是召回候选，不能仅因标签完整而升级为默认完整关键词并跳过确认。独立口令确认使用产生该召回结果的准确原音窗口，保留原始坐标；不以另一次裁尾、声纹分数、次数或等待时间代替完整口令。默认完整关键词的已有保留规则不变。
- Without enrollment, do not stall on the 1.1s owner-speech window.
- Firmware `voiceAutoStartEnabled` must be on for device VAD auto-start; settings UI:
  「检测到人声后自动开始」.

### Always-latest runtime gate (mandatory)

- 运行、日志采集和人工验收只允许使用当前 active tree 构建后安装到
  `C:\Program Files\Listener Type\listener-type.exe` 的版本；禁止从 sibling snapshot、旧桌面副本或旧快捷方式启动。
- 新程序安装、切换运行版本或发现旧进程时，核对所有 `listener-type` 进程的绝对路径、ProductVersion/FileVersion 和 SHA-256一次；允许同一哈希的主进程和 `--local-wake-helper` 子进程。相同运行版本的有效核对结果可复用，不因纯文档修改或重复采集而重装、重启、重复核对。发现旧进程、未知副本或哈希不一致时再修正，未通过核对不得称为“已修复”。
- 日志中的 session 只有在上述运行时核对通过后才可作为本次代码验收证据。
- 闸门命令：`pwsh -NoProfile -File .\scripts\verify_latest_runtime.ps1 -RequireRunning`。
