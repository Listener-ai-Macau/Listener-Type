# Listener Type 项目约定

- 后台开发不得随意关闭用户正在用的 Type，也不得抢输入焦点。必要更新等录音收尾完成后进行，随后立即恢复主进程和唤醒助手。
- 蓝牙修复须实际使用 nRF52840 Dongle 抓包器，和 Windows HCI、固件日志对齐后定位；先确认故障段捕获有效，不以漏抓代替设备丢包证据，不盲调参数。覆盖吞吐、音频、改名、连接、重连、状态同步；已通过的无关项目不重复测。
- 防回退索引：`scripts/regression-guard-commits.json`；用 `python scripts/regression-history.py trace --feature <功能>` 查历史。
- 待办与完整开发时间线：`C:/Users/Billy/Documents/Codex/2026-10-04/listener-repair-handoff/outputs/Listener-修复进度.md`。

- 蓝牙防回退死约束：尝试前先查既有索引和时间线；已否决方案只有实际条件出现新证据才可重试，并写明差异。未触发、未生效或采集不完整必须保留为未验证，不能算修好或提速；当前工作点及时更新，旧失败不删除。
