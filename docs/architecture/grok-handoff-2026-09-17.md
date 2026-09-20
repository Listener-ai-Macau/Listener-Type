# Listener 当前接手手册（Grok）

更新时间：2026-09-17  
适用范围：Listener-Type、Listener-Firmware，以及当前实机证据目录。  
当前结论：候选仍是 NO-GO，本文是接手资料，不是发布证明。

## 0. 接手后先做什么

1. 先读本文件和 `C:\Users\Billy\Documents\Codex\2026-09-12\c-users-billy-desktop-denzic-listener\coordination-model.md`。
2. 在 Type 仓库执行只读的 `git status`、`git diff --stat`、`git diff`，把现有 dirty/untracked 工作分层；禁止 reset、checkout、覆盖或删除历史工作。
3. 以本文件列出的证据为起点，不把旧文档的“通过”、单元测试通过或旧安装包 hash 当作当前候选通过。
4. 先解决当前“实际光标没有可靠上屏证明”的闭环，再按 A–H 顺序推进；每轮只形成一个有边界的候选和一轮有目的的实机证据。

## 1. 分工和决策权

- Grok/当前接手执行者：审查并保留现有 diff、修改代码、做必要的最小验证、构建并安装同一候选、采集实机日志和回报结果。
- Astra：只负责整体规划、阶段顺序、风险、验收门槛和方案性复核；需要时沿用现有 Astra，不开启 Luna 或其他子代理，不让 Astra 直接改主工作树。
- 用户：只在明确的一次实机操作中配合；需要真实第二说话人时，扬声器回放只能做定位，不能冒充真人第二说话人验收。

执行者不能自行把局部修补宣布为 A–H 完成，也不能用代码可读性、测试数量或一次偶然成功替代实机证据。

## 2. 固定路径和候选身份

- Type：`C:\Users\Billy\Desktop\Denzic\Listener\Listener-Type`
- Firmware：`C:\Users\Billy\Desktop\Denzic\Listener\Listener-Firmware`
- 证据根目录：`C:\Users\Billy\Documents\Codex\2026-09-12\c-users-billy-desktop-denzic-listener\work`
- 分工记忆：`C:\Users\Billy\Documents\Codex\2026-09-12\c-users-billy-desktop-denzic-listener\coordination-model.md`
- 当前分支：`use/1.0.5-good`
- 当前 HEAD：`e347c43`

当前工作树有大量既有修改和未跟踪文件，全部属于接手时的历史工作，必须保留。最近一次目标上屏修复构建在用户要求暂停后中断；没有新的安装候选。已知旧安装 exe hash 为：

`7D2AC7F42500B14BE0BEC91212106F1B9DD4A393C3837A2E1169C8A804F5769D`

这个 hash 只能标识旧候选，不能用来证明本轮源代码已经安装。

## 3. 当前问题合同：A–H

- A：唤醒不了或唤醒延迟。
- B：误唤醒。
- C：提前自动结束，后半句被截断或吞掉。
- D：停止后不能自动结束，或会话卡住。
- E：干扰下吞掉目标说话人的文字。
- F：把第二说话人/电视等旁人的话输入到目标文本。
- G：大段预览更新不流畅、滞后或滚动窗口重复。
- H：开启去除语气词后吞掉句中、句末或括号/引号标点。

旧文档中 A–G 的定义可能不同，以这里和最新 `coordination-model.md` 的 A–H 定义为准。真实第二说话人/电视抗干扰仍是 E/F 的发布验收门槛；`reset_reason` 仍未验证。

## 4. 当前整体状态

### 已有正向证据，但不能合并成“全部通过”

- r11 有一次真人手动窗口显示：会话能自动停止、传输完整、history 有文本，最终文本语义基本完整。
- `volcengine_untimed_merge` 的真实形状回归测试已经通过，目标是防止同一增长中的滚动窗口被重复追加，同时保留真实下一句续写。
- 受影响的 Rust 定向测试和前端构建曾通过；这只说明代码可编译/局部行为符合测试，不等于当前安装候选实机通过。

### 当前仍未闭合的核心问题

- r10 出现提前截断；r11/r12 的正向结果不足以证明同一新候选已经连续稳定。
- r12 的 history/final 记录为 `InsertStatus=Inserted`，但日志同时为 `target_confirmed=false`，且最终前台是 PowerShell/Administrator；实际光标收到的文字没有被可靠证明。
- trace PCM 为 951616 bytes，而 BLE complete 为 896000 bytes，区间差异仍未解释；在解释前不要调整唤醒阈值、声纹阈值、固件或 endpoint。
- A/B 的同一候选、同一来源、当前用户完整唤醒词闭环证据不完整；E/F 的真实第二说话人/电视验收未完成；H 没有当前候选的实机对照。
- 没有新的安装包，不能继续拿旧安装包做“最新修复”结论。

## 5. 关键证据索引

### 固定失败 trace

`C:\Users\Billy\AppData\Roaming\Listener Type\recordings\959d2caa-e1f0-4665-b798-f2bc36880059.asr-trace.json`

已知事实：frame 20→21 的 `fallback_partial_callback` 把同一滚动窗口重复拼接；trace PCM 与 BLE complete 长度不一致；原先“未上屏”还受 `allowNonTsfInsertionFallback=false` 影响。不能仅靠这条 trace 宣布修复。

### 当前实机回放

