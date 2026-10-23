# Listener v1.0.6-rc1 · Type build139 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Computer microphone error monitors now keep the identity of the recorder that created their error channel. They no longer take the identity of a newer recording when startup finishes late. Error cleanup checks and claims that same identity under one state lock, preventing another replacement between a separate check and abort. Genuine errors still terminate their own active recording.

The original production error-monitor counterexample changed an unrelated Starting recording to Idle before the fix and preserves it afterward. Related checks cover Starting/Listening, replacement during cleanup and matching-owner error handling. No service credentials, models, wake thresholds, Bluetooth or firmware are changed.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

RC1 is a prerelease. Strong background-speech separation remains incomplete. Older intermittent early-ending and arbitrary target-field scopes remain open. This update verifies computer-microphone error ownership using controlled recorder channels and coordinator state, not a physical reproduction or attribution of every earlier Listener BLE incident. Hardware convenience and purchase interest are not established by software checks.

## File identity

- Type source: `b2c6f33d7ecc83977373ddc8cc33c116e3123751` · `build-source/type139-20261023`
- MSI: 299192320 bytes · SHA256 `8aef71d6734499fdb6d93e98304c527d8f3d26e8dd2ce26d7290dcf0c93a463d`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

电脑麦克风错误从录音启动起保留原SessionId，错误处理在同一状态锁中核对归属并申领中止。旧错误不再借当前编号误结束新录音，当前错误仍正常收尾。生产监听器反例修前失败、修后及相关4项通过；不是全部蓝牙现场提前结束、抗干扰或购买价值的验收。
