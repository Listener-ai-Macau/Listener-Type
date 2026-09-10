<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

說話,它幫你打字。游標點進任意輸入框,按一下右 Ctrl,說,再按一下——字出現在那個框裡,不在我們的視窗裡。

Windows、macOS、Linux 都能跑,電腦麥克風拿來就能用。想要桌上有顆實體錄音鍵的話,可以配 [Listener 語音鍵盤](https://github.com/Listener-ai-Macau/Listener-Firmware)。

[English](README.md) · [简体中文](README.zh.md) · [使用說明](docs/USAGE.md) · [版本說明](docs/release/1.0.5.md)

## 怎麼用

Windows 上從 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下載 MSI,安裝,允許麥克風。設定就這麼多。

之後永遠是同一個動作:點進輸入框,按右 Ctrl,說話,再按一次。碰到不讓插入文字的軟體,結果會先放在剪貼簿裡。macOS 和 Linux 的熱鍵換成右 Option / 右 Alt,細節看[使用說明](docs/USAGE.md)。

## 能做什麼

- 寫進目前焦點的軟體:記事本、瀏覽器、聊天、程式碼編輯器都行。
- 把說過的話收拾好:原文照錄、輕度整理、列成條目、改成正式郵件,按 Shift 還能翻譯。風格就是本機檔案,隨便複製隨便改。
- 記住你的用詞:人名、行話寫進本機詞庫,辨識和整理都會參考。
- 動口開工:說一句「開始錄音」就開始。錄入三遍聲紋之後,別人說話——或者放你的錄音——都不會觸發。
- 跑在你自己的帳號上。辨識可以走火山引擎、OpenAI 相容介面、Apple Speech,也可以完全離線;整理可以接 Ark、DeepSeek、Anthropic 相容介面。Key 存在系統憑證裡,軟體不自帶任何 Key。
- 東西都留在本機:歷史、風格、詞庫、設定,不出這台電腦。

完整清單和平台差異在 [docs/product/features.md](docs/product/features.md)。

## 關於鍵盤

[Listener 語音鍵盤](https://github.com/Listener-ai-Macau/Listener-Firmware)是個 USB-C 小裝置:一顆能按的旋鈕管開始停止,四顆鍵隨便綁,幾盞燈分別管電源、藍牙、錄音、處理。韌體同樣開源。不是必需品——用麥克風就挺好——但一顆真按鍵總比摸快捷鍵強。

## 還做不到的

- 鍵盤的藍牙音訊目前只有 Windows;macOS 和 Linux 先用電腦麥克風。
- 聲紋是防誤觸發的,不是安全功能。
- 離麥克風太遠、或者兩個人同時說話,還是處理不好。排在 1.0.6。
- 不是系統輸入法,是把字打進目前焦點框。

## 自己建構

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面建構還需要 `third_party/denzic-platform` 子模組。提交程式碼前看 [CONTRIBUTING.md](CONTRIBUTING.md),回報漏洞看 [SECURITY.md](SECURITY.md)。

程式碼是開源的;Listener Type 的名字、圖示和吉祥物不是——改了名的分支請別再用它們。
