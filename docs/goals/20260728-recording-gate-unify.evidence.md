# Evidence：录音准入统一（RecordingGate）

Date: 2026-07-28

## 实现

- 新增 `src-tauri/src/coordinator/recording_gate.rs`
  - `RecordIntent`：HotkeyPress / DeviceKeyDictation / BleSessionStart / BlePcmIngest /
    BleBeginCandidateOrSession / ShowCapsule / BackgroundListener
  - `decide(snapshot, intent)` 纯策略；v1 = OTA 独占拒全部录音意图；Idle 类胶囊仍 Allow
  - `try_admit*` 统一日志前缀 `[recording-gate] deny ...`
- 入口改走 Gate：
  - `support.rs` emit_capsule
  - `dictation_session.rs` hotkey press
  - `hotkey_device_runtime.rs` device-key dictation
  - `dictation_embedded_stream.rs` Started / Pcm / begin_candidate_or_session
  - `embedded_ble_runtime.rs` background listener refresh / generation current / retry break
- `embedded_ble_ota_active` 仅保留在：状态机置位（try_begin/end）+ Gate 读取

## 机器证据

```text
cargo test recording_gate -- --nocapture
# 5 tests ok (policy + classifier + call-site source contract)
# Finished test profile; 906 filtered out
```

## 防回退

- OTA 中仍拒绝热键/设备键/BLE 开始与录音类胶囊
- suppress 路径可 emit Idle（Gate 放行非 recording-related capsule）
