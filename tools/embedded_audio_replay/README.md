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