- `work\human-endpoint-auto-stop-r10-20260917`：旧候选提前截断；final 在开放句中结束，STOP 后不久仍出现 `PendingSpeech`。失败。
- `work\human-endpoint-auto-stop-r11-human-1-20260917`：一次真人窗口自动停止和文本链路为正向证据，但不是三轮同候选完整验收。
- `work\human-endpoint-auto-stop-r12-human-target-20260917`：history/final 有文本，`route=unicode`、`target_confirmed=false`；前台窗口为 PowerShell，实际光标上屏未证明。上屏验收失败/未闭环。

交接者必须同时核对：session id、provider raw、speaker-filtered、preview/recovery、final、history、实际目标控件读回、`InsertStatus`、停止时序和 PCM/BLE 区间，不能只看 history。

## 6. 当前未提交代码变更（必须先审查）

最近一轮目标上屏修复涉及：

- `src-tauri/src/types.rs`、`src/lib/types.ts`、`src/pages/History.tsx` 和多语言：新增 `SubmittedUnconfirmed`，明确“事件已提交但没有确认实际渲染”。
- `src-tauri/src/coordinator/support.rs`：允许按会话绑定的根 HWND 获取 IME 提交目标，不再只依赖当前前台窗口。
- `src-tauri/src/coordinator.rs`：非 TSF/Unicode/流式路径不再把“已发送事件”冒充 `Inserted`；未确认时标为 `SubmittedUnconfirmed`。
- `src-tauri/src/coordinator/dictation.rs`：OriginalTarget 发送前重新恢复并检查目标窗口；目标快照与 delivery 绑定；恢复失败时拒绝向错误前台插入。
- `src-tauri/src/coordinator/dictation_wake_polish.rs` 和相关测试：未确认上屏不能显示为确认成功。

这些修改尚未经过新的安装候选和真实光标验收。接手者必须检查它们是否会影响“真实下一句续写”、流式回退和历史文本，不能因为状态名更诚实就直接判定问题已修好。

上一轮既有修改也必须保留并审查，尤其是：

- `src-tauri/src/asr/volcengine_untimed_merge.rs` 的滚动窗口替换与真实形状测试；
- `src-tauri/src/coordinator/dictation.rs` 的 provider 无正文时 preview 恢复来源；
- `src-tauri/src/coordinator/dictation_preview.rs` 删除按字符长度覆盖 final 的逻辑；
- product final / speech decision 相关测试期望。

## 7. 已知验证结果

最近一次成功的局部验证包括：

- `cargo test --locked --lib delivery_dispatch_policy_maps_platform_branches -- --nocapture`：1 passed。
- `cargo test --locked --lib done_message -- --nocapture`：2 passed。
- `cargo test --locked --lib target_speaker_endpoint -- --nocapture`：63 passed。
- `npm run build`：通过。
- `git diff --check`：无 whitespace error；存在换行格式提示。

之后的 `npm run tauri build -- --target x86_64-pc-windows-msvc` 被中断，不能把中断前的编译过程当成构建成功。除非重新构建、安装并核对同一候选身份，否则不要进行新的实机发布判断。

## 8. 推荐执行顺序

### 阶段 1：接手审计和候选身份

审查 git diff、当前安装进程、旧 hash、未跟踪文件和现有证据；确认当前源码中的 `SubmittedUnconfirmed`、目标 HWND 绑定和 preview/merge 变更没有互相覆盖。只跑能直接验证这条边界的最小测试，不跑无新增信息的 Rust 全量。

### 阶段 2：构建同一候选并验证真实目标上屏

重新构建并安装；记录 raw exe、安装 exe、MSI payload hash 和 runtime gate。使用原成熟弹窗/真实目标控件，不新造记事本，不把 PowerShell/终端当目标。一次会话内记录：准备时目标、唤醒时目标、final 时目标、delivery 时目标、实际控件读回和 history。

上屏只有在以下条件同时成立时才算通过：

`Inserted` + `target_confirmed=true` + history/final/实际目标文本一致 + session id 一致。

`SubmittedUnconfirmed` 是失败/未确认，不是成功的近似名称。

### 阶段 3：先收敛 C/D，再推进 E/F/G/H

在同一候选上验证连续说话不提前结束、目标说话人停止后约 1 秒自动结束、final 不缺句不重复；然后再用可追溯的本人/第二说话人来源处理 E/F，检查长段 preview 的增量连续性，最后做 H 的去语气词与标点对照。每个阶段只接受对应的日志和实机证据。

### 阶段 4：发布前综合门槛

至少保留目标上屏、停止时序、history、InsertStatus、trace/PCM、固件 `reset_reason` 和真实第二说话人/电视干扰证据。任一整句缺失、重复、旁人插入、错目标、错误自动结束、无法自动结束、卡死或证据无法解释，候选失败或 UNVERIFIED，不能发布。

## 9. 固定禁区

- 不为通过测试制造特例。
- 不在 PCM 区间解释前调整阈值、声纹、firmware、endpoint 或放宽准入语义。
- 不把扬声器播放同一录音当作真实第二说话人通过。
- 不把 history 或 `InsertStatus=Inserted` 当作实际光标上屏的充分证明。
- 不把旧成功版本、旧安装 hash 或局部单测与当前候选混合成一份发布证据。
- 不因一次成功就覆盖失败证据；不清空会话，不删除历史工作。

## 10. 接手回报格式

每次只回报四项：

1. 当前候选身份：commit、构建时间、raw/installed hash、runtime gate。
2. 本轮针对的 A–H 项和最早失败分叉。
3. 代码/测试/实机结果及证据绝对路径。
4. 明确的 PASS、FAIL、UNVERIFIED 或 EXTERNAL_BLOCKED，以及下一张执行卡。

在 A–H、实际目标上屏、PCM 区间、固件 reset reason 和真实干扰证据全部闭合前，状态保持 NO-GO。
