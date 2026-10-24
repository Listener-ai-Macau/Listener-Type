# Listener v1.0.6-rc1 · Type build178 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Diagnostic exports run device/file work on a worker and retain exclusive ownership while the work is active, including when a caller leaves. Repeated export requests share one operation and feedback across About navigation. The firmware's existing OTA readiness notification is recognized separately from real diagnostic log chunks; it no longer gets parsed as20,292 events. Unknown or corrupt chunks still fail their original validation and readiness does not renew the receive deadline.

The first177 candidate produced a valid ZIP but could not read firmware events. That failure is retained. The corrected installed178 received6,144 bytes of real firmware events with valid ZIP CRC;20 native status replies returned while export was pending. Audio readiness and all four existing state stores were preserved. The private test archive was deleted without playing, transcribing or publishing retained audio.177 worker/UI proofs and176 archive filtering tests are reused for their unchanged scopes; four new actual Windows receiver tests passed. The OS Save dialog and full About-to-save flow remain separately unverified.

Original local logs, explicit raw-log export and declared retained debug-WAV policy are unchanged. Recording, recognition, providers, models and Firmware37 are unchanged. This is a diagnostic reliability fix, not a Bluetooth throughput or background-speech separation claim.

## Before using

US$49 is the hardware price. Type is free and open source. Cloud recognition and optional cloud text services require preparation and separate usage charges; this release does not provide a ready-to-use official account or allowance. Optional Windows Chinese ↔ English local text translation needs a one-time component download and has no translation API fee; it does not configure speech recognition. USB flashing and Bluetooth OTA use the same firmware ZIP.

Strong background-speech separation, some real OS/global-key/physical paths and long-term qualification remain incomplete. Scoped software/native checks do not prove every customer-facing function or a real order.

## File identity

- Type source: `98cdb6cc969401aac715a5f334ffd9281b5cf31b` · `build-source/type178-20261025`
- MSI: 299360256 bytes · SHA256 `3b557756de051ee01b13804833fd4dc86f1ad60546d1d7e73849666a2bfac6ca`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP:1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

修复诊断导出可靠性：文件／设备任务移出界面调度，重复请求与跨页共用一次操作，固件就绪消息不再误作日志头。实际安装版已收到固件事件，导出时后台状态仍响应。录音、提供商、模型与固件37不变。 177候选的固件读取失败已保留；178实际导出收到6144字节固件事件，ZIP校验通过，并行原生请求正常返回，原配置／历史／凭据保持。仅清除测试新副本，未播放或发布录音。系统保存窗口、部分全局／实体与长期范围继续保留未完成，不宣称整机全功能合格。
