# Luna 执行进度

执行入口：[录音体验与发布执行方案 v2](commercial-recording-controller-plan-2026-09-14.md)。

当前状态：RUNNING（用户已授权按 v2 顺序逐卡执行）。

当前没有新的安装、固件升级或公开发布。此前工作树的 handoff 草稿及其针对性测试保留，不计为本方案某阶段通过。P0 已重新核对现场，但发现源码 release exe 与安装/运行 exe 哈希不一致；在统一候选构建前，历史实机结果不计入新候选。

| 阶段 | 状态 | 完成证据 | 下一步 |
| --- | --- | --- | --- |
| P0 基线与现场 | UNVERIFIED | `work/p0-baseline-20260914.md`；固件 live build_id=`cd945a0f528cb02d`；Type runtime gate 因源码/安装哈希不一致退出 1 | P7 统一候选构建后重新过身份门禁 |
| P1 数据与判定器 | RUNNING | `work/p1-cases-20260914.json`、`work/p1-measurements-20260914.md`、`work/replay_event_gate.py`；五类注入退出码 0；数据仍缺真人边界/起音标注 | 保留缺口，继续使用历史诊断进入 P2 |
| P2 身份能力评测 | UNVERIFIED | `work/p2-identity-feasibility-20260914.md`、`work/p2-identity-scores-20260914.json`；推理 p90 27–35 ms，但短窗分数相交且无真人人工标注 | 不调阈值，进入 P3 收尾确定性修复 |
| P3 收尾等待 | UNVERIFIED | `work/p3-finalization-evidence-20260914.md`；目标 speaker 1.5s 仅限真实干扰、本地 shadow 800/850ms、processing 750–5000ms；针对性测试退出码 0 | 仍需在统一候选上验证 delayed/missing final 与 spinner/Done SLA |
| P4 归属与端点 | UNVERIFIED | `work/p4-control-entrypoints-20260914.md`；STOP/lease/preview/final 入口已收敛核对；端点 60、handoff 18、pending identity 3、foreign 8、speakerless 2、lease 17 项均通过 | 当前源码级卡完成；进入 P5 正文 ledger，P8 再做同一候选实机门 |
| P5 正文与输出 | UNVERIFIED | `work/p5-transcript-output-evidence-20260914.md`；filler 6、product_final 14、punctuation 8、preview 44、final 86、transcript 75 项通过 | ownership 细粒度 ledger、流式真实光标与同一候选实机留到 P8；当前进入 P6 |
| P6 唤醒与设备 | UNVERIFIED | `work/p6-wake-device-evidence-20260914.md`；wake 103、candidate 53、device_key 13、session_actor 5、cancel 38、negative 6 项通过；素材缺口按 ignored 保留 | 统一候选构建后做 P8 真实播放/录音和下一会话矩阵 |
| P7 候选构建 | PASS | `work/p7-candidate-20260914.md`；源码构建、MSI 打包和静态门禁退出码均为 0；安装后 runtime gate 退出 0，exe SHA-256=`ACC7862E...`，MSI SHA-256=`4667F933...` | 用这同一候选执行 P8 实机矩阵 |
| P8 实机验收 | RUNNING | `work/p8-acoustic-evidence-20260914.md`；同一安装候选已完成电脑外放→键盘→BLE→Type→历史的真实回放；自动唤醒、0 丢包、自动结束和无口令候选拒绝均有日志证据，但回放正文出现截断/混入，且上屏记录为 `windowsImeTsfRequired` | 修正文连续性与上屏链路；另用真实第二说话人/电视尾音完成 G，不把同一录音外放误当声纹通过 |
| P9 发布准备 | NOT_STARTED | 无 | 补足真人覆盖和发布条件 |

## 本轮记录

2026-09-14：核对原始资料和仓库入口，修订 v2，明确“一秒时钟”与“身份及时识别”的不同验收；普通静音候选与疑似换人分开；取消未经验证的固定两窗恢复结论；明确历史代码不是完成证明。

2026-09-14 10:41：用户授权按 v2 顺序执行。P0 完成现场身份采集：设备 live build_id 为 `cd945a0f528cb02d`；当前安装 Type 哈希为 `8C533...`，源码 release 哈希为 `A8FE...`，runtime gate 未通过。未安装、未刷机，下一卡为 P1。

