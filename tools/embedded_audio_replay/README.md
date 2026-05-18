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

## 当前边界

这个工具只验证软件协议和 PCM 重组，不调用 ASR provider，也不连接真实 BLE 设备。进入 `P13.2` 时，需要选择可用的 `Listener-Type` ASR provider；进入真实设备验证时，需要 BLE 设备和硬件 gate。
