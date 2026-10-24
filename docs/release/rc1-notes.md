# Listener v1.0.6-rc1 · Type build176 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Diagnostic ZIP exports now omit records containing credential fields, including ordinary Bearer values, spaced JSON and separated assignments. Free-form provider response bodies and unframed continuation lines are excluded; connection metadata and HTTP status remain. Original local logs and explicit raw-log export are unchanged. Previously retained debug WAV samples may still be included, as declared in the archive manifest; export does not start a recording.

The old whitespace-only masker missed synthetic standard credential formats. The same actual Rust functions changed from four failures/one pass to five passes. Ten affected Windows Rust tests also verify the real ZIP writer/reader, checking all archive members for synthetic credential/transcript canaries, preserving safe BLE metadata and the existing offline/firmware archive structures. These are synthetic software checks, not evidence of a real-user leak or actual device export. Matching package launch/source identity is checked separately. The native Save dialog and real firmware diagnostic pull remain separately unverified.

Recording, recognition, models, providers, keys, BLE protocol and Firmware37 are unchanged.

## Before using

US$49 is the hardware price. Type is free and open source. Cloud recognition and optional cloud text services require preparation and separate usage charges; this release does not provide a ready-to-use official account or allowance. Optional Windows Chinese ↔ English local text translation needs a one-time component download and has no translation API fee. It does not configure speech recognition. USB flashing and Bluetooth OTA use the same firmware ZIP.

Strong background-speech separation remains incomplete; older intermittent ending, arbitrary-editor and other physical/long-term scopes remain separately open. Limited software checks are not all-feature qualification or a real order.

## File identity

- Type source: `1da31df3c201e6a321dc95877daece4ee4e82c83` · `build-source/type176-20261024`
- MSI: 299343872 bytes · SHA256 `aec81a42bb0675a388ded9d5bc3fb5e965c446159350bed97c29ba9ffb6f9ffd`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

加强诊断ZIP过滤：排除含凭据的记录、服务端自由正文及续行，保留连接和HTTP状态元数据。原始本地日志和已声明的调试音频保留政策不变；语音、模型、蓝牙协议与固件37不变。 实际Rust函数反例与10项Windows归档测试分别留证，原始失败保留；没有读取真人日志或让在用设备导出。仅确认本批诊断ZIP内容过滤，系统保存窗口／真实固件拉取及其它未完范围继续保留。
