# Listener 语音键盘内部测试下载入口

本页是 `voice-keyboard-production-readiness / 3.4` 的内部测试下载入口。正式公开发布前，包装二维码可指向公开发布页；当前内部测试以本地构建产物和哈希为准。

## Windows 产物

| 产物 | 路径 | SHA256 |
|---|---|---|
| MSI 安装包 | `.artifacts/windows-msvc/ListenerType_1.3.3_x64_en-US.msi` | `C69A5E7F0229366FFF677DB06D0C4121B8AF984C3A750482F4A3942CEB66E93F` |
| 便携 ZIP | `.artifacts/windows-msvc/ListenerType_1.3.3_x64_portable.zip` | `AA0CEA6D88FC215A8899E5172D6D2A529091D3AEA949BF5EB994E32FA1C7DCD8` |

## 安装前检查

1. 确认文件名和 SHA256 与上表一致。
2. 双击 MSI 安装；如果 Windows SmartScreen 提示未知发布者，确认哈希后继续。
3. 从开始菜单启动 Listener Type。
4. 按 `docs/quickstart/voice-keyboard-quickstart.md` 完成配对和第一次试玩。

## 失败时记录

- Windows 版本。
- 使用 MSI 还是 ZIP。
- 失败发生在安装、启动、配对、录音、转写还是插入。
- 安装包 SHA256。
- Listener Type 诊断包。
