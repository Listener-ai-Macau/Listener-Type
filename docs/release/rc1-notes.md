# Listener v1.0.6-rc1 · Type build137 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

A delayed host takeover timeout now cancels only the recording that created it. It cannot close a newer Starting or Listening recording, and a cancellation already consumed by the stream actor does not queue a second shared abort. Device cancellation uses the same owner-scoped cleanup. Existing live-body and actual-STOP audio-finalization protections remain.

A newly generated shorter draft is no longer labelled as a restored result with no new service request. Actual cached comparisons still reuse their edited results without another request. Saved-draft dates now follow the chosen interface language.

The stale-timeout defect was reproduced in production code before the fix and passes after it. Related private Windows regressions passed; this is not proof that every earlier field failure had the same cause. No new service/model, provider change, wake-threshold change, Bluetooth transport tuning or firmware change is included.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

RC1 is a prerelease. Strong background-speech separation remains incomplete. Older intermittent early-ending and arbitrary target-field scopes remain open; software checks do not establish physical/long-term reliability or purchase value. This update scopes a delayed host takeover timeout to its original recording and corrects fresh-result and saved-date presentation. Wake thresholds, recognition providers, Bluetooth transport parameters and Firmware33 are unchanged. The specific stale-timeout cancellation counterexample is fixed; complete field attribution of older intermittent failures is still unproven.

## File identity

- Type source: `c9d94570e4d799608f11acca98444028bb092b17` · `build-source/type137-20261022`
- MSI: 299204608 bytes · SHA256 `ae5ef3202bc877aee97af5b8205e055998b6dee42a378cf67f6031ef565d5de2`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

旧接管超时只清理它原本负责的录音，不取消后来开始的新录音，不再向后续录音留下共享中止标记。设备取消共用按会话归属清理，保留已出正文保护和真实STOP后的音频收尾。另修新缩短结果来源提示、成稿日期语言。生产代码反例前失败后通过；未宣称所有旧现场问题、抗干扰、任意目标上屏或实物长期稳定全部合格。
