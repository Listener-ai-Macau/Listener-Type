# Listener v1.0.6-rc1 · Type build184 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

The question window now has visible Open, Start question, Finish and ask, and Close controls. On Windows, showing and hiding use one native visibility owner, fixing a window that could remain visible after Close. Optional shortcuts are kept.

Before a question starts, Type shows separate configuration-presence indicators for Volcengine recognition and the active AI answering service. Missing setup links directly to Providers, with APP ID, Access Token and Resource ID named consistently. Configured means values are present; it does not establish authentication, network or quota health. The question window uses the computer microphone. It does not change ordinary Listener dictation or board key mappings.

Existing windows and retained setup errors now follow the interface-language choice. Missing service setup no longer shows Ready while Start is disabled. The history switch explains local question/answer storage and that cloud processing still sends data. Exact native checks cover missing-configuration handling, direct settings navigation, language changes and Close; actual speech-to-answer/selected-context and opening Settings before the main window exists remain unqualified. Earlier file-cancellation, diagnosis, device-profile and input-source fixes are retained. Firmware37 is unchanged; no reflash is needed for this app update.

## Before using

US$49 is the hardware price. Type is free and open source. Cloud recognition and optional AI services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP. Optional local translation components are prepared separately; general translation quality remains limited.

RC1 is a prerelease. Strong background-speech separation, some physical/global-key/optional-resource scopes and long-term acceptance remain incomplete. AI software feedback is not hardware qualification or a real order.

## File identity

- Type source: `03089e7a2f6bbb3007a1a5ca35dc346cf949d97b` · `build-source/type183-20261025`
- MSI: 299388928 bytes · SHA256 `247ddfe7a0cb1cefc19a43df8af276e3a21e0db408496a436d83279d6eb2befb`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

问答可以直接用按钮打开、开始、结束并提问、关闭，不要求记住快捷键。Windows显示／隐藏统一原生入口，修复后台已关闭但窗口还在的问题。开始前显示识别与回答两项配置存在状态，缺配置可直达提供商，字段统一为APP ID／Access Token／Resource ID；已开浮窗和已出现的配置错误随主界面语言更新。缺配置时不再提示准备就绪，历史开关说明本机保存与云处理的边界。问答仍使用电脑麦克风，保留Listener日用录音来源与板上按键。原生已检查相关配置、跳转、语言和关闭；配置存在不等于鉴权／网络／额度有效，真实讲话与选区到回答、首次创建主窗仍未完整验证。固件37不变，无需为本次软件修复重刷。
