# Listener v1.0.6-rc1 · Type build158 / Firmware37

## Download

- [Windows MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)
- [ESP32-S3 firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)

## Changes

Microphone capture now requests cancellation when startup fails/times out or its owner is dropped. Late device setup checks cancellation before playback and gates later callbacks; explicit Stop continues to wait for the worker and report its error. Product recorder handoff checks the current session, recording phase and update exclusion before accepting a handle and its archive status. A late result stops its own recorder instead of replacing the new one or continuing startup.

Normal sample rate, recognition services, Bluetooth parameters and the three-second ending policy are unchanged. Per-device auto-save, Translation and Q&A repairs from previous releases remain. Firmware37 is the same ZIP/binary.

## Checked scope and limits

The original capture lifecycle fails timeout/drop cancellation cases; the unchanged explicit-stop case passes. Repaired controlled lifecycle and actual consumed host-audio capture tests pass. Old product recorder storage fails two no-device ownership cases; matching native coordinator cases pass with32 existing cases still ignored. Exact packaged startup and installed data retention are checked. These results do not prove a real stalled microphone driver, physical speech, BLE auto-end behavior or every previously reported intermittent problem.

An OS driver call cannot be safely force-killed. Cancellation prevents a late result from taking ownership, and the worker disposes of the stream when that call returns. Drop is nonblocking; explicit Stop is the synchronous release path.

US$49 is the hardware price; Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release supplies no ready-to-use official account/allowance. RC1 remains a prerelease. Full pre-sale acceptance, physical Translation/global keys, live selection Q&A, cross-computer keys and some lamp/power behavior remain incomplete. Strong-interference exclusion is not guaranteed. AI interest is not a real order or full physical acceptance.

## Exact artifacts

- Type source `7d17b85933c4f30268097f4363c9d7bdd2c9a2a3`; tag `build-source/type158-20261024`.
- MSI 299212800 bytes; SHA-256 `b6ba21fee944393cfc7981b6ae52e38bc2d4ea3ed15fd371c5309bd627f429e8`.
- Consumed shared capture source `39bae009668ab1bd8b5257dc6ce187b14e09a675`; Denzic-Platform tag `build-source/host-audio158-20261024`. Other product pins are unchanged.
- Firmware source `87ea79ec3d6e34a95d09b73e2a5848207a919f1c`; tag `build-source/firmware37-20261023`.
- ZIP 1956799 bytes; SHA-256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`.

Only MSI and firmware ZIP are current downloads. Prior exact sources, failures and local rollback packages remain preserved.

## 中文说明

修复麦克风启动超时／句柄释放后的线程残留，以及旧初始化返回时覆盖新录音句柄的问题。过期结果只清理自己，录音句柄与归档状态属于同一个当前会话。已完成原生产生命周期失败对照、受影响原生检查、包体与隔离启动确认；没有实际注入声卡驱动卡死，不把它解释成所有蓝牙提前结束问题。正常采样、云识别、蓝牙、三秒结束规则与固件37均未改，全功能售前验收仍未完成。
