# 实施证据 — Type 后端 AI 可维护性

日期：2026-07-27

## 机器结果

| 检查 | 结果 |
| --- | --- |
| `node scripts/check-module-budgets.mjs` | PASS |
| `cargo check --manifest-path src-tauri/Cargo.toml --lib --tests` | PASS |
| `cargo test --lib installed_takeover_evidence` | ok |
| `cargo test --lib is_valid_local_pack_id` | ok (2) |
| `cargo test --lib embedded_ble_pcm_capsule_trace` | ok |
| `cargo test --lib crc32_matches_standard_vector` | ok |

## 结构交付

- `embedded_ble` 目录化：`mod.rs` + `windows_ble/{mod,tests}.rs` + 顶层 `tests.rs`
- `commands/` 目录化：`mod.rs`（设置/风格/ASR/市场 + 薄 Tauri 包装）+ `device/{mod,settings,ble,firmware}.rs`
- `commands_tests.rs` / `coordinator_tests.rs` / `coordinator/dictation_tests.rs` / `coordinator/support.rs`
- `scripts/check-module-budgets.mjs` + `npm run check:module-budgets`
- Goal 合同：`docs/goals/20260727-type-maintainability-excellent.md`

## 本轮（commands/device 再拆）补充

| 检查 | 结果 |
| --- | --- |
| `cargo check --lib --tests` | PASS |
| `node scripts/check-module-budgets.mjs` | PASS |
| `commands/mod.rs` | ~4424 行（≤5200 预算） |
| `commands/device/settings.rs` | ~1943 行（≤2800） |
| `commands/device/ble.rs` | ~544 行（≤1200） |
| `commands/device/firmware.rs` | ~2306 行（≤2800） |

Import 约束：`device/*` **禁止** `use super::*` / 经 `commands` 再导出的 glob，避免 rustc 循环解析 OOM；仅显式导入父模块符号（如 `CoordinatorState`、`persist_settings`）与 sibling（如 `settings → ble`）。

## 已知未完成（下一阶段）

以下文件仍 >4500 行（soft list），需按职责域再拆且保持编译：

- `embedded_ble/windows_ble/mod.rs` ~13k（配对/捕获/OTA 交织，禁止盲切行号）
- `coordinator.rs` ~9.2k
- `coordinator/dictation.rs` ~7.2k

当前阶段「优秀」定义为：**测试与生产分离 + 模块地图 + 机器预算门禁 + 编译/关键单测绿**。
进一步把单文件压到 ≤4500 属于同一 Goal 的后续迭代。
