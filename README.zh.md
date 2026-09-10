<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="128" />
</p>

<h1 align="center">Listener Type</h1>

<p align="center">
  <strong>说话，文字出现在当前光标。</strong><br/>
  聊天、邮件、文档、代码注释：按一下，说，再按一下。
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="docs/product/features.md">产品功能</a> ·
  <a href="docs/USAGE.md">使用说明</a> ·
  <a href="https://github.com/Listener-ai-Macau/Listener-Firmware">语音键盘固件</a>
</p>

Listener Type 是 Listener 的桌面软件。它可以单独用电脑麦克风试用；配上 [Listener 语音键盘](https://github.com/Listener-ai-Macau/Listener-Firmware) 之后，开始、停止和状态都在手上完成。

它不是会后录音笔，也不是「说完替你写成完美文章」的写作机器人。它是每天能用的**输入方式**：话说完，字已经在光标里。

## 为什么值得买

软件可以先装。完整体验是键盘 + Type 一条链：

| 你现在怎么输入 | Listener |
| --- | --- |
| 打字打断思路 | 眼睛留在屏幕上，手按一下旋钮 |
| 语音留在录音 App 里再复制 | 直接写进当前窗口 |
| 云厂商锁死识别和润色 | 自己的 Key，或本机识别；设置留在电脑上 |
| 只有软件热键 | 真键盘、真灯、唤醒词，桌面设备 |

1.0.5 已经跑通日常闭环：开始 → 说话 → 胶囊和灯有反馈 → 文字回到光标。插不进去时留在剪贴板。这是可以卖、可以天天用的基线，不是演示片。

## 30 秒上手

```text
光标放进输入框
    → 按右 Ctrl（有键盘就按 EC11）
    → 说话
    → 再按一次
    → 看光标，不要看我们的编辑器
```

没有键盘：设置 → 录音 → 输入源选麦克风。  
有键盘：配对名 `listener`，Windows 上走完整 BLE 音频。

逐步说明：[使用说明](docs/USAGE.md) · [语音键盘手册](docs/quickstart/voice-keyboard-readme.md)

## 能做什么

| | |
| --- | --- |
| **写进任何输入框** | 记事本、浏览器、聊天、编辑器。默认切换式录音，不是按住不放。 |
| **手上的键盘** | EC11 单击开始/停止，双击重新配对，四颗键可自定义。REC / AI / BLE 灯告诉你进行到哪。 |
| **按场景整理** | Raw 保原话，Light 去口癖，Structured 理任务，Formal 写邮件，Shift 走翻译。 |
| **词还是你的词** | 本机词库喂给识别和润色。风格包可复制、导出，不依赖远程市场。 |
| **尽量只留下你的话** | 唤醒词默认「开始录音」。可选三遍声纹。受控环境下，旁人播放不应单独写入正文。 |
| **本地优先** | 历史、设置、词库在本机。不内置开发者 Key。本机 Whisper 可用，中文效果通常不如云端。 |

完整功能、灯的含义、1.0.5 不承诺什么：[产品功能](docs/product/features.md)

## 买什么、装什么

- **先用软件**：安装 Listener Type。Windows 是完整硬件音频的主平台；macOS 可先用麦克风和快捷键。
- **再买键盘**：Listener 语音键盘。固件开源，见 [Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware)。
- **识别服务**：火山引擎等云端 ASR 用你自己的账号；也可以切本机识别。服务质量和费用由你选的服务商决定。

当前版本是 **1.0.5**。主人声纹在更远、更乱的场景里，以及真人多人同时说话的泛化，放到 1.0.6。一次受控干扰通过，不等于任何房间都 100% 干净。

## 开源

欢迎阅读、构建、提 issue 和 PR。产品功能怎么写，见 [怎么写产品功能](docs/product/writing.md)。

源码和文档在本仓库。`Listener Type` 名称、图标和吉祥物不随源码自动授权给改名发行版。完整桌面构建还依赖 `third_party/denzic-platform` 子模块；贡献前请读 [CONTRIBUTING.md](CONTRIBUTING.md)。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

发布通道：[Listener-ai-Macau/Listener-Type](https://github.com/Listener-ai-Macau/Listener-Type)
