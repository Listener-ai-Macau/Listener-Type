<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

在正在工作的地方直接说，文字回到当前光标。Listener Type 把语音变成文字，还可以按需要去口癖、补标点、整理结构或翻译。Windows、macOS、Linux 使用电脑麦克风就能运行；配上 Listener 语音键盘后，多了实体录音控制、蓝牙收音、状态灯和设备设置。

[English](README.md) · [繁體中文](README.zh-TW.md) · [下载](https://github.com/Listener-ai-Macau/Listener-Type/releases) · [使用说明](docs/USAGE.md) · [功能目录](docs/product/features.md)

<p align="center">
  <img src="docs/assets/readme/overview.png" alt="Listener Type 主界面：识别、模型、设备和使用状态" width="900" />
</p>

## 一个产品，两个仓库

| 部分 | 负责什么 | 仓库 |
| --- | --- | --- |
| Listener Type | 录音会话、语音识别、文字处理、光标插入、设置、历史和桌面端设备体验 | 当前仓库 |
| Listener Firmware | 麦克风采集、BLE 音频与 HID、按键、旋钮、灯、电池与电源管理、诊断和 OTA | [Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware) |

没有语音键盘，软件也能独立使用。键盘扩展的是同一条听写流程；它本身不负责把语音识别成文字。

## 30 秒开始使用

1. Windows 从 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 安装最新 MSI；macOS/Linux 可以从源码构建。
2. 允许麦克风权限，在设置 → 录音中选择“电脑麦克风”，并选一个识别引擎：填入云服务 API Key，或下载本地模型（macOS 上 Apple Speech 开箱即用）。
3. 光标点进输入框，Windows 按一下右 Ctrl，说话，再按一下；macOS 默认是右 Option。

结果会插回原来的光标位置。目标应用不允许直接插入时，Listener 会把文字保留在剪贴板并提示粘贴。录音中按 `Esc` 可以取消。

## Listener 包含的完整能力

| 范围 | 功能 |
| --- | --- |
| 听写 | 切换式录音、手动停止、自动结束、取消、胶囊实时预览、电脑麦克风和 Listener BLE 音频 |
| 识别 | 火山引擎流式、OpenAI 兼容批量 ASR、Apple Speech、百炼实时、macOS Qwen 本地识别、Windows Foundry Local Whisper |
| 文字处理 | Raw、Light、Structured、Formal；语气词清理、标点、纠错规则、翻译、自定义风格包和 ZIP 导入导出 |
| 个人词库 | 人名、术语、缩写和热词；供支持的识别及润色服务使用 |
| 后续处理 | 对选中文字提问、给上一条结果切换风格、可配置全局快捷键 |
| 输出 | 当前焦点框插入、剪贴板兜底、可选 Windows TSF 验证路径、macOS 辅助功能插入 |
| 历史 | 本地会话历史、上一条结果、保留期限和可选调试录音 |
| Provider | 自备云端 Key 或使用本地引擎；每个服务可选直连、系统代理或自定义代理 |
| 桌面体验 | 托盘、开机启动、单实例、深色模式、更新界面、权限状态和诊断包导出 |
| 语音键盘 | 配对与健康状态、四颗自定义键、按压/旋转旋钮、灯光亮度、低功耗时间、电量、固件在线升级（OTA）和设备恢复 |

## 从说话到光标

<p align="center">
  <img src="docs/assets/readme/flow.png" alt="对着键盘说话，Listener 识别并整理，文字落在光标处" width="900" />
</p>

> **你说**：「呃明天那个，就是，下午三点的会，帮我记一下」
> **你得到**：「明天下午三点的会，帮我记一下。」

Light 清理独立语气词并补标点，同时保留原意；Structured 整理需求和笔记；Formal 收拾商务表达；Raw 尽量保持识别原文；翻译按已选目标语言输出。润色服务不可用时，Listener 会保住可用的识别原文，不让整段听写丢失。

## Listener 语音键盘

<p align="center">
  <img src="docs/assets/readme/keyboard-front.jpg" alt="Listener 语音键盘：四颗透明键帽、金属旋钮和状态灯" width="900" />
</p>

在设置 → 设备中配对。单击旋钮开始或停止，双击重置配对，长按关机，旋转可调音量或亮度。KEY1–KEY4 的单击、双击、长按都能配置动作。PWR、BLE、REC、AI、OK、WARN 六颗灯替设备说话，完整「灯语」见固件仓库的[灯效说明](https://github.com/Listener-ai-Macau/Listener-Firmware/blob/master/README.zh.md#灯在说什么)。

自动语音开始可以等待自定义唤醒词，并可录三段引导声纹作为输入保护。声纹不是身份认证。固件可以在 Listener Type 中 OTA，正常升级会保留配对和设备设置。

详细操作见[语音键盘手册](docs/quickstart/voice-keyboard-readme.md)和[固件仓库](https://github.com/Listener-ai-Macau/Listener-Firmware)。

<p align="center">
  <img src="docs/assets/readme/recording-settings.png" alt="Listener Type 录音设置" width="720" />
</p>

## 本地优先

设置、历史、词库、风格和纠错规则保存在电脑上。Provider 凭据进入系统凭据库，应用不内置任何服务商 Key。可选调试录音默认关闭。诊断包用于报告产品和连接状态，设计上不包含 API Key、录音和转写正文。

核心使用链路不依赖 Listener 自营后端。远程市场和账号功能只有显式配置兼容后端后才启用。

## 平台支持与当前边界

| 平台或能力 | 当前范围 |
| --- | --- |
| Windows | 主要发布路径；支持电脑麦克风、Listener BLE 音频与设备控制、安装包和光标插入 |
| macOS 12+ | 电脑麦克风、Apple/本地识别路径、全局快捷键和辅助功能插入 |
| Linux | 电脑麦克风；X11 全局快捷键尽力支持，Wayland 使用桌面环境绑定 CLI 命令 |
| 自动唤醒与声纹 | 用于输入便利和降低干扰；远距离或嘈杂环境要复核最终文字 |
| 多人或重叠说话 | 仍在持续修复，1.0.5 不保证可靠过滤 |
| Windows 输入法 | 标准安装包向焦点框插入文字，不会把 Listener 注册成系统输入法 |

这些边界属于产品说明的一部分，详细行为见[功能目录](docs/product/features.md)。

## 版本脉络

| 版本 | 产品进展 |
| --- | --- |
| 1.0.1 | 收拢为 Listener 独立桌面产品，形成首套识别、预览和安装包链路 |
| 1.0.3 | 完成唤醒灵敏度与误触发调整，并配套固件音频传输路径 |
| 1.0.4 | 建立单人唤醒、停顿续说、自动结束和文字插入基线；多人隔离仍是实验能力 |
| 1.0.5 | 改善唤醒后正文连续性、胶囊与终稿分离、轻声唤醒和键盘反馈；远距离与多人重叠仍在修复 |

准确安装包、校验值和逐版改动以 [GitHub Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 为准。

## 下载与文档

- [最新发布](https://github.com/Listener-ai-Macau/Listener-Type/releases)
- [1.0.5 发布说明](docs/release/1.0.5.md)
- [使用说明](docs/USAGE.md)
- [产品功能目录](docs/product/features.md)
- [语音键盘与设备恢复](docs/quickstart/voice-keyboard-readme.md)
- [安全策略](SECURITY.md)

## 从源码构建

Listener Type 使用 Tauri 2、Rust、React、TypeScript 和 Vite。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整桌面构建还需要 `third_party/denzic-platform` 子模块。贡献前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。本仓库以 [Apache-2.0 许可证](LICENSE) 开源。
