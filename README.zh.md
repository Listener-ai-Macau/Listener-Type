<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

说话,它帮你打字。光标点进任意输入框,按一下右 Ctrl,说,再按一下——字出现在那个框里,不在我们的窗口里。

Windows、macOS、Linux 都能跑,电脑麦克风拿来就能用。想要桌上有颗实体录音键的话,可以配 [Listener 语音键盘](https://github.com/Listener-ai-Macau/Listener-Firmware)。

[English](README.md) · [繁體中文](README.zh-TW.md) · [使用说明](docs/USAGE.md) · [发布说明](docs/release/1.0.5.md)

## 怎么用

Windows 上从 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下载 MSI,安装,允许麦克风。设置就这么多。

之后永远是同一个动作:点进输入框,按右 Ctrl,说话,再按一次。碰到不让插入文字的软件,结果会先放在剪贴板里。macOS 和 Linux 的热键换成右 Option / 右 Alt,细节看[使用说明](docs/USAGE.md)。

## 能做什么

- 写进当前焦点的软件:记事本、浏览器、聊天、代码编辑器都行。
- 把说过的话收拾好:原文照录、轻度整理、列成条目、改成正式邮件,按 Shift 还能翻译。风格就是本地文件,随便复制随便改。
- 记住你的用词:人名、行话写进本地词库,识别和整理都会参考。
- 动口开工:说一句「开始录音」就开始。录入三遍声纹之后,别人说话——或者放你的录音——都不会触发。
- 跑在你自己的账号上。识别可以走火山引擎、OpenAI 兼容接口、Apple Speech,也可以完全离线;整理可以接 Ark、DeepSeek、Anthropic 兼容接口。Key 存在系统凭据里,软件不自带任何 Key。
- 东西都留在本机:历史、风格、词库、设置,不出这台电脑。

完整清单和平台差异在 [docs/product/features.md](docs/product/features.md)。

## 关于键盘

[Listener 语音键盘](https://github.com/Listener-ai-Macau/Listener-Firmware)是个 USB-C 小设备:一颗能按的旋钮管开始停止,四颗键随便绑,几盏灯分别管电源、蓝牙、录音、处理。固件同样开源。不是必需品——用麦克风就挺好——但一颗真按键总比摸快捷键强。

## 还做不到的

- 键盘的蓝牙音频目前只有 Windows;macOS 和 Linux 先用电脑麦克风。
- 声纹是防误触发的,不是安全功能。
- 离麦克风太远、或者两个人同时说话,还是处理不好。排在 1.0.6。
- 不是系统输入法,是把字打进当前焦点框。

## 自己构建

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面构建还需要 `third_party/denzic-platform` 子模块。提交代码前看 [CONTRIBUTING.md](CONTRIBUTING.md),报告漏洞看 [SECURITY.md](SECURITY.md)。

代码是开源的;Listener Type 的名字、图标和吉祥物不是——改了名的分支请别再用它们。
