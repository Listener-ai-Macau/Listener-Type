# 实施证据 — Type 可维护性 Phase2

日期：2026-07-27

## 本轮交付

### A–B. commands/device + embedded_ble 目录化

见 commit `cc5c404`。

### C. windows_ble OTA + pairing include

见 commit `89c6e36`。

### D. windows_ble 再切 capture / notify_open / pnp / recording（本轮）

| 路径 | 约行数 |
|---|---|
| `windows_ble/mod.rs` | ~7614 |
| `windows_ble/pairing.rs` | ~2447 |
| `windows_ble/notify_open.rs` | ~1034 |
| `windows_ble/capture_events.rs` | ~644 |
| `windows_ble/ota_transfer.rs` | ~661 |
| `windows_ble/pnp_cache.rs` | ~485 |
| `windows_ble/recording_control.rs` | ~337 |

均经 `include!` 并入同一 module，行为不变。

## 机器结果

| 检查 | 结果 |
|---|---|
| `node scripts/check-module-budgets.mjs` | **PASS** |
| `cargo check --lib --tests` | **PASS** |
| Goal 关键 5 测 | **PASS** |
| `cargo test --lib embedded_ble::` | **114 passed; 0 failed; 4 ignored** |
| `node scripts/check-embedded-ble-processing-led.mjs` | **PASS** |

## soft>4500（仍 open）

- `coordinator.rs` ~9.2k
- `embedded_ble/windows_ble/mod.rs` ~7.6k
- `coordinator/dictation.rs` ~7.2k
- `commands/mod.rs` ~4.6k

## 下一刀

1. 压 `coordinator.rs` free-function / 子模块
2. 或继续削 `windows_ble/mod.rs`（settings/USB serial、OTA open 残余）
