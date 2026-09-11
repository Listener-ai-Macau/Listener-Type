<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type 是個桌面聽寫軟體。把游標點進一個輸入框,按一下右 Ctrl,說話,再按一下,轉寫出來的文字就打進了那個框。聊天視窗、瀏覽器、編輯器、郵件,凡是能打字的地方都行。目標軟體不讓程式輸入時,文字會留在剪貼簿裡。

軟體免費、開源,電腦上現有的麥克風就能用。配套的硬體是 [Listener 語音鍵盤](https://github.com/Listener-ai-Macau/Listener-Firmware):一個桌面小裝置,韌體在隔壁倉庫,但沒有鍵盤也能正常用。

[English](README.md) · [简体中文](README.zh.md) · [使用說明](docs/USAGE.md) · [1.0.5 版本說明](docs/release/1.0.5.md)

### 安裝

Windows 是主平台:從 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 下載 MSI 裝上即可。安裝包依賴 WebView2,連網時會自動補齊。

| | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 麥克風聽寫 | ✓ | ✓(12+) | ✓ |
| 全域熱鍵 | ✓ | ✓ | X11 盡力而為;Wayland 要在桌面環境裡綁定 |
| 鍵盤藍牙音訊 | ✓ | 還沒有 | 還沒有 |

### 用法

錄音是切換式的,不用按住。按一下右 Ctrl(macOS 是右 Option)開始,說完再按一下。工作期間螢幕上有顆小膠囊,顯示進行到哪一步:錄音、轉寫、處理、完成。中途反悔按 `Esc` 取消,不會插入半截文字。

輸出成什麼樣,看你選了哪種風格。Raw 盡量保留原話;Light 去口癖、補標點;Structured 和 Formal 分別面向筆記和郵件;設了目標語言之後,Shift 把下一段錄音標記成翻譯。風格就是本機檔案,內建的幾個可以複製出來隨便改,也能匯出成 ZIP 分享。

另外幾個內建熱鍵:`Ctrl+Shift+;` 對選中的文字提問,`Ctrl+Shift+S` 給上一段結果換一種風格,`Ctrl+Shift+O` 喚起應用。macOS 上把 `Ctrl` 換成 `Cmd`。

### 工作原理

音訊從麥克風(或藍牙連著的鍵盤)進到辨識,轉寫結果可選地過一遍風格,然後經由系統的原生輸入路徑打進焦點框。

服務商沒有鎖定。辨識可以走火山引擎的流式介面、任意 OpenAI 相容端點、Apple Speech,或者完全在本機跑;風格可以接 Ark、DeepSeek,或任意 Anthropic / OpenAI 相容端點。API Key 存在系統憑證庫,軟體不帶任何 Key;歷史、詞庫、風格、設定都不出這台電腦,整條流程不依賴 Listener 的伺服器。

本機詞庫(人名、產品名、縮寫)會同時餵給辨識和風格兩步。同事的名字就是這麼不再被寫錯的。

### 鍵盤

Listener 鍵盤是個 USB-C 小裝置:一顆能按的旋鈕(開始/停止,雙擊重新配對,轉動調音量)、四顆可以在軟體裡綁動作的鍵、六盞分別表示電源、藍牙、錄音和處理狀態的燈。

配上鍵盤還能動口開工:打開「檢測到人聲後自動開始」,裝置會先等喚醒詞(預設「開始錄音」)。再照引導錄三遍自己的聲音,旁人播放的語音就不會混進正文。這是防誤錄的輸入保護,不是身分認證,別拿它當鎖用。

沒有鍵盤軟體一樣好用,它只是把錄音鍵放到你手指底下。韌體開源:[Listener-Firmware](https://github.com/Listener-ai-Macau/Listener-Firmware)。

### 它不做什麼

既然發的是 1.0.5,還是說清楚:

- 鍵盤的藍牙音訊目前只有 Windows。
- 遠距離拾音、兩個人同時說話還不行,那是 1.0.6 的活。
- 它不是系統輸入法,也沒打算做——它把字打進目前焦點框。

完整功能清單(包括每條做不到什麼)在 [docs/product/features.md](docs/product/features.md)。

### 從原始碼建構

應用是 Tauri v2:React + TypeScript 前端,Rust 後端,外加一個負責 Windows 插入的原生文字服務。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整的桌面建構還需要 `third_party/denzic-platform` 子模組,其餘見 [CONTRIBUTING.md](CONTRIBUTING.md);安全問題發 [SECURITY.md](SECURITY.md)。原始碼開放,但 Listener Type 的名字、圖示和吉祥物不授權給改名後的分支。
