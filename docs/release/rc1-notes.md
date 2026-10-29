# Listener v1.0.6-rc1 · Type build230 / Firmware38

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

The optional enhanced CPU translator now attempts the request without a fixed 4.4 GB available-memory admission gate. It can take time and uses memory; if system memory becomes critically low, its owned process stops and the original text remains, without cloud fallback. Two retained examples improved against the light model, but accuracy is not guaranteed. The optional email drafting style discourages invented prior interactions and removes only a trailing empty sender placeholder when none was requested; existing and custom styles stay available. Review all generated text.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

Both local translation models may mistranslate or omit names, negation, commitments or other meaning; compare the output to the original. Enhanced needs optional downloaded components and CPU time; performance and available memory vary. A critically low-memory task stops without cloud fallback. Email generation may still invent or omit facts. MP3/M4A decoding depends on system codecs and remains limited to 60 seconds. Existing GATT timeout and brownout causes, real other-PC and long-term physical behavior remain unresolved. Cloud services require separate credentials and charges.

## File identity

- Type source: `4090f9e065ac54d5b50731b9def9ebba4ec9de96` · `build-source/type230-20261029`
- MSI: 299896832 bytes · SHA256 `ed6424774ea039fbeb99575fac8d69b265745ea7f84e715a21ff0f5427e7a624`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

可选增强翻译仍在本机 CPU 上运行，不再按固定4.4GB可用内存提前拒绝；执行时内存实际紧张会停止并保留原文，不自动转云端。两个保留案例较轻量模型改善，但不保证任意译文准确。可选邮件起草更明确避免虚构此前互动，并只清理未要求的末尾空签名占位；原有和自定义风格保留。所有生成文字仍需核对。
