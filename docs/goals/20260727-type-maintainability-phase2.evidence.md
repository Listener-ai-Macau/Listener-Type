# 实施证据 — Type 可维护性 Phase2

日期：2026-07-27

## 本轮交付

### A. commands/device 按符号迁回（前序）

| 路径 | 约行数 |
|---|---|
| `commands/mod.rs` | ~4589 |
| `commands/device/mod.rs` | 11 |
| `commands/device/settings.rs` | ~1943 |
| `commands/device/ble.rs` | ~544 |
| `commands/device/firmware.rs` | ~2306 |
| `commands_tests.rs` | path 分离 |

### B. embedded_ble 拆目录 + 测试 path 分离（本轮续）

| 路径 | 约行数 | 说明 |
|---|---|---|
| `embedded_ble/mod.rs` | ~1535 | 共享类型、公开包装、非 Windows stub |
| `embedded_ble/windows_ble.rs` | ~13198 | Windows GATT/配对/OTA/采集生产代码 |
| `embedded_ble/mod_tests.rs` | ~1113 | `#[path]` 从 `mod.rs` |
| `embedded_ble/windows_ble_tests.rs` | ~2204 | `#[path]` 从 `windows_ble.rs` |
| `embedded_ble.rs`（单文件） | 删除 | 备份 `embedded_ble.rs.bak-split` |

脚本：

- `tools/_split_embedded_ble.py`
- `tools/_fix_embedded_ble_include_paths.py`
- `tools/_extract_windows_ble_tests.py`
- `tools/_extract_embedded_ble_mod_tests.py`

预算门禁已登记：`embedded_ble/mod.rs` ≤2000、`windows_ble.rs` ≤14000；`SEPARATED_TESTS` 含两者。

## 机器结果

| 检查 | 结果 |
|---|---|
| `node scripts/check-module-budgets.mjs` | **PASS** |
| `cargo check --lib --tests` | **PASS** |
| Goal 关键单测（5） | **5 passed** |
| `cargo test --lib embedded_ble::` | **114 passed; 0 failed; 4 ignored** |

结构测试已按拆分后布局对齐：path 分离后不再依赖同文件 `mod tests {` 边界；wrapper 层用 Windows 实现体锁定；嵌套模块去缩进后的空白契约已更新。

## soft>4500（仍 open）

- `embedded_ble/windows_ble.rs` ~13.2k（已从单体 ~18k 降；下一步可按 OTA / capture / pairing 再切）
- `coordinator.rs` ~9.2k
- `coordinator/dictation.rs` ~7.2k
- `commands/mod.rs` ~4.6k

## 下一刀

1. **请先 commit 本批**（含 embedded_ble 目录拆分 + 测试分离 + budget/ARCHITECTURE/evidence）
2. 再拆 `windows_ble` 职责域（OTA write_control / notify capture / pairing）
3. coordinator free-function 区 / dictation 子模块压 soft
