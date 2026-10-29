# Listener v1.0.6-rc1 · Type build224 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

A recording takeover that cannot maintain its connection now leaves an actionable error and a findable failed History entry. If recording retention is enabled, the audio already received before the interruption remains available for playback or export; later speech may be missing. Turning recording retention off continues to prevent audio-file storage. Actual user cancellation and subsequent recordings keep their existing behavior. In the text workspace, refining a translation that began as a message-drafting task now selects the existing Light refinement style, keeps the first instructions and saved versions, and still requires an explicit submit. Other selected styles remain selected. Firmware38 is unchanged.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

This improves failure reporting and recovery; it does not claim to repair the intermittent Windows GATT write timeout or restore speech never received after an interruption. Native seeded failure records come from production lifecycle tests with synthetic PCM, and text protocol checks use local controlled replies. Original voice/noise/early-ending, brownout cause, remaining real OS/physical/cross-PC and long-term evidence gaps stay open. The light CPU translation worker retains its editable-draft quality limits. Hardware costs US$49; Type is free/open source and third-party services have separate charges, with no bundled official cloud account or allowance.

## File identity

- Type source: `6ac9cb46eb53b0590fa7b7c53f5a45ea6c24c111` · `build-source/type224-20261029`
- MSI: 299868160 bytes · SHA256 `afa2ddfc3fd92ac7eb386efa47fa6ba1b28a349ada5d404d7e3d593f69619bcc`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

修复录音接管超时被记为用户取消、历史中无法回找的问题：技术失败显示错误并生成原会话的失败记录；仅在开启录音保留时保存已接收到的音频，便于回找或导出，不能补出中断后的声音。关闭录音保留继续不落音频，真正取消及后续会话保护保持。此前待交付的起草→翻译→继续润色同批收敛：明确继续时切到已有Light正文润色，保留原要求和各份成稿；用户手选其它风格不变，无自动提交或付费重试。固件38不变。
