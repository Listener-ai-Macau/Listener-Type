# Embedded Audio Replay Tool

这个工具用于 `P13.1` 纯软件验证：把 `16 kHz / mono / 16-bit / PCM little-endian` 的音频 fixture 切成固件当前的 `VKA1` 会话包，再回放进 `SessionCollector`，输出可被自动化消费的 JSON stats。

## 用途

- 验证 `VKA1 session_start -> audio_data... -> session_stop` 解析和重组。
- 验证缺包补静音、`cancel`、`error`、session stats。
- 用 Windows 本机 TTS 生成 seeded 随机中文人声 WAV，减少每轮人工说话。
- 为后续 `Listener-Type` BLE host adapter 和 ASR 接入提供 fixture。

## 运行 replay

```powershell
cargo run --manifest-path tools\embedded_audio_replay\Cargo.toml -- `
  --input path\to\audio.wav `
  --format wav `
  --session-id 1000 `
  --payload-bytes 480
```

输出会包含：

- pretty JSON 报告
- 单行 `replay_result_json=...`，用于 pipeline/脚本提取结构化结果

## 运行流式 replay

```powershell
cargo run --manifest-path tools\embedded_audio_replay\Cargo.toml -- `
  --input path\to\audio.wav `
  --format wav `
  --mode stream `
  --session-id 1000 `
  --payload-bytes 480 `
  --terminal stop
```

流式模式会逐条 notification 调用 `StreamingSessionCollector`，报告 `started`、`pcm_chunk`、`stopped`、`cancelled`、`error` 和 `ignored` 事件。报告中的 `streaming` 字段会记录 chunk sequence、每包 PCM 字节数、streamed PCM hash、terminal event，以及是否能按流式 chunk 重组回输入 PCM。

`--terminal cancel` 和 `--terminal error` 可用于无硬件验证 cancel/error 收尾路径。

## Listener-Type 调试入口

桌面端保留 batch 入口，并新增对应的 streaming 入口。两组入口复用同一套 VKA1 parser、collector、ASR provider 和 coordinator 收尾逻辑：

```powershell
# batch file replay: 完整重组 PCM 后再进入 ASR
listener-type --submit-embedded-audio path\to\audio.wav

# streaming file replay: session_start 创建 ASR consumer，audio_data 到一包推一包
listener-type --submit-embedded-audio-stream path\to\audio.wav

# batch BLE: 等 stop/cancel/error 终止包后提交
listener-type --submit-embedded-audio-ble-once 120000

# streaming BLE: notification 到达即进入 coordinator streaming path
listener-type --submit-embedded-audio-ble-stream 120000
```

`--submit-embedded-audio-wav-stream` 和 `--submit-embedded-audio-pcm16le-stream` 可显式指定文件格式；不带格式时按现有 batch 入口规则推断。流式 BLE 入口仍需要设备在线，并按协作协议获取硬件资源锁后再跑真实 smoke。

## 自动 BLE 流式 smoke

真实设备 smoke 默认不要人工按 KEY1。优先使用串口控制命令模拟按键，并把录音窗口对齐到第二遍 TTS 播放：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\embedded_audio_replay\run_ble_stream_smoke.ps1 `
  -Port COM3 `
  -BluetoothAddress DCB4D91112CE `
  -VerifyHistory
```

脚本会：

1. 默认通过 COM3 RTS 脉冲 reset 设备；需要跳过时加 `-NoResetBeforeCapture`。
2. 调用固件仓库的 `ensure_ble_hid_connection.ps1` 预热 BLE。
3. 以隐藏主窗口模式启动 `listener-type --submit-embedded-audio-ble-stream 45000`，避免测试时把主界面弹到前台。
4. 提前打开串口日志监听，等到 Listener-Type 日志出现 `ValueChanged handler registered`，并记录固件侧 notify/transport 状态。
5. 播放第一遍随机中文 TTS 作为预热。
6. 在第二遍播放前向串口 helper 发信号，由 helper 通过 COM3 发送 `~VREC:TOGGLE` 开始录音，播放后自动发送一次停止，并保存固件串口日志。
7. 若第二遍播放后 20 秒内没有看到 `embedded audio streaming dictation started`，直接失败并输出 Listener-Type 日志和固件串口日志路径，避免长时间空等。
8. 开启 `-VerifyHistory` 时，脚本会检查 `history.json` 中本次记录带有 `embeddedAudioStats`；需要自动打开临时 Notepad 做光标落字检查时再加 `-VerifyInsertion`，人工观察当前光标时不需要。
9. 输出 `ble_stream_smoke_result_json=...`，包含原句、识别文本、插入目标、历史记录、PCM 字节数、缺包数、Listener-Type 日志和串口日志路径。

