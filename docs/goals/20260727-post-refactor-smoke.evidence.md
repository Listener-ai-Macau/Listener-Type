# 机器回归证据 — 重构后（1/2/3 收尾）

日期：2026-07-27  
树：`2b2d8d0`（含 Phase3 soft 清空 + import 清理 + Chip/LED 修）

## 1. 固化

- 删除工作区 junk：`cargo_ble_fail.txt`、`embedded_ble.rs.bak-split`、`tools/_debug_ble_braces.py`、`tools/_fix_split_refs.py`
- 本地 commits 已 push 至 `origin/master`（见本会话 `git push` 输出）

## 2. 机器回归（当前工作树）

| 检查 | 结果 |
|---|---|
| `node scripts/check-module-budgets.mjs` | PASS，`softOver4500: []` |
| `node scripts/check-embedded-ble-processing-led.mjs` | PASS |
| `cargo check --lib --tests` | PASS（修 Chip re-export 后） |
| Goal 关键测（takeover / pack_id / pcm_trace / crc32） | PASS |
| `cargo test --lib embedded_ble:: coordinator::dictation::` | **196 passed** |
| `cargo build --lib`（debug） | PASS，artifact 在 `src-tauri/target/debug/` |

### 未做 / 需 owner

- **GUI 真人冒烟**（听写起停、BLE 录音、OTA、无声纹唤醒）：需插硬件 + 跑 **当前树** `npm run tauri -- dev` 或刚编 debug/release，不能用旧 MSI。
- `src-tauri/target/debug/listener-type.exe` 时间戳可能早于本批 lib 构建；体验验收请用 `tauri dev` 或完整 `tauri build --debug`。

## 3. warnings 清理

- `cargo fix` + 手工去掉 commands/device 残留 unused import
- 恢复 `Manager`（`AppHandle::state`）
- `pub use Chip` 供 `commands_tests` 有线 flash 测试

## 人工 gate 清单（可选）

1. 最新 `tauri dev` 启动 Type  
2. 听写一次 / Esc 取消  
3. BLE 连上并按键录音  
4. 设置里确认无声纹时唤醒仍可进 verification  
5. （可选）OTA 一次  
