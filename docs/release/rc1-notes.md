# Listener v1.0.6-rc1 · Type build136 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

A physical segment may stop while its last audio packets are still draining and Type is waiting for a request-tagged continuation. Those packets now keep their original collector and logical recording instead of failing with a session mismatch. Repeated packets are not submitted twice, and an unconfirmed or later segment cannot inherit that audio.

Device cancellation now closes both its stream actor and the matching product recording state. Stale cancellation cannot close a newer recording, and a consumed device cancellation does not requeue another shared abort.

The fixes have production-core before/after and isolated Windows checks. They do not prove every previously reported field failure had these causes. No new cloud service/model, recognition-provider change, wake-threshold change or firmware update is included.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

RC1 is a prerelease. Strong background-speech separation remains incomplete. Older intermittent early-ending and arbitrary target-field scopes remain open; software checks do not establish physical/long-term reliability or purchase value. This update changes host recording ownership/cleanup at physical STOP and CANCEL boundaries. Wake thresholds, recognition providers, Bluetooth transport parameters and Firmware33 are unchanged. The specific host packet-order counterexample is fixed; complete field attribution of older intermittent failures is still unproven.

## File identity

- Type source: `374ae18e25d85580294b9e5db5694d95f4e4dca4` · `build-source/type136-20261022`
- MSI: 299196416 bytes · SHA256 `b8c4eee3580a6856d836abfd43c6f7c9ef5ddf6aa218edec4ae731c7ca38cbb2`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

修复两处主机录音状态归属遗漏：停止后的同段尾包继续归原录音收取，不因等待续录而报会话不一致；设备取消同时关闭对应产品状态，旧取消不影响新录音，也不再次排入共享中止。生产代码前后对照通过；未把这些反例当作所有旧现场故障的唯一根因。固件及蓝牙参数不变。