只有串口触发不可用、需要验证实体按键本身，或设备不在线时，才需要人工介入。默认配置按 60 秒内的快速 smoke 设计；需要长录音时可调 `-TimeoutMs`，需要更久等待首包时可调 `-NoNotificationTimeoutSeconds`。若要把缺包视为失败，加 `-FailOnMissingPackets`；默认有最终 ASR 文本但存在缺包时输出 `WARNING`，便于继续调查尾包 flush/停止时序。

### BLE stream smoke report schema

`run_ble_stream_smoke.ps1` writes the same JSON object to the timestamped report file and the single-line `ble_stream_smoke_result_json=...` output. The object includes `report_schema` so firmware-side matrix code can validate the contract without scraping this README.

Required fields are stable across PASS, WARNING, and FAIL reports:

| Field | Meaning |
| --- | --- |
| `status` | `PASS`, `WARNING`, or `FAIL`. |
| `trigger` | Trigger mode: `serial-toggle`, `serial-cancel`, or `manual-key`. |
| `audio_profile` | TTS/audio profile used for the run. |
| `expected_text` | Text the run intended ASR to produce. |
| `transcript` | Best transcript available after log/history fallback. |
| `final_text` | Final text used by accuracy and insertion checks. |
| `partial_preview_count` | Count of non-final ASR text updates observed before final text. |
| `last_partial_preview` | Last partial preview before the final transcript, or empty string. |
| `asr_text_update_count` | Number of distinct ASR text updates parsed from logs. |
| `asr_text_updates` | Ordered list of parsed ASR text updates. |
| `inserted_text` | Text read from the temporary insertion target when `-VerifyInsertion` is used; otherwise null. |
| `history_session` | Matched history session object when available; null when not requested or not found. |
| `recording_archive_path` | Captured embedded recording archive path when one was written. |
| `timeline` | UTC timestamps for major smoke phases. |
| `started_at_utc` | UTC start time for this smoke run. |
| `log_path` | Listener-Type log captured for this run. |

Optional fields are present when the corresponding subsystem participates: `history_session.embeddedAudioStats`, `history_session.insertStatus`, `serial_report`, `serial_log_path`, `insertion_target_path`, `expected_stream_failure`, and `error`.

Diagnostic fields support quality gates and debugging: `normalized_expected`, `normalized_transcript`, `cer`, `accuracy`, `accuracy_threshold`, `accuracy_warning_only`, `accuracy_warning`, `accuracy_warning_message`, `wav_path`, `tts_rate`, `tts_gain`, `random_sentence_count`, `pcm_bytes`, `missing_packets`, and `verification_errors`. Non-warning profiles fail the smoke when `accuracy` falls below `accuracy_threshold`; warning-only profiles report `WARNING`.

## 生成随机 TTS fixture

```powershell
pwsh -NoProfile -File tools\embedded_audio_replay\generate_tts_fixtures.ps1 `
  -Seed 20260518 `
  -Count 6 `
  -OutDir artifacts\embedded_audio_tts `
  -PayloadBytes 480
```

脚本会：

1. 根据 seed 生成中文随机句子。
2. 用 Windows 内建 `System.Speech` 合成 `16k mono 16-bit` WAV。
3. 调用 replay 工具验证每条 WAV。
4. 写出 `manifest.seed<seed>.json`。
5. 输出单行 `tts_fixture_result_json=...`。

## 识别准确度报告

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\embedded_audio_replay\measure_asr_accuracy.ps1 `
  -Seed 20260518 `
  -Count 6 `
  -OutDir artifacts\embedded_audio_accuracy
```

脚本会复用 seeded TTS fixture，逐条调用 `tools/volcengine_asr_probe`，并在同一个目录写出 `accuracy.seed<seed>.json`。报告会记录原句、ASR transcript、简繁/标点/空白归一化后的文本、每句 CER、平均 CER 和最大 CER。

输出会包含单行 `asr_accuracy_result_json=...`，用于 pipeline 提取报告路径和汇总状态。高 CER 会返回 `WARNING` 但不让脚本失败；ASR 调用失败或空 transcript 会返回 `FAIL` 并以非零状态退出。

## 当前边界

`generate_tts_fixtures.ps1` 只验证软件协议和 PCM 重组，不调用 ASR provider，也不连接真实 BLE 设备。`measure_asr_accuracy.ps1` 会调用本机已配置的火山 ASR provider；进入真实设备验证时，需要 BLE 设备和硬件 gate。
