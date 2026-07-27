# 实施证据 — Type 可维护性 Phase2

日期：2026-07-27

## 本轮交付

### A–D. commands / embedded_ble / windows_ble include 拆分

见 commits：`cc5c404`、`89c6e36`、`6312788`。

### E. coordinator free-function 拆分（本轮）

| 路径 | 约行数 | 说明 |
|---|---|---|
| `coordinator.rs` | ~3709 | 主类型 + `impl Coordinator` + ASR/polish 尾部 |
| `coordinator/hotkey_device_runtime.rs` | ~2009 | 热键 supervisor + device-key BLE pending |
| `coordinator/embedded_ble_runtime.rs` | ~3485 | 背景 listener / 配对恢复 free functions |

`include!("coordinator/…")` 保持同一 module 作用域。

## 机器结果

| 检查 | 结果 |
|---|---|
| `node scripts/check-module-budgets.mjs` | **PASS** |
| `cargo check --lib --tests` | **PASS** |
| Goal 关键 5 测 | **PASS** |
| `cargo test --lib coordinator::tests::` | **144 passed** |
| `cargo test --lib embedded_ble::` | **114 passed** |

## soft>4500（仍 open）

- `embedded_ble/windows_ble/mod.rs` ~7.6k
- `coordinator/dictation.rs` ~7.2k
- `commands/mod.rs` ~4.6k

`coordinator.rs` 已从 soft 列表压下（~9.2k → ~3.7k）。

## 下一刀

1. 压 `dictation.rs` 或 `windows_ble/mod.rs` 残余
2. `commands/mod.rs` 再拆 marketplace/settings 段
