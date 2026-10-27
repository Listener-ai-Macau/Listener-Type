# Type build211 / Firmware38 · Listener v1.0.6-rc1

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## Saved drafts without piling up windows

Viewing a saved result switches from the workspace to the draft library. **Continue in text workspace…** reuses the workspace; repeated save/view/continue cycles no longer create a growing stack of dialogs. One dialog stays active.

Closing the library returns to the same edited result, name and scroll position. Starting from History and returning keeps the selected draft. Deleting a viewed draft invalidates its saved receipt so the retained text can be saved again. Unsaved rename and copy protections remain. Navigation does not submit text or change active dictation.

The old React path grew from one to seven open dialogs in three cycles. Six affected React interaction checks now pass, including the short viewport, History and style-page entries. The matching211 ordinary native UI also saved, viewed, continued, deleted, resaved and returned to exact local records without text-service requests. These are bounded engineering checks; no new211 independent customer or physical acceptance is claimed.

Retains210 direct instruction-to-message drafting, explicit original-source cloud retranslation with the edited local comparison, and language/route variant names. Retains208 question preview/readiness, protected History clearing and explicit completed-answer draft saving. Firmware38 is unchanged; no firmware flash is required.

## Existing limits

The offline translation model can change important actions or dates. This release does not fix model accuracy; local and cloud output still need review. Strong background-speech isolation, some early-ending conditions, real global/physical/cross-PC/long-term qualifications and the original brownout trigger remain open. RC1 remains a prerelease.

US$49 is the hardware price. Type is free/open source. Service preparation and usage charges are separate; no official cloud account or allowance is included.

## File identity

- Type source: `5cbb60298c14a40e716f4a1c090248237a18d6e4` · `build-source/type211-20261027`
- MSI: 299786240 bytes · SHA256 `b9cffc8cd85c66e0fe9347d9b4b6b88348ce19df7665fb9bc56d61bf20076f6f`
- Native EXE SHA256: `05b86d99ef0464cd6c9d88d14a16ed01a910b4e382edb21c56cdbfe43ce03f5f`
- Native DLL SHA256: `f34988cdae0bd10e48a8ed9521427e6c02faab63a94d51e0bdd7a2bd8606b42c`
- Firmware38 source: `bc1a1241e44ba79a424263bb89acdb8d72dec36a` · runtime `094f5a60049f59f4`
- ZIP: 1957429 bytes · SHA256 `af02bb866c7099bf24cebb0eed6cde35318743631cb720d65da8a4ee9f53b6e7`

## 中文说明

修复「保存 → 查看成稿 → 继续处理」反复叠加窗口的问题，改为复用工作区并保持一个活动窗口。返回保留手改正文、名称、滚动位置和所选成稿；删除后可重新保存保留的正文，未保存改名与副本保护继续有效。原前端三轮从1层累积到7层，修复后的六类交互检查及精确211原生本机保存／回找／删除重存／历史返回通过。导航不发起文本处理请求，不改变日常听写。固件38和既有语音处理保持；本机翻译准确性、强背景干扰、真实全局与长期范围、欠压触发源仍未完成。
