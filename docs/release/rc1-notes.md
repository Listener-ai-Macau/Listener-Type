# Listener v1.0.6-rc1 · Type build156 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this update

- Device Translation now starts the selected recording input and uses the regular stop path. Its output mode is bound when a recording starts, retained during that recording's BLE attachment, and cleared for the next ordinary recording. Recording admission is checked at the actual startup boundary as well as the key action.
- In selection Q&A, a changed selection now accompanies the current follow-up question. An unchanged reference reuses conversation context. Failed answers do not advance the remembered reference, so retry retains the new selected text.
- Saved Q&A records show Question, Answer, context source and Saved. They no longer claim that an answer was inserted or copied automatically. The explicit Copy action remains available. Existing history files are retained without migration; all five shipped UI locales have the new labels.

The matching MSI is installed with preferences, history, styles and protected credentials preserved. Controlled native source/session tests and an actual private packaged History → Copy → paste check passed. The History fixture was synthetic; this is not a live external selection, microphone, cloud Q&A or physical Translation-key qualification. Normal audio readiness returned after update; passive receiver, Windows HCI and firmware serial observations were retained, without forced device restart, pairing or firmware writes. Complete RF following and throughput were not qualified by this window.

Per-device automatic saving, automatic profile-read recovery and complete bounded diagnostics from the previous release are retained. Firmware37 is the same ZIP and binary; no new model, ASR provider or radio parameter is included.

## Before using

US$49 is the hardware price; Type is free and open source. Recognition and optional text services require preparation and separate usage charges. This release does not provide a ready-to-use official cloud account or allowance. Use the same firmware ZIP for USB flashing and Bluetooth OTA.

## Known limits

RC1 is a prerelease. Full pre-sale function acceptance is not complete. Physical Translation-key speech/output, live selection Q&A/global shortcuts, cross-computer keys and some lamp/power behaviors still need their separate checks. Strong-interference separation and universal wake/ending behavior are not guaranteed. Prior real-user input evidence covers only its recorded applications and conditions. AI interest is not a real order or complete physical acceptance.

## Exact artifacts

- Type source: `1eda7b285849b1af652bd2a1cd059055672afc35` · source tag `build-source/type156-20261023`.
- MSI: `ListenerType_1.0.6-rc1_x64_en-US.msi` · 299220992 bytes · SHA-256 `4b9e2bacf2a128bab1310b0682a755a5c67227e1b7358fb96d76f99541762656`.
- Firmware source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · source tag `build-source/firmware37-20261023`.
- ZIP: `ListenerFirmware_1.0.6-rc1_ESP32S3.zip` · 1956799 bytes · SHA-256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`.

Only the MSI and firmware ZIP are the current downloads. Previous exact source tags and local rollback artifacts are preserved.

## 中文说明

修复设备翻译按键用错音源及停止路径；划词问答在更换选区后会携带新的参考内容，失败重试不会丢掉它。问答历史明确显示问题、回答与已保存，不再误报自动上屏或自动复制。已完成受影响的原生软件检查和隔离包内历史复制取回；实体翻译按键、真实划词到云端回答等完整链路尚未全部验收，不宣称全功能通过。固件37未改。
