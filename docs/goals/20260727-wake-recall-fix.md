# Goal：开始录音唤醒召回修复

## 现象

Owner 喊「开始录音」无胶囊。日志（15:02–15:05）显示：

- `origin=VoiceActivation` 有候选（设备 VAD 正常）
- `phrase_signal=None` / `gate_decision=Reject` / `wake_phrase_non_match`
- 本地 Paraformer 亦 `phrase_relation=Absent`
- 校准已是 3.0/0.08 仍不够

## 修复方向

1. 流式 KWS 未中时，对**整段候选 PCM 离线重跑**（重新估 gain，避免流式前 800ms 锁死增益）
2. 召回级联：更敏感多档 (score/threshold) + 短前缀变体
3. 默认落盘少量 non-match 诊断 wav 到 AppData，便于下次复盘
4. 本机 debug/安装路径自测 + 日志

## 成功标准

- 单元测试：级联/整段重跑逻辑 PASS  
- 启动后 owner 再喊「开始录音」应出胶囊，或日志有 KeywordModel/LocalTranscript Accept  
- 证据文件  
