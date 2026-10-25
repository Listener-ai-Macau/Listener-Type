# Listener v1.0.6-rc1 · Type build179 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

Cancelling the Windows file chooser in **Tools → More tools → Transcribe recorded audio** keeps the recording workspace and its current selection open. Closing the workspace itself still works. The file-input cancellation was previously mistaken for a dialog close; exact native before/after evidence now covers this case. Previous diagnostic export privacy, responsiveness and firmware-log fixes remain included.

This maintenance fix does not change live recognition, wake/ending, Bluetooth transport, firmware, cloud credentials or models. It does not upload audio when selecting or cancelling a file.

## Before using

US$49 is the hardware price. Type is free and open source. Recognition and optional text services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP. Optional local translation components are prepared separately; their general translation quality is not guaranteed by this file-picker fix.

RC1 is a prerelease. Strong background-speech separation and some physical/global-key, optional-resource and long-term acceptance scopes remain incomplete. The scoped software checks do not establish complete hardware reliability, purchase intent or a real order.

## File identity

- Type source: `adabc2c89ec9301806b63cdfeab2692c2745d23c` · `build-source/type179-20261025`
- MSI: 299360256 bytes · SHA256 `2148767f124a202f313fdd60d0da2cf9ac9b45b86a66a6bbd8e9ab5eaa509e64`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

修复“工具→更多工具→转写已有录音”取消系统文件选择框时把整个录音工作区一起关闭、丢失已选录音的问题。取消选文件会保留原选择；正常关闭工作区仍然有效。已用实际 Windows 文件框和同一原始失败条件核对；不改日常识别、唤醒／结束、蓝牙协议、固件、账号或模型。共享验收弹窗的点击输入修复属于本机工程工具，不是 MSI 新功能。固件继续复用37，无需因本次文件框修复重新刷固件。
