# Listener 语音键盘 README

Listener 语音键盘是一个 BLE 桌面语音输入设备。它用一个可按压旋钮触发录音，用四个按键发送 WASD 快捷键，适合内部测试从安装、配对到第一次语音试玩的完整路径。

## 硬件要求

| 项目 | 要求 |
|---|---|
| 电脑系统 | Windows 10 1903+ 或 Windows 11 |
| 蓝牙 | Bluetooth 4.2+ |
| USB | USB-C 充电线和可用 USB 电源 |
| 网络 | 云端 ASR/LLM 需要联网；Demo 模式可无 Key 体验 |
| 设备名 | Windows 蓝牙中显示为 `listener` |

## 安装

内部测试下载入口：`docs/quickstart/voice-keyboard-download.md`

- MSI：`.artifacts/windows-msvc/ListenerType_1.3.3_x64_en-US.msi`
- MSI SHA256：`C69A5E7F0229366FFF677DB06D0C4121B8AF984C3A750482F4A3942CEB66E93F`
- 便携 ZIP：`.artifacts/windows-msvc/ListenerType_1.3.3_x64_portable.zip`
- ZIP SHA256：`AA0CEA6D88FC215A8899E5172D6D2A529091D3AEA949BF5EB994E32FA1C7DCD8`

安装步骤：

1. 双击 MSI。
2. 如果 Windows 显示未知发布者，核对文件名和 SHA256 后继续。
3. 从开始菜单启动 Listener Type。
4. 首次启动按软件提示授权麦克风/热键并进入配对。

## 配对

1. 确认设备已充电并开机。
2. 在 Listener Type 中点击「开始配对」。
3. 在 Windows 蓝牙窗口选择 `listener`。
4. 软件显示已连接后即可开始录音。

如果搜索不到设备，先在 Windows 蓝牙设置中删除旧的 `listener` 记录，再重新配对。

## 使用

| 操作 | 功能 |
|---|---|
| 按下 EC11 旋钮 | 开始或停止语音录音 |
| 旋转 EC11 旋钮 | 调节电脑音量 |
| KEY1-KEY4 | WASD 快捷键 |
| 电脑键盘 `Esc` | 取消当前录音或处理中任务 |

KEY1-KEY4 不触发语音录音。语音录音键是 EC11 旋钮的按压动作。

## 恢复和故障排除

| 现象 | 处理 |
|---|---|
| 搜索不到 `listener` | 充电、重启设备，确认蓝牙已打开 |
| 配对失败或反复配对 | 删除 Windows 旧配对记录，再从 Listener Type 重新开始配对 |
| 设备连上但录音没反应 | 确认按的是 EC11 旋钮，不是 KEY1-KEY4 |
| 有录音但没有文字 | 检查 ASR/API Key、网络，或进入 Demo 模式 |
| 软件卡住 | 退出 Listener Type 后重新启动 |
| 仍无法恢复 | 设置 -> 关于 -> 导出诊断包，并附上安装包哈希和复现步骤 |

当前内部测试恢复路径只覆盖桌面端/Windows 侧忘记配对和重新配对；不要把它描述成已实现的设备端硬件恢复出厂。

## 隐私与安全

- 录音会传到本机 Listener Type，再按你的配置交给 ASR 服务商处理。
- API Key 保存在本机配置或凭证存储中。
- 诊断包不包含录音、转写文本或 API Key。
- 建议 13 岁以上使用；不是玩具、医疗设备、心理治疗工具或儿童教育产品。

## 相关文档

- 快速开始：`docs/quickstart/voice-keyboard-quickstart.md`
- 下载入口：`docs/quickstart/voice-keyboard-download.md`
- 安装说明：`docs/quickstart/installation.md`
- 使用指南：`docs/USAGE.md`
- 设备手册：`!docs/product/voice_keyboard_user_manual.md`
