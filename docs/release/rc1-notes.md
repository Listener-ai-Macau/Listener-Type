# Listener v1.0.6-rc1 · Type build225 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Automatic-recording takeover is now bound to recognition startup. A failed or cancelled startup expires its unsent queued command; cleanup of an already-sent takeover keeps the original request/capture identity. Successful startup and existing deadline/recording behavior remain. The text workspace now labels ordinary processing and instruction drafting directly, shows preparation, and reserves its action area while readiness changes. Editing, immutable saved drafts, first-input retention, received-audio recovery and opt-in shortcuts are retained. Firmware38 is unchanged.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

The original intermittent Windows GATT write timeout has not been attributed or declared repaired. Source cancellation/queue and firmware ownership checks are controlled native/C executions; MSI failure records use explicit synthetic production-test seeds, and text protocol checks use local controlled replies. They do not prove new over-air fault recovery, real microphone/speaker, system clipboard/external insertion, brownout cause, second-PC or long-term behavior. Existing light CPU translation remains an editable draft requiring review. Hardware costs US$49; Type is free/open source. Third-party services require separate preparation and charges; no official cloud account or allowance is bundled.

## File identity

- Type source: `c1ea4de9ecc54f635c8fb0e7f0d4ce0f75a36c50` · `build-source/type225-20261029`
- MSI: 299872256 bytes · SHA256 `f1919e3f58d29735ca2fb5650ce835e12d0862004275e396cc5d8871b3ecd590`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

自动录音的设备接管请求现在跟随识别启动的生命周期：启动失败或取消时，未发出的排队命令失效；对已发出请求的清理仍绑定原请求和采集代次。成功启动及既有时限、录音收尾规则保留。文字工作区区分「处理文字」与「生成正文」，显示准备状态，并在服务准备状态变化时固定操作区。原始要求、各份独立成稿、已接收音频回找与按需设置快捷键保留；固件38不变。
