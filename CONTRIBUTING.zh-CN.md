# 参与开发

**简体中文** · [英文](CONTRIBUTING.md)

Listener Type 使用 Tauri 2、Rust 后端和 React / TypeScript 前端。

## 开发与验证

```bash
git submodule update --init --recursive
npm ci
npm run build
npm test
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run
```

子模块命令检出仓库记录的准确 Denzic Platform 版本。打包或更新器改动前运行发布审查：

```bash
npm run check:brand
npm run check:cloud
npm run check:traceability
npm run release:check
```

## 目录与变更要求

- `src/`：产品界面与桌面交互。
- `src-tauri/`：录音、识别、设备、文字写入、更新与平台适配。
- `docs/features/`：工程行为、实现与验证；`docs/product/` 和 `docs/quickstart/`：产品与使用说明。
- `specs/traceability/`：源码责任与追踪关系，增加或移动产品源码时同步维护。

保留本地优先行为。录音改动应保持单一会话状态归属，补充对应时序边界的回放或硬件证据。不要提交凭据、录音、转写、生成构建文件、验证日志或发布产物；验证证据写在问题或拉取请求正文。如实描述实验性唤醒、声纹分离与系统输入法路径的限制。

[返回中文软件首页](README.zh-CN.md)。
