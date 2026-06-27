<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type app icon" width="128" />
</p>

<h1 align="center">Listener Type</h1>

<p align="center">
  <strong>面向 macOS 和 Windows 的本地优先语音输入应用。</strong><br/>
  按快捷键，说话，把整理后的文字插入当前光标。
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="docs/USAGE.md">使用指南</a> ·
  <a href="specs/ARCHITECTURE.md">架构</a> ·
  <a href="specs/traceability/files.md">代码追踪</a>
</p>

## 核心能力

Listener Type 做一件事：把语音变成当前光标处可直接使用的文字。它包含原文转写、轻度润色、结构化 prompt、正式表达、翻译热键、词典热词、历史记录、风格包、状态胶囊，以及选中文本后的语音问答面板。

它默认本地优先：

- 设置、词典、历史、录音、风格包保存在本机。
- Provider 凭据保存在系统凭据库。
- ASR/LLM 云服务采用用户自己的 key。
- 在 Listener Type 后端上线并配置前，远端市场和 GitHub OAuth 默认禁用。

## 品牌形象

Listener Type 的品牌形象是一只趴在语音胶囊上的小狗伙伴。它采用极简圆润的设计：下垂圆耳、开心眯眼、柔和腮红、圆鼓鼓的小爪子和亲近的表情，让语音输入这件事显得温暖、轻松，而不是冰冷的工具。

小狗下方的胶囊来自应用内录音时的浮动胶囊。胶囊内部保留五根圆角竖条，对应录音时随音量变化的 audio bars 动效；中间一根使用温暖琥珀色作为唯一强调。整体配色延续当前 UI：石英白背景、低饱和鼠尾草绿线条、深墨色五官、柔和灰线和克制的暖色点缀。

品牌源图保存在 [docs/assets/brand/listener-ai-app-icon.png](docs/assets/brand/listener-ai-app-icon.png)，桌面端生成图标位于 [src-tauri/icons](src-tauri/icons)。

## 当前状态

- Tauri 2 + Rust 后端 + React/TypeScript 前端。
- 目标平台：macOS 12+、Windows 10+。
- ASR：火山引擎流式 ASR、OpenAI 兼容批量 ASR、Apple Speech、本地 Qwen ASR、Windows Foundry Local。
- 润色：Ark、DeepSeek/OpenAI 兼容、Anthropic 兼容、自定义 OpenAI 兼容端点。
- Windows 插入：原生 hook 路径 + Listener Type TSF IME。
- 自动更新指向 [Listener-ai-Macau/Listener-Type](https://github.com/Listener-ai-Macau/Listener-Type)。

## 构建

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

发布前运行：

```bash
npm run check:brand
npm run check:cloud
npm run check:traceability
```

## 文档入口

- [使用指南](docs/USAGE.md)
- [安装](docs/quickstart/installation.md)
- [权限](docs/quickstart/permissions.md)
- [火山引擎配置](docs/setup/volcengine.md)
- [Windows 构建](docs/platform/windows-build.md)
- [Windows IME](docs/platform/windows-ime.md)
- [架构](specs/ARCHITECTURE.md)
- [设计](specs/DESIGN.md)
- [代码追踪](specs/traceability/files.md)
