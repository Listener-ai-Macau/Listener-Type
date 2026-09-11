<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

说话,文字出现在当前光标。Listener Type 是个桌面听写软件:光标点进任何输入框,按一下右 Ctrl,说,再按一下,字就打进去了。开源,Windows / macOS / Linux 都能装,电脑麦克风就够用。

[English](README.md) · [繁體中文](README.zh-TW.md) · [使用说明](docs/USAGE.md) · [1.0.5 发布说明](docs/release/1.0.5.md)

## 先花 30 秒试一下

1. Windows 上从 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下载 MSI 安装(要 WebView2,联网会自动补)。
2. 打开,允许麦克风。
3. 点进记事本,按右 Ctrl,说一句话,再按一次。

字应该已经在记事本里了。中途按 `Esc` 取消;某个软件不让自动输入时,文字会进剪贴板,提示你自己粘贴。macOS 上热键是右 Option。到这步能用了,再往下看。

macOS 12+ 和 Linux 也能装,先用电脑麦克风;键盘的蓝牙音频只在 Windows 上验收过。Linux 的全局热键在 X11 下尽力而为,Wayland 要在桌面环境里绑定命令,细节在[使用说明](docs/USAGE.md)。

## 一天里怎么用

**回消息。** 默认的 Light 风格:去掉「那个」「嗯」,补上标点,别的不动。说的时候是口水话,发出去是正常人话。

**写邮件和正式沟通。** 切到 Formal,语气收拾整齐,但不替你编内容。对方读另一种语言时,设好目标语言,按 Shift 再说,出来就是译文。

**记需求、任务、prompt。** Structured 按主题和目标整理成条目。写代码注释、或者要留原话的时候用 Raw,它尽量不动你的词。

**它认识你的词。** 人名、产品名、缩写加进本机词库,识别和整理都会参考——同事的名字就是这么不再被写错的。

顺手再记三个快捷键:`Ctrl+Shift+;` 对选中的文字提问,`Ctrl+Shift+S` 给上一段结果换个风格,`Ctrl+Shift+O` 唤起应用(macOS 把 `Ctrl` 换成 `Cmd`)。

## 桌上的键盘

[Listener 语音键盘](https://github.com/Listener-ai-Macau/Listener-Firmware) 是配套硬件:一颗能按的旋钮管开始和停止,双击重新配对,转一下调音量;四颗键在软件里随便绑动作;六盏灯分别说电源、蓝牙、录音、处理到哪一步。

不想碰键盘也行:打开「检测到人声后自动开始」,说声「开始录音」就开工。照引导录三遍声纹之后,旁边放别人的语音不会混进正文。声纹是防误录的,不是身份认证,别当锁用。

没有键盘软件照常用,键盘只是把录音键放到手指底下。固件也开源:[Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware)。

## 你的数据和你的 Key

识别可以走火山引擎流式接口、任意 OpenAI 兼容端点、Apple Speech,或完全在本机;润色接 Ark、DeepSeek,或任意 Anthropic / OpenAI 兼容端点。Key 存在系统凭据库,软件不自带任何 Key。历史、词库、风格、设置都在本机——整条链路不依赖 Listener 的服务器。

## 目前的边界

发的是 1.0.5。远距离拾音、多人同时说话还不可靠,排在 1.0.6。它不是系统输入法,文字进当前焦点框。完整功能清单(包括每条做不到什么)在 [docs/product/features.md](docs/product/features.md)。

## 自己动手

Tauri v2 + React + Rust;Windows 的插入走 `windows-ime/` 里的 TSF 文本服务,macOS 走辅助功能接口。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面构建还需要 `third_party/denzic-platform` 子模块。贡献见 [CONTRIBUTING.md](CONTRIBUTING.md),安全问题发 [SECURITY.md](SECURITY.md)。源码开放;Listener Type 的名字、图标和吉祥物不授权给改名后的分支。
