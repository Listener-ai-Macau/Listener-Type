# Listener v1.0.6-rc1 · Type build163 / Firmware37

## Download

- [Windows MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)
- [ESP32-S3 firmware ZIP · USB or Bluetooth OTA](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)

## Correction from build162

A real first-use customer encountered a short paragraph that returned only its last sentence and a question that dropped the final thanks. Both original inputs and the refusal are retained. The worker now gives every sentence its own model request, regardless of paragraph length, and uses bounded beam5 decoding to retain the previous no-send/save-draft case. No models, provider keys or ordinary recording/Bluetooth paths change. Nine actual-worker content-preservation cases and three matching native ordinary-UI examples passed. Wording can remain awkward; this is not professional or universal translation quality.

## Local translation

Tools → Translation, or Try typed text with a chosen output language, offers optional local Chinese↔English translation on Windows x64. Choose the original language and explicitly download the components once: about 113 MB for the first Chinese→English setup (runtime included), plus about 71 MB for English→Chinese. Both installed directions plus runtime use about 258 MB. Models stay outside this 299.4 MB MSI; users need no separate Python installation.

Local translations run on CPU, require no text-provider key and make no per-translation API charge. Original and output are editable; copy/save/insert remain explicit. Opening a page or selecting a language does not download components. No automatic fallback to paid cloud. Existing active cloud choices are preserved; unknown new settings cannot reset provider credentials. Normal dictation remains on its original path. Explicit local voice translation uses the same engine after recognition; cloud ASR setup and usage charges remain separate.

## Checked scope and limits

Build162 verified the ordinary component downloads with hashes, editable translations, explicit Copy/private Paste and settings save/readback with empty cloud keys. Those unchanged paths retain that precise evidence. Matching build163 native UI specifically restores both original omitted-content inputs and retains the prior no-send/save-draft meaning in Traditional Chinese. Saved-worker cases use the new exact worker. Installed163 preferences/history/styles/vault are preserved and normal BLE audio readiness returned. Prior controlled cancellation/readiness/post-ASR routing evidence retains its original source scope.

These are scoped software checks, not physical voice/Translation-key, arbitrary editor, global language quality, complete encrypted-RF or hardware purchase qualification. Review translations, especially amounts, dates and negations. Other local language pairs are not qualified yet. Existing full pre-sale gaps and strong background-speech separation remain open. Firmware37 is unchanged.

## Price and services

US$49 is the hardware price. Type is free and open source. This release does not provide a ready-to-use official account or allowance. Local text translation is optional and has no translation API fee; configured cloud recognition and optional cloud text services have their own preparation and charges.

## Exact artifacts

- Type source: `efe9c7f17c940dc3217449e7834af8bf5909077e` · `build-source/type163-20261024`
- MSI: 299364352 bytes · SHA256 `49c06d90e77fc6161a428ed61f26d39bc6ce8ab6cd566e2c148ff6214c299950`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

本版提供可选本地中英互译：首次明确下载组件，之后 CPU 离线运行，无需文本服务密钥、不收逐次翻译 API 费用。原文/译文可编辑；不会自动退回收费云。语音识别是独立步骤，原云识别配置与费用仍保留。当前只确认 Windows x64 中英两个方向，不保证所有语句或全球语言。匹配包内下载、翻译、修改复制粘贴和设置读回已验证；真人语音/硬件翻译键、任意输入框与整机售前范围仍分别待验。固件37未变。
