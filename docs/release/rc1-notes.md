# Listener v1.0.6-rc1 · Type build246 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

On Windows, an optional setting lets an unmodified Enter finish the current recording like the capsule checkmark. It is off by default. While enabled and recording, Enter is not sent to the active app and cannot confirm input-method candidates; modified Enter combinations remain available. Targeted key-edge and session-owner tests were run; a physical-key-to-final-text end-to-end check remains pending.

When a clean raw final differs too much from an earlier pasted prefix to locate a safe tail, Type attempts to replace only the same session’s verified, unchanged editor range with the complete final. If the range cannot be verified, it leaves the existing text untouched and reports that it may be incomplete. This scenario has targeted tests; actual end-to-end reproduction in the original editor is still pending.

Type retries the original wake-recording cancellation when the initial Bluetooth control write fails, while protecting later recordings and firmware updates. It also improves recovery of the wake helper, bundles the CPU voice-activity runtime for fresh Windows installations, and reduces sensitive diagnostic logging. Raw recording retention remains off by default.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

The cancellation retry is bounded and applies to the original connection and wake request; it cannot guarantee recovery if the connection remains unavailable or changes. A delayed scoped cancellation was verified on a physical device using a separate BLE client. The actual intermittent timeout and Type retry were not reproduced end to end, and complete encrypted sniffer following was not qualified. Intermittent Bluetooth timeout, occasional wake failure, other-computer and long-term physical behavior remain under investigation. Review translations and wording suggestions against the original.

Translations can alter meaning or numbers and require review. Enhanced CPU translation requires optional components. Cloud recognition and wording services may incur separate charges. Long files use overlapping segments; check boundaries and unrecognized portions. Closing the file transcription window loses audio and resume progress, while copied or saved text remains. MP3/M4A depend on system codecs. Historical brownout causes and long live recordings with the actual owner remain unverified.

## File identity

- Type source: `f92a484c1ca6611c4bf5ade313840ea4d4d1ff05` · `build-source/type246-20261031`
- MSI: 306540544 bytes · SHA256 `f7ac76c6bff65dd5ecef55cc4dbedbcc8d34f16e180c180e3beb927e5c32ac87`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

Windows 新增可选的 Enter 提前结束录音，作用与录音胶囊的勾相同，默认关闭。启用后，录音期间的单独 Enter 不会发送给当前应用，也不能用来确认输入法候选字；组合键不受影响。按键及会话定向测试已运行，实体按键到最终文字的端到端验证尚待完成。

本次更新在提前上屏的文字与最终原文无法安全对齐时，尝试只替换本次录音在同一输入框内已确认且未被修改的范围；不能确认时不强贴，并提示内容可能不完整。此路径已做定向测试，原输入框端到端复现仍待确认。

同时，在自动唤醒录音的取消指令暂时失败时，对原录音执行有限重试，同时保护后续录音及固件更新。改善唤醒辅助程序恢复，补齐新装机器的 CPU 语音活动检测组件，并减少敏感诊断日志。原始录音默认不保留。

蓝牙持续不可用或连接已更换时，重试不保证成功。实体设备已验证延迟取消，但通过独立蓝牙程序操作；尚未完整复现 Type 自身的真实超时与重试，抓包也未完整跟随。偶发蓝牙超时和唤醒失败的根因仍在排查。
