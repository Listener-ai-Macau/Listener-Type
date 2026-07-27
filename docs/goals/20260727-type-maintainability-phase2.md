# Goal：Listener Type 后端可维护性达到 agent 满意标准

> Active Codex Goal 合同（本环境无 `create_goal` 工具；合同落在产品仓 `docs/goals/`）。

目标：在不改变听写/BLE/OTA/设置产品行为的前提下，把 Listener Type Rust 后端拆成 AI 可导航模块；**agent 满意标准** = （1）关键大模块测试 path 分离（2）`npm run check:module-budgets` PASS（3）`cargo check --lib --tests` PASS（4）关键单测绿（5）ARCHITECTURE 地图与目录一致（6）soft>4500 列表持续压降并有下一刀边界文档。完全清空 soft 列表为同一 Goal 的后续迭代，不因 git reset 丢失进度。

技术指标：

| 功能 | 成功状态 | 技术阈值 | 自动判定 |
|---|---|---|---|
| 预算门禁 | 已登记文件在硬预算内 | 见 `scripts/check-module-budgets.mjs` | 脚本 exit 0 |
| 测试分离 | coordinator / dictation 测试独立文件 | 主文件无 `mod tests {` 大块 | 同上 SEPARATED_TESTS |
| 编译 | lib+tests 可编译 | `cargo check --manifest-path src-tauri/Cargo.toml --lib --tests` | exit 0 |
| 行为锁 | 关键单测不回归 | `installed_takeover_evidence`、`is_valid_local_pack_id`、`embedded_ble_pcm_capsule_trace`、`crc32_matches_standard_vector` | 全部 ok |
| 地图 | 架构说明可用 | `specs/ARCHITECTURE.md` 与目录一致 | 人工抽查 |
| soft 压降 | 生产文件 >4500 有登记与下一步 | 报告 `softOver4500` 可解释 | 报告存在且 evidence 记录 |

机器闭环：预算脚本 + cargo check + 定向 cargo test；失败只修边界/可见性，不改业务语义。

人工边界：机器 PASS 后 owner 可选听写/BLE 冒烟。
