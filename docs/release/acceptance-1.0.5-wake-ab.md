# Listener Type 1.0.5 唤醒 A/B 验收指引

这份验收只针对唤醒链路。当前桌面快捷方式已经指向安装版 `1.0.5`。

## 先确认版本

在 PowerShell 中运行：

```powershell
$p = 'C:\Program Files\Listener Type\listener-type.exe'
(Get-Item $p).VersionInfo.ProductVersion
```

输出必须是 `1.0.5`。如果不是，停止验收，不要继续记录数据。

## A/B 定义

- **A 组（当前基线）**：1.0.5 当前使用的 Sherpa Zipformer `zh-en-3M` FP32 KWS 图，保持现有融合、声纹和阈值不变。
- **B 组（候选）**：同一模型的 INT8 执行图；只有模型精度/执行图改变，其他代码、固件、麦克风位置、音量、说话距离和环境保持不变。

B 组没有单独构建并安装前，不要把 A/B 结果混写。每组使用独立日志和结果文件。

## 每组 20 轮

建议顺序：先完成 A 组 20 轮，再完成 B 组 20 轮。每轮之间等待胶囊回到空闲，避免上一轮影响下一轮。

可用现成的轮次引导窗口记录唤醒标记（它只保存时间和 session ID，不保存音频）：

```powershell
New-Item -ItemType Directory -Force .artifacts\acceptance-1.0.5 | Out-Null
pwsh -NoProfile -File .\scripts\show-live-wake-acceptance.ps1 `
  -LogPath (Join-Path $env:LOCALAPPDATA 'Listener Type\Logs\listener-type.log') `
  -OutputPath '.artifacts\acceptance-1.0.5\wake-A-markers.json' `
  -Attempts 20
```

完成 A 组后，把输出文件名中的 `A` 改为 `B` 再运行一次。每组的人工结果仍要按下方模板补齐；窗口标记不能代替“是否吞字/是否自动结束”的人工判断。

1. 点击“开始第 N 轮”。
2. 约 0.3 秒后自然说一次：**开始录音**。
3. 说完后等待胶囊自动结束；不要手动按键补救。
4. 记录下面 5 个字段：
   - 是否唤醒：是/否
   - 从说完唤醒词到录音胶囊出现的大致延迟：毫秒
   - 首句是否吞字：是/否
   - 是否误把旁人/噪声当成主人：是/否
   - 是否自动结束：是/否；若否，记录最后看到的状态

## 四种固定场景

每组 20 轮按下面比例执行，不能只测试安静环境：

| 场景 | 轮数 | 操作 |
| --- | ---: | --- |
| 安静直说 | 5 | 安静环境，自然说“开始录音” |
| 轻微变体 | 5 | 稍快、稍慢、轻声、带前后空白各 1 轮 |
| 中文干扰 | 5 | 播放旁人中文或让旁人说话，再说“开始录音” |
| 非唤醒噪声 | 5 | 说相近但不是唤醒词的短句，或播放环境噪声 |

## 判定规则

按优先级比较，不用平均分掩盖漏唤醒：

1. **硬失败**：任一组出现连续 3 轮唤醒失败；或录音出现无法自动结束；或出现新的 reset/WDT。
2. **第一优先级**：唤醒召回率（成功轮数 / 20）。召回率更高者优先。
3. **第二优先级**：在召回率相同或更高时，比较唤醒延迟的 p95。
4. **第三优先级**：比较误唤醒率。
5. **不可接受**：吞字、主人句尾丢失、干扰导致永久录音不结束。

用户体验取舍已经确定：宁可少量误唤醒，也不能让用户反复唤醒仍无响应。

## 结果记录模板

复制下面模板各一份，保存为 `wake-ab-A.md` 和 `wake-ab-B.md`：

```text
版本/变体：
EXE SHA256：
模型图：
固件版本：1.0.5
测试日期：

安静直说：成功 __/5，吞字 __，误唤醒 __，自动结束 __/5
轻微变体：成功 __/5，吞字 __，误唤醒 __，自动结束 __/5
中文干扰：成功 __/5，吞字 __，误唤醒 __，自动结束 __/5
非唤醒噪声：误唤醒 __/5，自动结束 __/5

唤醒延迟 ms（20 轮）：
平均：
p95：
最大：

新 reset/WDT：无 / 有（附日志行）
最长未结束会话：
hold reason：
结论：通过 / 失败
备注：
```

## 日志中重点看什么

在 `%LOCALAPPDATA%\Listener Type\Logs\listener-type.log` 中检查：

- 唤醒延迟：`wake_to_capsule_request_ms`、`phrase_tail_to_capsule_ms`
- 自动结束：`target-speaker auto-stop sent`、`stop_to_transcribing_ms`
- hold 原因：`target endpoint hold ... reason=`
- 重启：`reset_reason`、`task_wdt`、`interrupt_wdt`、`Guru Meditation`
- 音频异常：`I2S stall`、`ringbuffer full`、`queue_full`

## 结论门槛

- 如果 B 组召回率下降，即使更快，也淘汰 B。
- 如果 A/B 召回率接近，选择 p95 延迟更低且不吞字的一组。
- 如果两组都出现漏唤醒或干扰不结束，停止调阈值，进入“更换 KWS 模型/重新做前端特征”的专项，不把问题归咎于用户发音。
