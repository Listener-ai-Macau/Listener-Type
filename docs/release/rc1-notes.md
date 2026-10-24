# Listener v1.0.6-rc1 · Type build175 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

One-click repair now keeps one outstanding operation across the ordinary main-window entries, including the cancellation step. Closing and reopening About observes the same repair. About shows progress and the result; it allows a fresh retry after completion or failure. A failure to open system Bluetooth settings has its own message. A closed About page cannot later open those settings from a stale response.

The old About button could send two cancellation/recovery requests while the first repair was still pending. Eight controlled UI/request checks cover the new lifecycle. The exact packaged native check covers the idle control, local Help navigation and retained state; it intentionally does not invoke repair on a device in use. Actual GATT recovery/re-pair and OS launch remain separately unqualified. The native recovery protocol, Firmware37, recording, recognition, models and provider preferences are unchanged.

## Before using

US$49 is the hardware price. Type is free and open source. Cloud recognition and optional cloud text services require preparation and separate usage charges; this release does not provide a ready-to-use official account or allowance. Optional Windows Chinese ↔ English local text translation needs a one-time component download and has no translation API fee. It does not configure speech recognition. USB flashing and Bluetooth OTA use the same firmware ZIP.

Strong background-speech separation remains incomplete; older intermittent ending, arbitrary-editor and other physical/long-term scopes remain separately open. Limited software checks are not all-feature qualification or a real order.

## File identity

- Type source: `58e387143a23a5033e562d4f4fe60a7ffdb53567` · `build-source/type175-20261024`
- MSI: 299364352 bytes · SHA256 `76cb2c0a4b16dd99312398b82a8143d1519f123bad7ca7f38176d47f9bc75794`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

让一键修复在执行期间保持同一操作，关闭再打开关于页也不会重复发起；关于页显示处理中和结果，打不开系统蓝牙设置时给出明确提示。固件37、原生蓝牙恢复协议及语音／模型设置不变。 原“关于”按钮处理中仍可重复点击、没有结果回显的问题已保留反例。8个受控界面范围和匹配原生空闲入口分别留证；没有调用在用设备恢复，不冒充真实断连／重新配对通过。原未完成验收范围继续保留。
