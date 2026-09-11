<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

說話,文字出現在目前游標。Listener Type 是個桌面聽寫軟體:游標點進任何輸入框,按一下右 Ctrl,說,再按一下,字就打進去了。開源,Windows / macOS / Linux 都能裝,電腦麥克風就夠用。

[English](README.md) · [简体中文](README.zh.md) · [使用說明](docs/USAGE.md) · [1.0.5 版本說明](docs/release/1.0.5.md)

## 先花 30 秒試一下

1. Windows 上從 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下載 MSI 安裝(要 WebView2,連網會自動補)。
2. 打開,允許麥克風。
3. 點進記事本,按右 Ctrl,說一句話,再按一次。

字應該已經在記事本裡了。中途按 `Esc` 取消;某個軟體不讓自動輸入時,文字會進剪貼簿,提示你自己貼上。macOS 上熱鍵是右 Option。到這步能用了,再往下看。

macOS 12+ 和 Linux 也能裝,先用電腦麥克風;鍵盤的藍牙音訊只在 Windows 上驗收過。Linux 的全域熱鍵在 X11 下盡力而為,Wayland 要在桌面環境裡綁定命令,細節在[使用說明](docs/USAGE.md)。

## 一天裡怎麼用

**回訊息。** 預設的 Light 風格:去掉「那個」「嗯」,補上標點,別的不動。說的時候是口水話,發出去是正常人話。

**寫郵件和正式溝通。** 切到 Formal,語氣收拾整齊,但不替你編內容。對方讀另一種語言時,設好目標語言,按 Shift 再說,出來就是譯文。

**記需求、任務、prompt。** Structured 按主題和目標整理成條目。寫程式碼註解、或者要留原話的時候用 Raw,它盡量不動你的詞。

**它認識你的詞。** 人名、產品名、縮寫加進本機詞庫,辨識和整理都會參考——同事的名字就是這麼不再被寫錯的。

順手再記三個熱鍵:`Ctrl+Shift+;` 對選中的文字提問,`Ctrl+Shift+S` 給上一段結果換個風格,`Ctrl+Shift+O` 喚起應用(macOS 把 `Ctrl` 換成 `Cmd`)。

## 桌上的鍵盤

[Listener 語音鍵盤](https://github.com/Listener-ai-Macau/Listener-Firmware) 是配套硬體:一顆能按的旋鈕管開始和停止,雙擊重新配對,轉一下調音量;四顆鍵在軟體裡隨便綁動作;六盞燈分別說電源、藍牙、錄音、處理到哪一步。

不想碰鍵盤也行:打開「檢測到人聲後自動開始」,說聲「開始錄音」就開工。照引導錄三遍聲紋之後,旁邊放別人的語音不會混進正文。聲紋是防誤錄的,不是身分認證,別當鎖用。

沒有鍵盤軟體照常用,鍵盤只是把錄音鍵放到手指底下。韌體也開源:[Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware)。

## 你的資料和你的 Key

辨識可以走火山引擎流式介面、任意 OpenAI 相容端點、Apple Speech,或完全在本機;潤飾接 Ark、DeepSeek,或任意 Anthropic / OpenAI 相容端點。Key 存在系統憑證庫,軟體不自帶任何 Key。歷史、詞庫、風格、設定都在本機——整條鏈路不依賴 Listener 的伺服器。

## 目前的邊界

發的是 1.0.5。遠距離拾音、多人同時說話還不可靠,排在 1.0.6。它不是系統輸入法,文字進目前焦點框。完整功能清單(包括每條做不到什麼)在 [docs/product/features.md](docs/product/features.md)。

## 自己動手

Tauri v2 + React + Rust;Windows 的插入走 `windows-ime/` 裡的 TSF 文字服務,macOS 走輔助使用介面。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面建構還需要 `third_party/denzic-platform` 子模組。貢獻見 [CONTRIBUTING.md](CONTRIBUTING.md),安全問題發 [SECURITY.md](SECURITY.md)。原始碼開放;Listener Type 的名字、圖示和吉祥物不授權給改名後的分支。
