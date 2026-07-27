# Goal：Listener Type 后端 AI 可维护性达到优秀

> Active Codex Goal 合同（本环境无 `create_goal` 工具；合同落在产品仓 `docs/goals/`，与 quickstart 分离）。

目标：把 Listener Type Rust 后端从「上帝文件」拆成 AI 可导航模块，使后续 agent 在不读全库的情况下能定位、修改、验证单个职责域；不改变听写/BLE/OTA/设置产品行为。

技术指标：

| 功能 | 成功状态 | 技术阈值 | 自动判定 |
|---|---|---|---|
| 生产源文件体积 | 核心后端生产 `.rs` 单文件不超过预算 | `coordinator.rs`/`commands.rs`/`embedded_ble/windows_ble*.rs` 生产代码（不含 `*_tests.rs`）单文件 ≤ 4500 行；`dictation.rs` 生产代码 ≤ 4500 行 | `node scripts/check-module-budgets.mjs` 退出码 0 |
| 测试与生产分离 | 大型模块的 `mod tests` 不混在生产主文件尾部 | `commands`/`coordinator`/`embedded_ble`/`dictation` 主测试模块为独立文件 | 同上脚本检查；路径存在且主文件不再包含 `mod tests {` 大块（允许 `#[path=...] mod tests;`） |
| 编译闭环 | 拆分后库与测试可编译 | `cargo check --manifest-path src-tauri/Cargo.toml --lib --tests` 成功 | 退出码 0 |
| 行为锁 | 关键 source-structure / 单元测试不回归 | 至少运行：`installed_takeover_evidence`、`crc32_matches_standard_vector`、`is_valid_local_pack_id`、`embedded_ble_pcm_capsule_trace` 及 dictation 相关已有单元测试子集 | 全部 ok |
| 可追溯 | 新文件进入 traceability | `specs/traceability/files.md` 含新增模块路径 | `npm run check:traceability` 或脚本写回后 diff 干净意图 |
| 架构说明 | agent 有一张地图 | `specs/ARCHITECTURE.md` 描述 embedded_ble/、coordinator/support、commands 测试拆分 | 人工抽查文档与目录一致 |

机器闭环：

- 输入/触发：`node scripts/check-module-budgets.mjs`；`cargo check --manifest-path src-tauri/Cargo.toml --lib --tests`；定向 `cargo test --lib <filter>`。
- 可观察结果：脚本 JSON/文本报告、cargo 输出、文件行数。
- 自动判定：预算脚本与 cargo 退出码均为 0；FAIL 必须回修模块边界/可见性/include 路径，不得改业务语义绕过。

人工边界：仅在机器判定全部 PASS 后，owner 可选做一次日常听写/BLE 冒烟确认体验无回归；人工 FAIL 回到修复，不替代机器闭环。
