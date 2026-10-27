# Type build203 / Firmware38 · Listener v1.0.6-rc1

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB flashing or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this paired update

- Type confirms all firmware segment checksums before restarting. On native ESP32-S3 USB, it reads the boot state and uses a full reset when the sampled BOOT state requires it. A held BOOT pin remains an explicit error.
- Startup success requires a normal application response from the same USB device. A reset request or a port appearing is insufficient.
- Firmware38 performs a clean USB detach and peripheral initialization once during boot, before the USB driver owns it. It preserves the power latch and skips an attached debugger or an already running driver. It adds a 500 ms startup detach interval, not a recurring interruption during dictation.
- Includes the build202 fix for reading the hardware model and actual hardware revision independently during Bluetooth OTA preflight. Incompatible or unknown hardware remains blocked.
- Real normal-UI Bluetooth OTA (installed202, unchanged OTA code in203) and normal-UI wired flashing (exact203 MSI) passed on the current board, including key/settings readback. Install both paired files for these fixes.
- Retains build201 saved-draft continuation and first-save naming, earlier text safeguards and configured recognition services.

## Scope and remaining limits

The original brownout/low boot-strap event is retained as an unresolved power/startup-source investigation. This update improves recovery and USB initialization; it does not establish that supply transients cannot recur. No brownout protection or application watchdog was disabled. One board's recovery is not long-term or all-board qualification.

US$49 is the hardware price. Type is free/open source; recognition and optional text services need preparation and separate usage charges. This release supplies no ready-to-use official cloud account or allowance. RC1 remains a prerelease. Strong background-speech separation, remaining physical/global-input combinations and local-translation meaning quality retain their stated limits. AI software feedback is not a purchase or physical acceptance.

## File identity

- Type source: `394eb24266c9a9c68548d11a4e2a6c7d9b1599fa` · `build-source/type203-20261027`
- MSI: 299507712 bytes · SHA256 `91657b1229af7fb072c51fb7ac5e1872f346898a3fca922114edf3bb20c079ea`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · `build-source/firmware38-20261027` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`
- Firmware binary: SHA256 `c16fba3faeb6c7998a9012cb85e639655f8c7c3f95e348081e810ea90eba4e98`

## 中文说明

本次配套更新处理刷完后未真正启动、USB复位后接口异常的问题。有线刷机先逐段校验，再依据真实启动状态选择复位，并等待同一设备的应用回包；固件在开机早期对USB执行一次明确断开和重新初始化。当前板的正常界面有线刷机、蓝牙OTA、重连和按键/设置保留已有实测。原欠压触发源仍在排查，未关闭保护，也未宣称长期或全部硬件验收完成。
