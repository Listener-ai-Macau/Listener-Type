# Embedded BLE Dictation Quality Hardening

日期：2026-05-19
计划：`listener-type-quality-hardening`

## 做了什么

### 1. Transcript 合并模块抽离

`src-tauri/src/asr/volcengine.rs` 从 1790 行拆为两个文件：

- **volcengine.rs** (895 行) — WebSocket session、音频发送、连接管理
- **volcengine_transcript.rs** (910 行) — 纯函数：transcript 解析、合并、去重

抽离的纯函数：`normalized_result`、`transcript_candidate_from_result`、`transcript_text_from_result`、`choose_transcript_text`、`merge_streaming_transcript`、`merge_streaming_candidate`、`is_unstable_initial_partial` 等 20+ 函数。

新增 10 个 table tests 覆盖：标点差异、空文本 fallback、单字符边界、JSON 数组解析等场景。总计 30 个 transcript 测试。

无 utterance 时间戳时，`volcengine_untimed_merge.rs` 额外跟踪当前滚动窗口：第一次窗口移动按重叠续接，同一窗口的后续修订只替换尾窗，防止长句预览和最终插入重复膨胀；不重叠的新句仍正常追加。

### 2. BLE Smoke Report Schema

Tai 固化了 `run_ble_stream_smoke.ps1` 输出字段分类（required / optional / diagnostic），文档在 `tools/embedded_audio_replay/README.md`。

### 3. 胶囊 Partial Preview 规则

新建 `src/lib/capsulePreviewRules.ts`，集中定义：

- **截断规则**：`truncatePreview()` + `PREVIEW_MAX_CHARS`（win: 28/34, mac: 14/18）
- **去重策略**：`PREVIEW_DEDUP_POLICY = 'exact-match'`（Rust 端 `update_embedded_audio_partial_preview`）
- **状态过渡**：`PREVIEW_FINAL_TRANSITION`（previewStates、finalState、exitAnimMs）
- **布局稳定性**：`LAYOUT_RULES`（固定高度，processing 允许 2 行 wrap）

Capsule.tsx 已改用共享常量，不再有硬编码魔法数字。

### 4. 格式化 + Warning 清理

7 个 compiler warning 修复：`cfg` 平台门控（cache.rs、test_run.rs、coordinator.rs）。

## 代码在哪

| 变更 | 文件 |
|------|------|
| Transcript 合并纯函数 | `src-tauri/src/asr/volcengine_transcript.rs` |
| 无时间戳滚动窗口合并 | `src-tauri/src/asr/volcengine_untimed_merge.rs` |
| Transcript 合并测试 | 同上 `#[cfg(test)] mod tests` |
| Volcengine session（瘦身） | `src-tauri/src/asr/volcengine.rs` |
| 胶囊 Preview 规则 | `src/lib/capsulePreviewRules.ts` |
| 胶囊 Preview 规则测试 | `src/lib/capsulePreviewRules.test.ts` |
| Capsule 组件（引用规则） | `src/components/Capsule.tsx` |

## 如何验证

```bash
cargo fmt --check   # 零差异
cargo check         # 零 warning
npm run build       # 前端构建通过
```

## 测试/调试环境变量

| 变量 | 用途 | 默认值 | 测试专用 |
|------|------|--------|----------|
| `LISTENER_TYPE_ACCEPT_SYNTHETIC_HOTKEY_EVENTS` | 允许接受注入的热键事件 | 未设置 | 是 |
| `LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN` | 热键注入只发事件不触发录音 | 未设置 | 是 |
| `LISTENER_TYPE_DEBUG_TRANSCRIPT_FILE` | 将 transcript 写入指定文件路径 | 未设置 | 是 |
| `LISTENER_TYPE_DISABLE_BACKGROUND_BLE` | 禁用后台 BLE 监听 | 未设置 | 是 |
| `LISTENER_TYPE_SHOW_MAIN_ON_START` | 启动时强制显示主窗口 | 未设置 | 否 |
| `LISTENER_TYPE_HIDE_MAIN_ON_START` | 启动时隐藏主窗口 | 未设置 | 否 |
| `LISTENER_TYPE_CODEX_AUTH_PATH` | Codex 认证文件路径 | 默认 ~/.codex | 否 |
| `LISTENER_TYPE_IME_DLL_X64` | MSI 打包用的 x64 IME DLL 路径 | 未设置 | 否 |
| `LISTENER_TYPE_IME_DLL_X86` | MSI 打包用的 x86 IME DLL 路径 | 未设置 | 否 |

## 已知限制

- Windows test binary 有 `STATUS_ENTRYPOINT_NOT_FOUND`，Rust 单元测试无法在当前 Windows 环境运行。编译验证通过，但运行时测试受限。
- Transcript 合并逻辑假设 Volcengine 协议不变；如果服务端行为变更，可能需要调整去重/合并阈值。
