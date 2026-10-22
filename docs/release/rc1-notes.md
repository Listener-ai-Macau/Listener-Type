# Listener v1.0.6-rc1 · Type build131 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Routine audio lifecycle diagnostics now use compact summaries instead of repeatedly serializing accumulated detail. This fixes records exceeding the logger limit and becoming incomplete JSON. Detailed bounded pipeline metadata remains available through the existing **Export diagnostic package** ZIP as `desktop/audio_pipeline.json`; an unavailable export is reported explicitly. No additional audio recording or transcript payload is added by this metadata export.

Recognition providers, wake/end decisions, streaming, clipboard behavior and Bluetooth parameters are unchanged. Build130's selected-recording/settings protection and Firmware33 remain. Installed131 produced92 valid pipeline records, maximum3861bytes in the observed window; this is evidence of repaired diagnostic output, not proof of faster perceived dictation. Populated full export and ZIP have domain-test evidence; the exact native export/file check used an empty private capture. Populated installed export remains unverified.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not include a ready-to-use official cloud account or allowance. File transcription accepts short16kHz/16-bit mono WAV through direct Volcengine. Review important words and optional rewriting.

RC1 is a prerelease. Strong background-speech separation remains incomplete and intermittent early-close cases remain under review. This update does not qualify new live-wake, automatic-ending, arbitrary external input, BLE throughput or physical-hardware behavior. USB flashing and Bluetooth OTA use the same firmware ZIP.

## File identity

- Type source: `fd872c301ba11f0a6bf22bfda4c3098246071678` · `build-source/type131-20261022`
- MSI: 299192320 bytes · SHA256 `8f893fbca6aad12cd87d04feea9e9ba633ac5e4bf2d8e4a7e40793fcbe56e013`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

131修复常规音频诊断日志过大导致JSON截断；完整有界元数据按需进入原诊断ZIP，不添加录音或转写正文。实际日用窗口92条均有效，最大3861字节；不据此宣称语音体验提速。语音服务、唤醒结束、流式、剪贴板与蓝牙参数均未改。保留原录音文件选择修复和Firmware33。49美元为纯硬件，云准备与用量另计；强干扰、偶发提前结束与其它未验范围继续保留。
