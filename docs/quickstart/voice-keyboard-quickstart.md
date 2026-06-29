# Listener 语音键盘快速开始

一句话：安装 Listener Type，配对名为 `listener` 的设备，按 KEY3 说话，再按一次得到文字。

## 1. 安装软件

1.0.1 下载入口：`docs/quickstart/voice-keyboard-download.md`

- 推荐安装包：`.artifacts/windows-msvc/ListenerType_1.0.1_x64_en-US.msi`
- SHA256：`174E92566785FC4D9E488D51B5BD2022DC1F94B23147E1EC5C7ACE4A310BA08B`
- 备用便携版：`.artifacts/windows-msvc/ListenerType_1.0.1_x64_portable.zip`

双击 MSI 安装。未签名构建可能出现 Windows「未知发布者」提示，请先核对文件名和 SHA256，再选择继续运行。

## 2. 上电并配对

1. 用 USB-C 给设备充电；首次使用建议先充 1-2 小时。
2. 打开 Listener Type。
3. 首次启动会进入配对引导，点击「开始配对」。
4. 在 Windows 蓝牙窗口选择 `listener`。
5. 软件显示已连接后，把光标放到记事本、浏览器或任意文本框。

## 3. 第一次试玩

1. 按一下 **KEY3**，屏幕出现录音提示后开始说话。
2. 再按一下 **KEY3**，结束录音并等待文字出现。
3. 双击 **EC11 旋钮** 可重置蓝牙配对。
4. EC11 旋钮是开机键；开机后单击执行 EC11 自定义动作，默认切换风格，旋转默认调节电脑音量，可在 Listener Type 设置里改成屏幕亮度或禁用。
5. KEY1-KEY4 和 EC11 单击是自定义动作键，默认 KEY3 是语音录音键。EC11 单击的底层入口是 `Shift+F13`；不要使用 F25 作为替代入口。

可以试着说：

| 你说 | 可能得到 |
|---|---|
| 今天要做三件事 | 今天要做三件事 |
| 帮我把这句话改得轻松一点 | 更自然的改写结果 |
| 我还没有 API Key | 先配置本地 ASR，或补齐云端服务商凭据 |

## 4. 取消、恢复和重试

| 场景 | 操作 |
|---|---|
| 录音中说错了 | 按电脑键盘 `Esc` 取消本次录音 |
| 结果不满意 | 重新按 KEY3 录一遍 |
| 蓝牙配对混乱 | Listener Type 设置 -> 关于 -> 设备恢复会停止当前录音/BLE 会话；打开 Windows 蓝牙后删除 `listener`，再回到录音设置重新配对 |
| 软件无法打开 | Windows 设置 -> 蓝牙和设备，删除 `listener` 后重试 |
| 没有 API Key 或设备 | 先完成服务商凭据、本地 ASR 或设备配对配置 |

## 5. 状态怎么看

| 状态 | 含义 | 下一步 |
|---|---|---|
| Ready | 设备和软件都准备好 | 按 KEY3 开始说话 |
| Recording | 正在录音 | 说完再按一次 KEY3 |
| Transferring | 正在把音频送到电脑 | 等待文字出现，不要连续按键 |
| Error | 当前步骤失败 | 按提示重试；仍失败就导出诊断包 |
| Recovery | 正在恢复连接或需要重新配对 | 打开 Windows 蓝牙删除 `listener`，再重新配对 |

恢复流程不需要终端、ESP-IDF、串口工具或 SDK。

## 6. 隐私和安全

- 录音会传到本机 Listener Type，再按你的配置交给 ASR 服务商处理。
- API Key 保存在本机配置或凭证存储中。
- Listener Type 诊断包不包含录音、转写文本或 API Key。
- 建议 13 岁以上使用；本产品不是玩具、医疗设备、心理治疗工具或儿童教育产品。

更多说明见 `docs/quickstart/voice-keyboard-readme.md` 和 `docs/USAGE.md`。