2026-09-14 10:46：P1 建立本地 manifest、时间字段记录和事件回放判定器。五类注入故障均被捕获；四条历史会话证据边界通过回放门，但 overlap 会话仍明确含旁人尾句，故不关闭 G。P1 仍为 RUNNING，下一步先做 P2 身份能力评测。

2026-09-14：P2 从同一播放窗口提取本地声纹分数与推理耗时。`work/p2-identity-feasibility-20260914.md` 记录：推理本身较快，但本人/旁人短窗分布相交，且首个完整窗口约 1.0–1.2 s；不能靠阈值或固定“两窗”达到严格一秒。当前卡转为 P3 收尾等待，P2 保留 UNVERIFIED。

2026-09-14：P3 收尾边界核对完成。目标 speaker final wait、local shadow 超时取消、processing 可见时间和异常收口均有源码测试，但 runtime identity gate 未过，P3 保留 UNVERIFIED。

2026-09-14：P4 入口核对完成。正式录音 STOP、owner lease、preview ledger、provider/product final seal 的运行时入口已收敛；hidden candidate transport 和 stats-only 是明确隔离旁路。针对性测试 108 项通过，P4 保留 UNVERIFIED，下一卡为 P5。

2026-09-14：P5 正文与输出核对完成。语气词开关、标点保留、预览高水位、foreign veto、空/迟到 final、shadow 边界和 exactly-once 提交的生产测试通过；细粒度 ownership ledger 和真实流式光标仍未验证，下一卡为 P6。

2026-09-14：P6 唤醒与设备闭环核对完成。无口令不得正式唤醒、hidden candidate 隔离、device-key action 绑定、cancel/actor/跨会话清理和 BLE recovery 的针对性测试通过；真实声学负例和物理链路留到 P8，下一卡为 P7。

2026-09-14：P7 候选构建并安装完成。`npm run build`、品牌/云端/traceability 检查、`cargo check` 和 Windows MSI 打包均退出 0；安装后 runtime gate 退出 0，运行进程使用 exe `ACC7862E...`。P7 通过，下一卡为 P8。另记录一次用户报告的固件端“重启”：现有证据是 BLE 心跳/音频 notify 中断后恢复，诊断 GATT 导出超时，尚无 `reset_reason`，见任务工作区 `work/firmware-reboot-investigation-20260914.md`。

2026-09-14 11:23：在同一安装候选上完成一次电脑外放实机回放。自动唤醒实际接受，3008/3008 音频包到达且无丢包，自动结束 `stop_to_done_ms=1423`；未说口令的后台候选均静默 Reject。最终会话正文仍出现前段缺失和同段尾音混入，不能关闭 E/G；插入还因当前偏好 `allowNonTsfInsertionFallback=false` 记录 `windowsImeTsfRequired`，文字仅保留在剪贴板。见任务工作区 `work/p8-acoustic-evidence-20260914.md`。这次结果证明 P8 已开始，但不是通过。

## 执行记录模板

每张任务卡完成后追加一条，并更新上表；失败不能被后来的成功记录覆盖。

- 当前卡：
- 输入版本/候选哈希：
- 发现（事实）：
- 当前原因（假设）：
- 改动文件及作用：
- 验证命令与退出码：
- 原始证据、期望与实际：
- 通过/失败/未验证：
- 尚未解决的产品影响：
- 下一卡或需审核者判断的一项：

## A–G 产品结果

当前统一为 UNVERIFIED（本方案尚未执行）；不否定历史局部改善，也不把历史局部改善当作当前候选联合通过。

| 能力 | 当前候选结果 | 对应证据 |
| --- | --- | --- |
| A 正常唤醒 | UNVERIFIED | 待 P8 |
| B 无口令不正式录音输出 | UNVERIFIED | 待 P8/P9 |
| C/D 结束与正文完整交接 | UNVERIFIED | 待 P8 |
| E 不漏整句、语气词标点 | UNVERIFIED | 待 P8 |
| F 预览与结束加载 | UNVERIFIED | 待 P8 |
| G 旁人不续时、不输出 | UNVERIFIED | 待 P8/P9 |
| 日常流式插入 | UNVERIFIED | 待 P8 |
| 安装/升级/回退/低功耗 | UNVERIFIED | 待 P9 |
