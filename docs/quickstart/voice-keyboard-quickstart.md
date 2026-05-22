# Listener 语音键盘快速开始

一句话：安装 Listener Type，配对名为 `listener` 的设备，按下旋钮说话，再按一次得到文字。

## 1. 安装软件

内部测试下载入口：`docs/quickstart/voice-keyboard-download.md`

- 推荐安装包：`.artifacts/windows-msvc/ListenerType_1.3.3_x64_en-US.msi`
- SHA256：`C69A5E7F0229366FFF677DB06D0C4121B8AF984C3A750482F4A3942CEB66E93F`
- 备用便携版：`.artifacts/windows-msvc/ListenerType_1.3.3_x64_portable.zip`

双击 MSI 安装。内部测试版可能出现 Windows「未知发布者」提示，请先核对文件名和 SHA256，再选择继续运行。

## 2. 上电并配对

1. 用 USB-C 给设备充电；首次使用建议先充 1-2 小时。
2. 打开 Listener Type。
3. 首次启动会进入配对引导，点击「开始配对」。
4. 在 Windows 蓝牙窗口选择 `listener`。
5. 软件显示已连接后，把光标放到记事本、浏览器或任意文本框。

## 3. 第一次试玩

1. 按一下 **EC11 旋钮**，屏幕出现录音提示后开始说话。
2. 再按一下 **EC11 旋钮**，结束录音并等待文字出现。
3. 旋转 EC11 旋钮可调节电脑音量。
4. KEY1-KEY4 是 WASD 快捷键，不是语音录音键。

可以试着说：

| 你说 | 可能得到 |
|---|---|
| 今天要做三件事 | 今天要做三件事 |
| 帮我把这句话改得轻松一点 | 更自然的改写结果 |
| 我还没有 API Key | 进入 Demo 或测试音频路径 |

## 4. 取消、恢复和重试

| 场景 | 操作 |
|---|---|
| 录音中说错了 | 按电脑键盘 `Esc` 取消本次录音 |
| 结果不满意 | 重新按 EC11 旋钮录一遍 |
| 蓝牙配对混乱 | Listener Type 设置 -> 关于 -> 忘记已配对设备，然后重新配对 |
| 软件无法打开 | Windows 设置 -> 蓝牙和设备，删除 `listener` 后重试 |
| 没有 API Key 或设备 | 使用 Demo 模式先体验 |

## 5. 隐私和安全

- 录音会传到本机 Listener Type，再按你的配置交给 ASR 服务商处理。
- API Key 保存在本机配置或凭证存储中。
- Listener Type 诊断包不包含录音、转写文本或 API Key。
- 建议 13 岁以上使用；本产品不是玩具、医疗设备、心理治疗工具或儿童教育产品。

更多说明见 `docs/quickstart/voice-keyboard-readme.md` 和 `docs/USAGE.md`。
