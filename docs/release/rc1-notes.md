# Listener v1.0.6-rc1 · Type build174 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

The Style editor and explicit new-pack templates now use the selected English, Simplified/Traditional Chinese, Japanese or Korean interface language. The previous Japanese and Korean editors fell back to Chinese. The original English/Chinese defaults and all existing authored/imported style content are preserved, including the literal `{{HOTWORDS}}` slot. Only unchanged factory vocabulary names get translated display labels; editing, saving and applying still use the original names and phrases. Presets containing Chinese terms are labelled accordingly.

This addresses the earlier full-function language finding. Actual isolated packaged UI checks cover creation, saving and reload in all five locales and vocabulary editing/application. They do not prove OS-wide actions, physical hardware, optional models, remote marketplace operation, language model quality or all-feature/long-term reliability. Prior failures and incomplete scopes remain recorded.

## Before using

US$49 is the hardware price. Type is free and open source. Cloud recognition and optional cloud text services require preparation and separate usage charges; this release does not provide a ready-to-use official account or allowance. Optional Windows Chinese ↔ English local text translation needs a one-time component download and has no translation API fee. It does not configure speech recognition. USB flashing and Bluetooth OTA use the same firmware ZIP.

Recognition, wake/end logic, translation models and Bluetooth firmware are unchanged. Strong background-speech separation remains incomplete; older intermittent ending and arbitrary-editor scopes remain separately open. RC1 and limited software checks are not universal qualification or a real order.

## File identity

- Type source: `120cc28d877ee511cc55af87adc4896b70bc977f` · `build-source/type174-20261024`
- MSI: 299372544 bytes · SHA256 `840fb6b1ede2eddae59bb332150f0a6c41af481a579ba2e4e4bf9496946f7509`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

补齐英文、简繁中文、日文和韩文的风格编辑器及新建模板；已有自写风格和词库内容原样保留，内置词库名称按界面语言显示。语音、模型、设备设置与固件37不变。 原173的日文界面编辑器中文回退已留实机证据。已有自写或导入的中文是用户内容，不自动翻译或覆盖。当前仅确认本批五语言实际本机编辑与保存范围，原未完成的全局快捷键、系统文件窗口、选装模型及实物／长期范围继续保留。
