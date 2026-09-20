# 连续监听固件有线安装与实测

2026-09-13：用户明确指定有线刷机后，执行 Firmware 仓库 tools/flash.ps1 -Port COM5 -NoBuild -PreserveOtaData 成功。此前蓝牙组合命令被执行接口拒绝，不代表有线升级被禁止；无需提交官方反馈作为 Listener 后续开发的前提。本轮未再次执行被拒绝的蓝牙组合命令。

## 身份

- Type 1.0.5：73D0869DC317CD9648D8C22326D12FC3EEEC095F7E3C2EB5E048DD2EAC321DF9，正常运行核对 PASS。
- Firmware 1.0.5：cefb802eed6e36d1；bin SHA-256 5920782cf962b4d40e73cb02120583d27e95aa668d2884a6ca409e7c60521b86。
- 当前源码通过 tools/build.ps1 再构建，二进制与既有连续监听 OTA 包一致。刷写校验通过，串口确认 running=ota_0、boot=ota_0、pending_verify=0、build_id=cefb802eed6e36d1，与产物 ELF SHA 前缀一致。保留 OTA 数据模式不写 NVS，未执行整片擦除。
- 匹配 OTA 包仍为 Firmware .cache/repair-20260913-drain-monitor/listener-ota-1.0.5-20260913-134543.zip；未发布到稳定渠道。

## 六轮实机（任务 work/wired-drain-*）

| 场景 | 最终输出与结论 |
| --- | --- |
| clean-1 正常长句 | 80 字完整，末尾「可追溯。原因」标点不理想；正文比较通过不代表标点通过 |
| pause-1 约 1.5 秒句中停顿 | 80 字完整，未提前截断 |
| overlap-1 原无效语音后唤醒失败素材 | 这次开录并自动结束，尾包期间未见 monitoring=0 或 DMA 溢出；仍漏正文开头「今天」，严格正文失败 |
| with-wake-1 本人带口令短句 | 开录后最终仅「1」，失败；归档 WAV 的独立本地识别能恢复本人原句，云诊断仅返回「1」及空文本，需进一步区分送云 PCM、会话交接与服务识别 |
| without-wake-1 同一本人无口令录音 | 约 46 秒观察无正式开录、无最终输出；声纹不匹配，不直接覆盖主人匹配但无口令的分支 |
| manual-owner-1 手动开始后播放同一本人正文 | 本人原句完整；由应用命令手动开始，不代表物理按键已实测 |

五个正例都开录并完成，忽略标点的严格正文比较 3 通过、2 失败；无口令单独核对，不把无输出当抗干扰通过。完成等待约 2.71–4.69 秒（末次后端字符数增长至完成），不是最终文本变化或正文起声的精确时钟。

原分段素材的唤醒通路恢复且监听未关闭，支持连续监听改动有作用；还没有三种相对唤醒位置和多轮联合通过证据，不能称 A/E 根治。G、语气词标点供应、首预览延迟、真实双人、拔 USB 和光标上屏等原未通过项继续保留。

## 后续

先对齐两个失败的实际收音、归档、送云音频和原始识别响应；不把缺字补成固定答案。自动/手动短句对照只能帮助缩小范围，一轮结果不能证明根因。再补不同分段位置和保持正常场景的联合回放。

蓝牙代码路径已核对：产品 FirmwareOtaPanel 使用 transferFirmwareOtaBle，底层为 transfer_firmware_ota_ble；既有 OTA 脚本也是调用该生产命令。本轮没有当前候选经蓝牙传输的成功证据，不能把此前执行接口拒绝称为产品 OTA 功能故障。

结束时 streamingInsert=true、allowNonTsfInsertionFallback=true、removeFillerWords=true、recordAudioForDebug=false，9222 关闭。安装证据：任务 outputs/wired-drain-install.json；正文判定 outputs/wired-drain-acceptance.json；负例 outputs/wired-drain-negative-evidence.json。所有失败保留，未放行发布。
