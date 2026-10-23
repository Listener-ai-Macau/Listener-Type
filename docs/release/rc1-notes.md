# Listener v1.0.6-rc1 · Type build159 / Firmware37

## Download

- [Windows MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)
- [ESP32-S3 firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)

## Changes

Recognition startup now checks session identity, recording phase, cancellation and update exclusion before accepting a connection. A late or duplicate initializer keeps the first valid resource and cannot replace a newer session. Rejected resources are cancelled outside state locks. Every microphone and embedded-provider registration handles rejected startup explicitly; retiring a session during startup is ignored quietly instead of reporting an error for a replacement.

Bailian connections are registered before the first network wait. Cancellation interrupts pending connection/handshake/task-start waits and a cancelled instance cannot reopen. Both Volcengine startup-completion paths retain current Processing for tail finalization while rejecting completed Inserting/Idle or replaced sessions. Shared capture cleanup from158 remains.

Normal sample rate, Bluetooth protocol/parameters and the three-second ending policy are unchanged. No service credentials or optional models changed. Firmware37 is the same ZIP/binary.

## Checked scope and limits

Original resource storage fails5 of8 controlled cases; original Bailian cancellation fails2 local-loopback cases; the extracted old completion predicate also fails the completed-state check. Matching repaired native coordinator780 tests pass with32 existing cases ignored;6 Bailian tests and3 embedded-startup integration cases pass. Loopback tests use fake credentials and withhold the handshake locally, including cancellation through the actual coordinator. Exact packaged startup, cleanup, installed identity and preservation of preferences/history/styles/protected vault are checked.

These results do not qualify physical speech, remote provider recognition accuracy, BLE cancellation, all target editors or every intermittent early-ending report. Full pre-sale acceptance, physical Translation/global keys, live external-selection Q&A, cross-computer keys and some lamp/power behavior remain incomplete. Strong-interference exclusion is not guaranteed.

US$49 is the hardware price; Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release supplies no ready-to-use official account/allowance. RC1 remains a prerelease. AI interest is not a real order or full physical acceptance.

## Exact artifacts

- Type source `2e90f66ef22fcecee2f62b436cb9ca8cc1273ce6`; tag `build-source/type159-20261024`.
- MSI 299216896 bytes; SHA-256 `3ea2c950bb9f3ff060650dc33708ec7a9b48c5c752656c27cc03f3a9b28dc442`.
- Consumed shared capture source `39bae009668ab1bd8b5257dc6ce187b14e09a675`; Denzic-Platform tag `build-source/host-audio158-20261024`. Other product pins are unchanged.
- Firmware source `87ea79ec3d6e34a95d09b73e2a5848207a919f1c`; tag `build-source/firmware37-20261023`.
- ZIP1956799 bytes; SHA-256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`.

Only MSI and firmware ZIP are current downloads. Prior exact sources, failures and local rollback packages remain preserved.

## 中文说明

修复同一识别启动链的资源归属、等待期间取消、迟到完成回调问题。过期任务不能覆盖当前录音，取消可及时中断百炼连接等待，有效尾句仍可完成收尾。已保留原实现失败并完成匹配原生与本地连接对照、包体及空闲安装核验；不把受控测试说成真实云端语音、所有现场故障或全功能售前验收通过。固件37、蓝牙参数、采样和三秒结束规则未改。
