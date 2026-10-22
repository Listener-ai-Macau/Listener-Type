# Listener v1.0.6-rc1 · Type build138 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

A malformed stream input that never acquired a recording owner could previously publish an error against whichever recording was current, moving an unrelated Starting recording to Idle. That fallback dates to the earlier fix for a stuck error capsule. It now cannot terminate an unrelated Starting, Listening or Processing recording. Errors with a known recording owner still close only that owner. Idle diagnostics retain their generation identity.

The public-input counterexample failed before the fix and passed after it, alongside the matching-owner/newer-owner and existing background terminal/timeout protections. No new cloud service or model is included.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

RC1 is a prerelease. Strong background-speech separation remains incomplete. Older intermittent early-ending and arbitrary target-field scopes remain open; software checks do not establish physical/long-term reliability or purchase value. This update fixes the production ownerless-error counterexample, without attributing all older field failures to it. Wake thresholds, providers, timers, Bluetooth transport and Firmware33 are unchanged.

## File identity

- Type source: `4065fe1a9aef1aab28fbd7eb874a5ffbdc5a7816` · `build-source/type138-20261022`
- MSI: 299208704 bytes · SHA256 `c923af7e8e7a3d7c4a5cec22135eef11564f0cd338e8c15f620b920293730034`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

无owner的流失败不再借用当前录音编号结束其它录音；保留本次录音真实错误收尾和按代际归属的空闲提示。公开生产入口反例修前失败、修后通过，相关归属与旧保护通过。未宣称所有旧现场提前结束、任意输入框、抗干扰或实物长期稳定已解决。
