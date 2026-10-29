# Listener v1.0.6-rc1 · Type build232 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Long WAV, MP3 and M4A recordings are processed as explicit separate speech-service requests. Neighboring segments overlap by one second and are labeled for review; a later failure or stop retains completed segments in the open window for copying, saving or explicit continuation. Each request is billed under the configured provider, and retries may incur extra charges. In local translation, optionally list names or terms to preserve exactly. If a preservation marker cannot be verified, the original stays and no unreliable result is presented.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

Adjacent segment boundaries may repeat words; check them and any segments marked unrecognized against the original. Progress cannot resume after the transcription window closes; already copied or saved text remains. MP3/M4A depend on system codecs, and long files can use substantial local memory. Both local translation models may mistranslate, omit or misassociate meaning, names or negation even when explicit names are preserved; always compare the output to the source. The optional enhanced model requires downloaded components and CPU resources; severe memory pressure can stop it. Cloud services require separate credentials and charges. Existing intermittent GATT timeout and historical brownout causes, other-PC and long-term physical behavior remain unresolved.

## File identity

- Type source: `71de4e7247ca812c6865f4452ff3b91ff92e6069` · `build-source/type232-20261029`
- MSI: 299900928 bytes · SHA256 `1d8224cd35005dc19c2ea15709fc8b80e0df95e6c1a4c56291fa1bd5eb8d69b4`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

支持将较长的WAV、MP3或M4A录音分段转写；相邻片段重叠一秒并标记时间供核对，后续失败或停止时已完成文字留在当前窗口，可复制、保存或继续。每段按语音服务规则计费，重试可能额外计费。本机翻译可选填需原样保留的人名或专有词；无法验证时保留原文并提示失败。
