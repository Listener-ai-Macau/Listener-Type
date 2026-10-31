# Listener v1.0.6-rc1 · Type build242 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Type retries the original wake-recording cancellation when the initial Bluetooth control write fails, while protecting later recordings and firmware updates. It also improves recovery of the wake helper, bundles the CPU voice-activity runtime for fresh Windows installations, and reduces sensitive diagnostic logging. Raw recording retention remains off by default.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

The cancellation retry is bounded and applies to the original connection and wake request; it cannot guarantee recovery if the connection remains unavailable or changes. A delayed scoped cancellation was verified on a physical device using a separate BLE client. The actual intermittent timeout and Type retry were not reproduced end to end, and complete encrypted sniffer following was not qualified. Intermittent Bluetooth timeout, occasional wake failure, other-computer and long-term physical behavior remain under investigation. Review translations and wording suggestions against the original.

Translations can alter meaning or numbers and require review. Enhanced CPU translation requires optional components. Cloud recognition and wording services may incur separate charges. Long files use overlapping segments; check boundaries and unrecognized portions. Closing the file transcription window loses audio and resume progress, while copied or saved text remains. MP3/M4A depend on system codecs. Historical brownout causes and long live recordings with the actual owner remain unverified.

## File identity

- Type source: `ff927ccde203a4a7ae0ab0c9450f1031f48e0bc7` · `build-source/type242-20261031`
- MSI: 306548736 bytes · SHA256 `0668ba64f33a8cb8f325a364bcc26b696479625ecb81234dd56607a243f91227`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

本次更新在自动唤醒录音的取消指令暂时失败时，对原录音执行有限重试，同时保护后续录音及固件更新。改善唤醒辅助程序恢复，补齐新装机器的 CPU 语音活动检测组件，并减少敏感诊断日志。原始录音默认不保留。

蓝牙持续不可用或连接已更换时，重试不保证成功。实体设备已验证延迟取消，但通过独立蓝牙程序操作；尚未完整复现 Type 自身的真实超时与重试，抓包也未完整跟随。偶发蓝牙超时和唤醒失败的根因仍在排查。
