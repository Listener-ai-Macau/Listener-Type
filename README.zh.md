<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type 是一个桌面语音输入工具。把光标点到想写字的地方，开始录音，然后正常说话。Listener 会把语音变成文字，再放回刚才使用的应用。

只用电脑自带的麦克风就能工作。配上 Listener 语音键盘后，同一套输入流程也能用旋钮和按键控制，并在桌面上看到蓝牙连接、录音和处理状态。

[English](README.md) · [繁體中文](README.zh-TW.md) · [下载](https://github.com/Listener-ai-Macau/Listener-Type/releases) · [使用说明](docs/USAGE.md) · [完整功能](docs/product/features.md)

<p align="center">
  <img src="docs/assets/readme/overview.png" alt="Listener Type 主界面" width="900" />
</p>

## 可以做什么

- **在当前应用里直接听写。** 用全局快捷键开始和停止，也可以说完后自动结束。小胶囊会显示实时结果，按 `Esc` 可以取消。
- **决定文字怎么写。** 保留识别原文，轻度清理语气词并补标点，整理成结构化笔记，改成正式表达，或者翻译成另一种语言。
- **让 Listener 认识你的词。** 人名、产品名、缩写、热词和纠错规则会跟随本地设置保存。
- **选择云端或本地识别。** 目前支持火山引擎、OpenAI 兼容批量 ASR、Apple Speech、百炼实时、macOS Qwen 本地识别和 Windows Foundry Local Whisper。
- **听写完继续处理。** 可以给上一条结果换风格、询问选中的文字、查看本地历史，也可以为常用动作设置快捷键。

如果当前应用不接受直接插入，Listener 会把结果留在剪贴板并提示粘贴，不会让整段文字消失。

## 开始使用

1. Windows 从 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 安装最新 MSI。macOS 和 Linux 可以从源码运行。
2. 允许麦克风权限，在设置 → 录音中选择“电脑麦克风”。
3. 把光标点进输入框。Windows 按右 Ctrl，macOS 按右 Option，说完后再按一次。

[使用说明](docs/USAGE.md)介绍了 Provider、写作风格、词库、快捷键、历史和常见问题。

## Listener 语音键盘

在设置 → 设备中配对键盘。单击旋钮开始或停止听写，旋转调节音量或亮度，KEY1–KEY4 可以分配常用动作。PWR、BLE、REC、AI、OK、WARN 会显示键盘和桌面应用当前在做什么。

设备页面还可以查看电量，调整灯光和休眠时间，恢复配对，以及更新固件。正常 OTA 会保留蓝牙配对和设备设置。

应用可以开启语音唤醒，也可以录三段声纹作为额外的输入保护。声纹不是身份认证。嘈杂环境、远距离和多人声音重叠仍在继续调校，这些情况下需要快速确认一下最终文字。

设置和恢复方法见[语音键盘手册](docs/quickstart/voice-keyboard-readme.md)。

<p align="center">
  <img src="docs/assets/readme/recording-settings.png" alt="Listener Type 录音设置" width="720" />
</p>

## 你的数据

设置、历史、词库、风格和纠错规则保存在电脑上。Provider 凭据进入操作系统凭据库，Listener 不内置任何服务商 Key。调试录音是可选功能，默认关闭。

日常听写不依赖 Listener 自营账号服务。选择云端 Provider 时，识别音频只会发送给你选择的服务；选择本地引擎时，识别留在电脑上完成。

## 平台说明

Windows 是主要发布平台，同时支持电脑麦克风和 Listener 键盘。macOS 12+ 支持电脑麦克风、Apple 与本地识别、全局快捷键和辅助功能插入。Linux 目前面向开发者；麦克风听写可以使用，快捷键行为取决于 X11 或 Wayland 桌面环境。

Windows 标准安装包会把文字写入当前焦点框，不会把 Listener 安装成系统输入法。单独的 TSF 路径仍用于工程验证。

## 这个仓库

Listener 分成三个仓库维护：

- **Listener Type** 负责录音会话、识别、文字处理、插入、历史、设置和桌面端设备体验。
- [**Listener Firmware**](https://github.com/Listener-ai-Macau/Listener-Firmware) 负责麦克风采集、BLE 音频与 HID、实体控制、灯、电池与电源、诊断和设备 OTA。
- [**Denzic Platform**](https://github.com/Listener-ai-Macau/Denzic-Platform) 维护桌面端与固件共用的版本化协议和可移植状态机。

桌面应用使用 Tauri 2、Rust、React、TypeScript 和 Vite。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整构建还需要固定版本的 `third_party/denzic-platform` 子模块。开发流程见 [CONTRIBUTING.md](CONTRIBUTING.md)，问题报告见 [SUPPORT.md](SUPPORT.md)，安全问题请按 [SECURITY.md](SECURITY.md) 私下提交。

版本说明和校验值保存在 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases)。Listener Type 采用 [Apache License 2.0](LICENSE) 开源；由其他开源项目改编的部分仍遵循原始许可，详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
