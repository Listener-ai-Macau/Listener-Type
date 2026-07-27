# 实施证据 — Type 可维护性 Phase2

日期：2026-07-27

## 本轮交付

### A. commands/device 按符号迁回（已 commit 前序）

| 路径 | 约行数 |
|---|---|
| `commands/mod.rs` | ~4589 |
| `commands/device/{mod,settings,ble,firmware}.rs` | 薄入口 + 职责文件 |

### B. embedded_ble 目录化 + 测试 path 分离（commit `cc5c404`）

| 路径 | 约行数 |
|---|---|
| `embedded_ble/mod.rs` | ~1535 |
| `embedded_ble/mod_tests.rs` | ~1.1k |
| `embedded_ble/windows_ble_tests.rs` | ~2.0k |

### C. windows_ble 再拆 OTA + pairing（本轮续）

| 路径 | 约行数 | 说明 |
|---|---|---|
| `embedded_ble/windows_ble/mod.rs` | ~10098 | GATT/notify/capture 主干 + `include!` |
| `embedded_ble/windows_ble/ota_transfer.rs` | ~661 | OTA prepare/transfer/probe |
| `embedded_ble/windows_ble/pairing.rs` | ~2447 | pairing/unpair/AEP discovery |

用 `include!` 保持同一 module 作用域，避免 `pub(super)` 可见性爆炸。

## 机器结果

| 检查 | 结果 |
|---|---|
| `node scripts/check-module-budgets.mjs` | **PASS** |
| `cargo check --lib --tests` | **PASS** |
| Goal 关键单测（5） | **5 passed** |
| `cargo test --lib embedded_ble::` | **114 passed; 0 failed; 4 ignored** |
| `node scripts/check-embedded-ble-processing-led.mjs` | **PASS** |

## soft>4500（仍 open）

- `embedded_ble/windows_ble/mod.rs` ~10.1k（已从 ~18k 单体 / ~13k 单文件压下）
- `coordinator.rs` ~9.2k
- `coordinator/dictation.rs` ~7.2k
- `commands/mod.rs` ~4.6k

## 下一刀

1. 再拆 `windows_ble/mod.rs`（notify capture / open_target / settings）
2. coordinator free-function 区 / dictation 子模块压 soft
