<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="128" />
</p>

<h1 align="center">Listener Type</h1>

<p align="center">
  <strong>开口说话,字就到光标。</strong><br/>
  开源的桌面语音输入——在你正在用的任何软件里。
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <strong>简体中文</strong> ·
  <a href="README.zh-TW.md">繁體中文</a>
</p>

<p align="center">
  <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases"><img src="https://img.shields.io/github/v/release/Listener-ai-Macau/Listener-Type" alt="Release" /></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-blue" alt="Platform" />
</p>

<p align="center">
  <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases">下载</a> ·
  <a href="docs/USAGE.md">使用说明</a> ·
  <a href="docs/product/features.md">产品功能</a> ·
  <a href="docs/release/1.0.5.md">发布说明</a> ·
  <a href="https://github.com/Listener-ai-Macau/Listener-Firmware">键盘固件</a>
</p>

<!-- 头图:有产品截图或演示动图后,放在这里。 -->

Listener Type 是一款本地优先的听写软件:按一下,说话,再按一下——字已经打在你刚才的输入框里。它不是录音笔,也不是聊天窗口:字去的是光标所在的地方。

它免费、开源(Tauri + React + Rust)。有电脑麦克风就能开始用;配上 [Listener 语音键盘](https://github.com/Listener-ai-Macau/Listener-Firmware)——一个带旋钮、实体键和状态灯的桌面小设备——开始、停止、「它到底听到没有」,都在手上完成,眼睛不用离开屏幕。

## 30 秒上手

**Windows:** 从 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下载 MSI,安装,允许使用麦克风。

```text
光标点进任意输入框
  → 按右 Ctrl(有键盘就按一下旋钮)
  → 说话
  → 再按一次
  → 字已经在光标处
```

没有键盘:设置 → 录音 → 输入源选「麦克风」。
有键盘:在蓝牙里配对称作 `listener` 的设备——目前完整的键盘音频链路以 Windows 为主,macOS 和 Linux 先用电脑麦克风。

详细步骤:[使用说明](docs/USAGE.md) · [语音键盘手册](docs/quickstart/voice-keyboard-readme.md)

## 用起来不一样的地方

| | |
| --- | --- |
| **在哪儿都能写** | 记事本、浏览器、聊天、代码编辑器——字直接落进目标软件;个别软件不让插,就先放进剪贴板。按一下开始、再按一下结束,不用一直按住。 |
| **手上有台键盘** | 单击旋钮开始/停止,双击重新配对,四颗键随你绑定。PWR / BLE / REC / AI 几盏灯,进行到哪一步一眼看清。 |
| **按场合换语气** | 原文照录、轻度整理、结构化纪要、正式邮件——按 Shift 还能直接翻译。语气风格就是本地文件,随改随导,不用登录任何商店。 |
| **你的词它认识** | 人名、产品名、行话写进本地词库,识别和整理都会参考。 |
| **动口就开工** | 说一句「开始录音」就开始。可选录三遍声纹:别人在你旁边播放的声音,进不了正文。 |
| **服务你选,钥匙你拿** | 识别可接火山引擎、OpenAI 兼容接口、Apple Speech,也能完全在本机跑;润色可接 Ark、DeepSeek、Anthropic 兼容接口。Key 存在系统凭据里,软件不内置任何 Key。 |
| **本地优先** | 历史、风格、词库、设置都在你自己电脑上;没有任何 Listener 服务器也照常用。 |

功能全表、灯的含义、平台差异,见[产品功能](docs/product/features.md)。

## 1.0.5 不承诺什么

- 键盘的蓝牙音频链路目前以 Windows 为主,macOS 和 Linux 先用电脑麦克风。
- 声纹是防止误录旁人说话的输入保护,不是身份认证,别当门锁用。
- 远距离声纹、多人同时说话的隔离是 1.0.6 的事,这一版没有。
- 它不是系统输入法,是把文字插入当前焦点的输入框。

## 从源码构建

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面构建还需要 `third_party/denzic-platform` 子模块。提交代码前请读 [CONTRIBUTING.md](CONTRIBUTING.md);安全问题请走 [SECURITY.md](SECURITY.md)。

源码可以自由阅读、构建、修改;但 Listener Type 的名称、图标和吉祥物,不随源码授权给改名后的发行版。
