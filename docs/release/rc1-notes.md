# Type build208 / Firmware38 · Listener v1.0.6-rc1

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

- Computer-microphone questions now show a live question preview and mark audio ready after buffered audio is delivered. This removes a false finalization timeout/replay on an otherwise healthy connection; retained-audio recovery still handles actual transport failure.
- History keeps Refresh in a stable position as records appear. Clear requires an explicit in-app confirmation with Cancel initially focused.
- Save a completed question answer directly as a local text draft. The saved original and current text contain that answer; private selected context is not added to the draft. Find it later under History → Saved drafts and continue with the existing text tools.
- The question window may save only a completed answer in its current native conversation. A frontend-only forged answer is rejected. Normal recording-history preferences and the other draft permissions stay in force.
- Retains build205 automatic local ASR language detection and Firmware38. No firmware flash is required for these app changes. The synthetic-input adapter used in private engineering/customer sessions is excluded from this MSI.

Engineering confirmation used synthetic speech through ordinary controls and existing configured ASR/text services. It does not establish live-microphone, physical Listener, arbitrary external-input or long-term qualification. Independent AI experiences and real orders remain separate. Review generated answers and translations before using them.

An independent assumed-owned AI user actually saved and retrieved two self-edited bilingual drafts and a spoken-question answer, including renaming the answer draft. This was software use with synthetic speech and preconfigured services, without an order or physical receipt. Formal transformation can retain author-facing writing instructions as text; local translation and further polishing can retain incorrect or unnatural terms. Those limitations remain open, and the user's manually edited drafts are not proof of automatic writing quality.

US$49 is the hardware price. Type is free/open source; service preparation and usage charges are separate. No ready-to-use official cloud account or allowance is included. RC1 remains a prerelease. Strong background-speech isolation, remaining physical/global scopes and the original brownout trigger remain unresolved.

## File identity

- Type source: `1d4e7978f52fd5fd0c1f4b0d57437cc91833d3c1` · `build-source/type208-20261027`
- MSI: 299790336 bytes · SHA256 `3d4dc7ab0d337a0d7be7946ea6791c70674773c249830eea339f7e1eb25d1d56`
- Native EXE SHA256: `5ce4eb64a20bbe35f939cd6664659e481a84dda9df11819f6cbca188345f0786`
- Native DLL SHA256: `c747a6d40dc762aae678e23a87b83ed565b8e374916f1c50d605eb2403aa731b`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

电脑麦克风提问会显示实时问题预览，并在前缀音频送达后正确标记就绪，避免健康连接在结束时误等待并重传。历史刷新按钮位置固定，清空须应用内明确确认且默认取消。完成的回答可直接保存为本机成稿，从「历史 → 已保存成稿」回找；原文和当前正文均为该回答，不额外带入私密选区。仅允许保存当前原生会话中已完成的回答，前端伪造内容会被拒绝。保留205本地识别修复和原固件38，无需重刷；普通MSI不包含隔离合成输入适配器。合成语音和AI体验不能代替真人收音、外部输入、实体长期或成交；答案与译义仍需核对，原22类剩余验收和欠压触发源继续保留。
