# Listener v1.0.6-rc1 · Type build157 / Firmware37

## Download

- [Windows MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)
- [ESP32-S3 firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)

## Changes

Selection Q&A now follows the common recording exclusion during firmware updates. An update stops Q&A capture while keeping its panel and prior answers; an answer already processing is retained. The Q&A panel displays the actual configured recording shortcut both when opened and after a shortcut edit, instead of the old key or a missing-label token.

Prior per-device automatic saving, key-profile recovery, Translation input selection and Q&A context/history repairs are retained. Firmware37, radio settings, providers and model choices are unchanged.

## Checked scope

Actual old production boundaries failed two OTA/Q&A cases; the repaired matching Windows library passes the affected QA and recording-gate cases. The exact packaged private UI saves Ctrl+Shift+F10 and displays it on live update and reload. Installed preferences, history, styles and protected credentials are preserved, and normal device audio readiness returns. Passive receiver/HCI/UART observations are not complete encrypted RF or throughput qualification.

## Before using and known limits

US$49 is the hardware price; Type is free and open source. Recognition and optional text services require preparation and separate usage charges. This release does not provide a ready-to-use official cloud account or allowance.

RC1 remains a prerelease. Full pre-sale function acceptance is incomplete: physical Translation, live selection-to-cloud Q&A, actual global hotkeys, cross-computer keys, some lamp/power behavior and other recorded gaps remain unqualified. Strong-interference exclusion is not guaranteed. AI interest is not a real order or full physical acceptance.

## Exact artifacts

- Type source `2a64a18d41927f60da8ca40d71453f1053835876`; tag `build-source/type157-20261024`.
- MSI `299208704` bytes; SHA-256 `b4fae3a7a6d401843fceedb3d2ed6e87e3b57a41015d1441727380edfa5bc143`.
- Firmware source `87ea79ec3d6e34a95d09b73e2a5848207a919f1c`; tag `build-source/firmware37-20261023`.
- Firmware ZIP `1956799` bytes; SHA-256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`.

Only the MSI and firmware ZIP are current downloads. Earlier exact source and local rollback packages remain preserved.

## 中文说明

修复问答录音绕过固件更新限制，以及自定义录音键在问答窗口显示错误的问题。更新时停止仍在录制的问答，保留窗口和已有回答；已进入处理的回答继续保留。问答按键提示与实际配置保持一致，修改后和重新打开均已用真实包内界面确认。保留原设备自动保存等功能，固件37未改；全功能售前验收仍未完成。
