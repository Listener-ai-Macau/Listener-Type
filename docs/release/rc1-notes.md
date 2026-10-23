# Listener v1.0.6-rc1 · Type build144 / Firmware33

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Fixes History recording playback being rejected by the packaged media policy. Local blob audio is now explicitly permitted while other source policies retain their restrictions. Also fixes retained audio resources when switching History records or leaving the page before a recording finishes loading. A late file read checks its original player before allocating audio or reporting errors.

The exact144 MSI decoded a generated retained WAV and released its resources on record changes and page exit, with no media-src violation. Controlled actual-React tests cover late reads. The failed143 package and its observed policy error are retained in engineering history;143 was not installed or published. This is a specific playback/resource correction, not a measured claim about total application memory. Prior142 notice and recording fixes are retained.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

Strong-interference exclusion is incomplete. Firmware33 and its USB/OTA ZIP are unchanged; historical device tests retain their original scope. Live selection-QA/global shortcuts, simultaneous physical boards, fresh flash/OTA and optional local inference are not comprehensively qualified. Some remote marketplace/non-English factory content remains untranslated. Automatic shortening may return nearly unchanged wording; review and edit before use.

## File identity

- Type source: `7c7ff16d27b1a2929b2ae610d16823f3c976091a` · `build-source/type144-20261023`
- MSI: 299233280 bytes · SHA256 `9568c315e267528c21f219fd1f8846aaec981076852a91e9b956aa2065294831`
- Firmware33 source: `149b6d950ebc5f416924c8670cf2b207bdc2b5b7` · runtime `e7bd1f1619b696a1`
- ZIP: 1956905 bytes · SHA256 `306fbf8bb012f46ec911c074be8ee7f3633a1ac27f6e55273515cca6de25f272`

## 中文说明

修复安装版历史录音被媒体来源策略拦截，显式允许同源与本机blob音频；同时修复切换记录／离页后的迟到读取资源泄漏。以精确144包和自制静音核对解码及释放，无media-src违规；迟到场景另有实际React对照。143因原生播放失败未安装／发布，原失败包与根因留存，不覆盖为成功。未声称整机RSS长期降低。保留142既有录音／处理提示，固件仍33，识别与文字服务另行准备计费。
