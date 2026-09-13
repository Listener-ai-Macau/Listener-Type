<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Listener Type" width="96" />
</p>

# Listener Type

Listener Type 是一個桌面語音輸入工具。把游標點到想寫字的地方，開始錄音，然後正常說話。Listener 會把語音變成文字，再放回剛才使用的應用程式。

只用電腦內建的麥克風就能工作。配上 Listener 語音鍵盤後，同一套輸入流程也能用旋鈕和按鍵控制，並在桌面上看到藍牙連線、錄音和處理狀態。

[English](README.md) · [简体中文](README.zh.md) · [下載](https://github.com/Listener-ai-Macau/Listener-Type/releases) · [使用說明](docs/USAGE.md) · [完整功能](docs/product/features.md)

<p align="center">
  <img src="docs/assets/readme/overview.png" alt="Listener Type 主介面" width="900" />
</p>

## 可以做什麼

- **在目前應用程式裡直接聽寫。** 用全域快速鍵開始和停止，也可以說完後自動結束。小膠囊會顯示即時結果，按 `Esc` 可以取消。
- **決定文字怎麼寫。** 保留辨識原文，輕度清理語氣詞並補標點，整理成結構化筆記，改成正式表達，或者翻譯成另一種語言。
- **讓 Listener 認識你的詞。** 人名、產品名、縮寫、熱詞和糾錯規則會跟隨本機設定保存。
- **選擇雲端或本機辨識。** 目前支援火山引擎、OpenAI 相容批次 ASR、Apple Speech、百鍊即時、macOS Qwen 本機辨識和 Windows Foundry Local Whisper。
- **聽寫完繼續處理。** 可以為上一條結果換風格、詢問選取的文字、查看本機歷史，也可以為常用動作設定快速鍵。

如果目前應用程式不接受直接插入，Listener 會把結果留在剪貼簿並提示貼上，不會讓整段文字消失。

## 開始使用

1. Windows 從 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases) 安裝最新 MSI。macOS 和 Linux 可以從原始碼執行。
2. 允許麥克風權限，在設定 → 錄音中選擇「電腦麥克風」。
3. 把游標點進輸入框。Windows 按右 Ctrl，macOS 按右 Option，說完後再按一次。

[使用說明](docs/USAGE.md)介紹了 Provider、寫作風格、詞庫、快速鍵、歷史和常見問題。

## Listener 語音鍵盤

在設定 → 裝置中配對鍵盤。單擊旋鈕開始或停止聽寫，旋轉調節音量或亮度，KEY1–KEY4 可以分配常用動作。PWR、BLE、REC、AI、OK、WARN 會顯示鍵盤和桌面應用程式目前在做什麼。

裝置頁面還可以查看電量，調整燈光和休眠時間，恢復配對，以及更新韌體。正常 OTA 會保留藍牙配對和裝置設定。

應用程式可以開啟語音喚醒，也可以錄三段聲紋作為額外的輸入保護。聲紋不是身分認證。嘈雜環境、遠距離和多人聲音重疊仍在繼續調校，這些情況下需要快速確認一下最終文字。

設定和復原方法見[語音鍵盤手冊](docs/quickstart/voice-keyboard-readme.md)。

<p align="center">
  <img src="docs/assets/readme/recording-settings.png" alt="Listener Type 錄音設定" width="720" />
</p>

## 你的資料

設定、歷史、詞庫、風格和糾錯規則保存在電腦上。Provider 憑證進入作業系統憑證庫，Listener 不內建任何服務商 Key。診斷錄音是可選功能，預設關閉。

日常聽寫不依賴 Listener 自營帳號服務。選擇雲端 Provider 時，辨識音訊只會傳送給你選擇的服務；選擇本機引擎時，辨識留在電腦上完成。

## 平台說明

Windows 是主要發布平台，同時支援電腦麥克風和 Listener 鍵盤。macOS 12+ 支援電腦麥克風、Apple 與本機辨識、全域快速鍵和輔助使用插入。Linux 目前面向開發者；麥克風聽寫可以使用，快速鍵行為取決於 X11 或 Wayland 桌面環境。

Windows 標準安裝包會把文字寫入目前焦點框，不會把 Listener 安裝成系統輸入法。單獨的 TSF 路徑仍用於工程驗證。

## 這個倉庫

Listener 分成兩個倉庫維護：

- **Listener Type** 負責錄音工作階段、辨識、文字處理、插入、歷史、設定和桌面端裝置體驗。
- [**Listener Firmware**](https://github.com/Listener-ai-Macau/Listener-Firmware) 負責麥克風採集、BLE 音訊與 HID、實體控制、燈、電池與電源、診斷和裝置 OTA。

桌面應用程式使用 Tauri 2、Rust、React、TypeScript 和 Vite。

```bash
npm ci
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

完整建置還需要固定版本的 `third_party/denzic-platform` 子模組。開發流程見 [CONTRIBUTING.md](CONTRIBUTING.md)，問題回報見 [SUPPORT.md](SUPPORT.md)，安全問題請按 [SECURITY.md](SECURITY.md) 私下提交。

版本說明和校驗值保存在 [Releases](https://github.com/Listener-ai-Macau/Listener-Type/releases)。目前倉庫還沒有 `LICENSE`，因此能看到原始碼不代表已經取得再散布或修改授權。
