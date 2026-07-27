# Goal：Type 后端 soft>4500 清空（agent 满意闭环）

> Active Goal 合同（产品仓 `docs/goals/`）。承接 Phase2 已完成的目录化/include 拆分。

## 目标

在不改变听写/BLE/OTA/设置产品行为的前提下，把 `node scripts/check-module-budgets.mjs` 报告的 **`softOver4500` 列表清空**（生产 `.rs` 单文件 ≤4500 行，`*_tests.rs` 不计入），使 agent 可按职责文件定位修改。

## 满意标准（全部机器判定）

| # | 功能 | 成功状态 | 技术阈值 | 自动判定 |
|---|---|---|---|---|
| 1 | soft 清空 | 无生产文件 >4500 | 报告 `softOver4500: []` | 脚本 exit 0 且 JSON 中 soft 为空 |
| 2 | 硬预算 | 已登记文件 ≤ maxLines | `scripts/check-module-budgets.mjs` | exit 0 |
| 3 | 编译 | lib+tests 可编译 | `cargo check --manifest-path src-tauri/Cargo.toml --lib --tests` | exit 0 |
| 4 | 行为锁 | 关键单测绿 | `installed_takeover_evidence` `is_valid_local_pack_id` `embedded_ble_pcm_capsule_trace` `crc32_matches_standard_vector` | 全部 ok |
| 5 | 回归子集 | BLE/dictation/coordinator 不炸 | `cargo test --lib embedded_ble::`；`coordinator::dictation::`；`coordinator::tests::` | 0 failed |
| 6 | 地图 | 架构与目录一致 | `specs/ARCHITECTURE.md` | evidence 记录 |

## 范围

- **做**：`coordinator/dictation.rs`、`commands/mod.rs`（及必要的 include 兄弟）；预算脚本与结构测对齐。
- **不做**：改听写/BLE/OTA 业务语义；release 打包；固件。

## 机器闭环

预算脚本 + cargo check + 上表定向测试。失败只修边界/可见性/`include_str` 契约，不改产品语义。

## 人工边界

机器全部 PASS 后，owner 可选一次听写/BLE 冒烟。
