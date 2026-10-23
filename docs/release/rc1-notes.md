# Listener v1.0.6-rc1 · Type build151 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this update

- Fixes a newly applied layout being cleared by an earlier pending save. Acknowledgements confirm only their submitted fields; newer edits continue saving to the original device. Failed writes keep the latest layout for explicit retry. Saved feedback waits for all pending edits.
- Device settings save automatically after you finish an edit. Switches and choices save immediately; names and numeric inputs save when editing finishes; sliders save when released. Saving, saved and failure/retry feedback replaces the separate Write action.
- Settings and custom key mappings remain on the selected keyboard. Reading, connecting or switching devices does not write settings. Pending edits keep their original target and failed saves remain available for retry.
- USB key-profile writes use matching request acknowledgements so delayed replies cannot be mistaken for a successful save. Firmware also protects an upload already receiving data from a repeated start command.
- Bluetooth settings reuse characteristic discovery only within the current selected-device transaction; actual setting values are still read fresh. This reduces repeated discovery, without changing radio parameters or claiming higher air throughput.
- Recording animation uses its recording lifecycle instead of restarting on state refresh. Earlier History playback/resource, processing-notice and question-session ownership fixes remain included.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP.

Strong-interference exclusion is incomplete. USB auto-save was checked through the actual app UI; current Bluetooth read/write/readback/restore was checked through the selected-device backend with HCI. Ordinary Bluetooth UI, physical HID on another computer, simultaneous physical boards and complete RF capture are not comprehensively qualified. Firmware37 was flashed through Type's built-in USB path while retaining device settings; this stable ZIP contains those exact binary payloads. No new OTA-throughput claim is made. Some remote marketplace/non-English factory content remains untranslated. Automatic shortening can return nearly unchanged wording; review before use.

Actual React checks cover a held earlier request, failures in either request with retry, and switching devices during the save. This app change does not modify the native backend, firmware or radio parameters; the prior USB/Bluetooth backend evidence keeps its original scope.

## File identity

- Type source: `a8aa31d450c701db15db83e91e82dc58c38d09fb` · `build-source/type151-20261023`
- MSI: 299188224 bytes · SHA256 `f41042263fbf139e683b67c0b3c7730fdebd881fb95dd78f6f91e2fdbe968946`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · `build-source/firmware37-20261023` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

修复保存期间应用新布局被旧请求清掉：只确认该请求提交的字段，后来的修改继续保存；失败保留最新布局供重试，全部完成后才显示已保存。设备设置编辑完成后自动保存，不用另点“写入”。开关／选项立即保存，文字与数值结束编辑时保存，滑块松手保存；保存失败保留修改并可重试。只写选中设备，不因读取、重连或切换误写旧值；按键档案保存在设备里。USB回执与请求对应，蓝牙同一次事务减少重复枚举但始终读取新值。录音灯效不再随状态刷新重启动画。保留历史录音播放等已验修复。

USB普通界面与当前蓝牙后台的真实读写、回读及还原分别有证据，未冒充普通蓝牙界面／物理HID／第二台电脑／完整空口均已验收。强干扰排除仍有限，云服务需另行准备与计费。本批没有增加云服务或本地模型。
