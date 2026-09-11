<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type 是个桌面听写软件。把光标点进一个输入框,按一下右 Ctrl,说话,再按一下,转写出来的文字就打进了那个框。聊天窗口、浏览器、编辑器、邮件,凡是能打字的地方都行。目标软件不让程序输入时,文字会留在剪贴板里。

软件免费、开源,电脑上现有的麦克风就能用。配套的硬件是 [Listener 语音键盘](https://github.com/Listener-ai-Macau/Listener-Firmware):一个桌面小设备,固件在隔壁仓库,但没有键盘也能正常用。

[English](README.md) · [繁體中文](README.zh-TW.md) · [使用说明](docs/USAGE.md) · [1.0.5 发布说明](docs/release/1.0.5.md)

### 安装

Windows 是主平台:从 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下载 MSI 装上即可。安装包依赖 WebView2,联网时会自动补齐。

| | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 麦克风听写 | ✓ | ✓(12+) | ✓ |
| 全局快捷键 | ✓ | ✓ | X11 尽力而为;Wayland 要在桌面环境里绑定 |
| 键盘蓝牙音频 | ✓ | 还没有 | 还没有 |

### 用法

录音是切换式的,不用按住。按一下右 Ctrl(macOS 是右 Option)开始,说完再按一下。工作期间屏幕上有颗小胶囊,显示进行到哪一步:录音、转写、处理、完成。中途反悔按 `Esc` 取消,不会插入半截文字。

输出成什么样,看你选了哪种风格。Raw 尽量保留原话;Light 去口癖、补标点;Structured 和 Formal 分别面向笔记和邮件;设了目标语言之后,Shift 把下一段录音标记成翻译。风格就是本地文件,内置的几个可以复制出来随便改,也能导出成 ZIP 分享。

另外几个内置快捷键:`Ctrl+Shift+;` 对选中的文字提问,`Ctrl+Shift+S` 给上一段结果换一种风格,`Ctrl+Shift+O` 唤起应用。macOS 上把 `Ctrl` 换成 `Cmd`。

### 工作原理

音频从麦克风(或蓝牙连着的键盘)进到识别,转写结果可选地过一遍风格,然后打进焦点框。Windows 上插入走一个小的 TSF 文本服务,macOS 走辅助功能接口;目标软件两边都不让插时,文字留在剪贴板。

服务商没有锁定。识别可以走火山引擎的流式接口、任意 OpenAI 兼容端点、Apple Speech,或者完全在本机跑;风格可以接 Ark、DeepSeek,或任意 Anthropic / OpenAI 兼容端点。API Key 存在系统凭据库,软件不带任何 Key;历史、词库、风格、设置都不出这台电脑,整条流程不依赖 Listener 的服务器。

本地词库(人名、产品名、缩写)会同时喂给识别和风格两步。同事的名字就是这么不再被写错的。

### 键盘

Listener 键盘是个 USB-C 小设备:一颗能按的旋钮(开始/停止,双击重新配对,转动调音量)、四颗可以在软件里绑动作的键、六盏分别表示电源、蓝牙、录音和处理状态的灯。

配上键盘还能动口开工:打开「检测到人声后自动开始」,设备会先等唤醒词(默认「开始录音」)。再照引导录三遍自己的声音,旁人播放的语音就不会混进正文。这是防误录的输入保护,不是身份认证,别拿它当锁用。

没有键盘软件一样好用,它只是把录音键放到你手指底下。固件开源:[Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware)。

### 目前的边界(1.0.5)

键盘的蓝牙音频只在 Windows 上验收过,macOS 和 Linux 先用电脑麦克风。远距离拾音和多人同时说话还不可靠,排在 1.0.6。另外它不是系统输入法:文字进当前焦点框。

完整功能清单(包括每条做不到什么)在 [docs/product/features.md](docs/product/features.md)。

### 从源码构建

应用是 Tauri v2:React + TypeScript 前端,Rust 后端,外加一个负责 Windows 插入的原生文本服务。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面构建还需要 `third_party/denzic-platform` 子模块,其余见 [CONTRIBUTING.md](CONTRIBUTING.md);安全问题发 [SECURITY.md](SECURITY.md)。源码开放,但 Listener Type 的名字、图标和吉祥物不授权给改名后的分支。
