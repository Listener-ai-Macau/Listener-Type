# Listener v1.0.6-rc1 · Type build154 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this update

Type now recovers automatic loading of a connected keyboard's stored host-action profile after a temporary read failure. Recording defers this background read without permanently abandoning it after 30 seconds. A single task follows the exact connected peer and generation, uses paced retries, yields at query boundaries when recording starts, and discards an old connection's completion. It does not push computer defaults or change saved settings.

The previous one-shot behavior failed the controlled temporary-error/long-recording scenarios. The repaired Windows driver and retained profile cases passed. The exact installed MSI also loaded the actual board automatically without opening Settings or manually reading its profile, while audio returned ready. Windows HCI confirmed matching hardware UID, revision and checksum; passive receiver and firmware serial observations were retained. This proves normal automatic loading. Deliberate radio failures, physical key output, a second computer and complete air capture were not qualified in this batch.

Per-device automatic save, pending-layout protection, the fixed-text editor and bounded complete diagnostics are retained. Recognition, ending, insertion and radio parameters are unchanged. Firmware37 is the same binary ZIP.

## Before using

US$49 is the hardware price; Type is free and open source. Recognition and optional text services require preparation and separate usage charges. This release does not provide a ready-to-use official cloud account or allowance.

## Known limits

Strong-interference isolation and universal wake/ending behavior are not guaranteed. Prior real-user input evidence covers its recorded applications and conditions. Physical HID, multiple computers/boards, complete RF throughput, dual microphones and hardware buying intent retain their separate qualification limits. AI feedback is not a real order or complete physical acceptance.

## Exact artifacts

- Type source: `06e8dc5f6d8133f987f86da9e7cf6207dd25973a` · source tag `build-source/type154-20261023`.
- MSI: `ListenerType_1.0.6-rc1_x64_en-US.msi` · 299208704 bytes · SHA-256 `b1afa29fba306b4237acfcd0a3f355b7581a16e80183778af5625267ea48ba6d`.
- Firmware source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · source tag `build-source/firmware37-20261023`.
- ZIP: `ListenerFirmware_1.0.6-rc1_ESP32S3.zip` · 1956799 bytes · SHA-256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`.

Only the MSI and firmware ZIP are the current downloads. Previous exact source tags and local rollback artifacts are preserved.
