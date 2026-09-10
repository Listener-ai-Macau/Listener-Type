<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="128" />
</p>

<h1 align="center">Listener Type</h1>

<p align="center">
  <strong>開口說話,字就到游標。</strong><br/>
  開源的桌面語音輸入——在你正在用的任何軟體裡。
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <strong>繁體中文</strong>
</p>

<p align="center">
  <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases"><img src="https://img.shields.io/github/v/release/Listener-ai-Macau/Listener-Type" alt="Release" /></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-blue" alt="Platform" />
</p>

<p align="center">
  <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases">下載</a> ·
  <a href="docs/USAGE.md">使用說明</a> ·
  <a href="docs/product/features.md">產品功能</a> ·
  <a href="docs/release/1.0.5.md">版本說明</a> ·
  <a href="https://github.com/Listener-ai-Macau/Listener-Firmware">鍵盤韌體</a>
</p>

<!-- 頭圖:有產品截圖或演示動圖後,放在這裡。 -->

Listener Type 是一款本機優先的聽寫軟體:按一下,說話,再按一下——字已經打在你剛才的輸入框裡。它不是錄音筆,也不是聊天視窗:字去的是游標所在的地方。

它免費、開源(Tauri + React + Rust)。有電腦麥克風就能開始用;配上 [Listener 語音鍵盤](https://github.com/Listener-ai-Macau/Listener-Firmware)——一個帶旋鈕、實體鍵和狀態燈的桌面小裝置——開始、停止、「它到底聽到沒有」,都在手上完成,眼睛不用離開螢幕。

## 30 秒上手

**Windows:** 從 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下載 MSI,安裝,允許使用麥克風。

```text
游標點進任意輸入框
  → 按右 Ctrl(有鍵盤就按一下旋鈕)
  → 說話
  → 再按一次
  → 字已經在游標處
```

沒有鍵盤:設定 → 錄音 → 輸入來源選「麥克風」。
有鍵盤:在藍牙裡配對名為 `listener` 的裝置——目前完整的鍵盤音訊鏈路以 Windows 為主,macOS 和 Linux 先用電腦麥克風。

詳細步驟:[使用說明](docs/USAGE.md) · [語音鍵盤手冊](docs/quickstart/voice-keyboard-readme.md)

## 用起來不一樣的地方

| | |
| --- | --- |
| **在哪裡都能寫** | 記事本、瀏覽器、聊天、程式碼編輯器——字直接落進目標軟體;個別軟體不讓插,就先放進剪貼簿。按一下開始、再按一下結束,不用一直按住。 |
| **手上有台鍵盤** | 單擊旋鈕開始/停止,雙擊重新配對,四顆鍵隨你綁定。PWR / BLE / REC / AI 幾盞燈,進行到哪一步一眼看清。 |
| **按場合換語氣** | 原文照錄、輕度整理、結構化紀要、正式郵件——按 Shift 還能直接翻譯。語氣風格就是本機檔案,隨改隨匯出,不用登入任何商店。 |
| **你的詞它認識** | 人名、產品名、行話寫進本機詞庫,辨識和整理都會參考。 |
| **動口就開工** | 說一句「開始錄音」就開始。可選錄三遍聲紋:別人在你旁邊播放的聲音,進不了正文。 |
| **服務你選,鑰匙你拿** | 辨識可接火山引擎、OpenAI 相容介面、Apple Speech,也能完全在本機跑;潤飾可接 Ark、DeepSeek、Anthropic 相容介面。Key 存在系統憑證裡,軟體不內建任何 Key。 |
| **本機優先** | 歷史、風格、詞庫、設定都在你自己電腦上;沒有任何 Listener 伺服器也照常用。 |

功能全表、燈的含義、平台差異,見[產品功能](docs/product/features.md)。

## 1.0.5 不承諾什麼

- 鍵盤的藍牙音訊鏈路目前以 Windows 為主,macOS 和 Linux 先用電腦麥克風。
- 聲紋是防止誤錄旁人說話的輸入保護,不是身分認證,別當門鎖用。
- 遠距離聲紋、多人同時說話的隔離是 1.0.6 的事,這一版沒有。
- 它不是系統輸入法,是把文字插入目前焦點的輸入框。

## 從原始碼建構

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面建構還需要 `third_party/denzic-platform` 子模組。提交程式碼前請讀 [CONTRIBUTING.md](CONTRIBUTING.md);安全問題請走 [SECURITY.md](SECURITY.md)。

原始碼可以自由閱讀、建構、修改;但 Listener Type 的名稱、圖示和吉祥物,不隨原始碼授權給改名後的發行版。
