# 实施证据 — Type 可维护性 Phase2

日期：2026-07-27

## 本轮交付：commands/device 按符号迁回

| 路径 | 约行数 |
|---|---|
| `commands/mod.rs` | ~4588 |
| `commands/device/mod.rs` | 10 |
| `commands/device/settings.rs` | ~1942 |
| `commands/device/ble.rs` | ~543 |
| `commands/device/firmware.rs` | ~2305 |
| `commands_tests.rs` | path 分离（`CARGO_MANIFEST_DIR` include） |

脚本：`tools/_migrate_commands_device.py`（按符号剥离 + 迁走类型的 `impl` + thin Tauri wrappers）。

## 机器结果

| 检查 | 结果 |
|---|---|
| `node scripts/check-module-budgets.mjs` | **PASS** |
| `cargo check --lib --tests` | **PASS** |
| `cargo test --lib installed_takeover_evidence` | ok |
| `cargo test --lib is_valid_local_pack_id` | ok (2) |
| `cargo test --lib embedded_ble_pcm_capsule_trace` | ok |
| `cargo test --lib crc32_matches_standard_vector` | ok |

## soft>4500（仍 open）

- `embedded_ble.rs` ~18k
- `coordinator.rs` ~9.2k
- `coordinator/dictation.rs` ~7.2k
- `commands/mod.rs` ~4.6k（已从 ~12.9k 压下；可再拆 marketplace/settings 段）

## 下一刀

1. **请先 commit 本批**（防 reset 再丢）
2. 拆 `embedded_ble`（`tools/_recovered_modules/embedded_ble/`）
3. 再拆 coordinator free-function 区
