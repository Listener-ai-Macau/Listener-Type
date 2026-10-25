# QA And Selection Ask

## Status

- status: `implemented`
- scope: a question window with visible open, record and finish controls; optional keyboard shortcuts

## Source Map

- Coordinator: `src-tauri/src/coordinator/qa.rs`, `src-tauri/src/coordinator.rs`
- Hotkey: `src-tauri/src/qa_hotkey.rs`, `src-tauri/src/shortcut_binding.rs`
- Selection capture: `src-tauri/src/selection.rs`
- UI: `src/pages/QaPanel.tsx`, `src/pages/SelectionAsk.tsx`
- Window positioning/lifecycle: `src-tauri/src/lib.rs`

## Behavior

- QA hotkey is independent from dictation hotkey and translation hotkey.
- Open the question window from Selection Ask. Choose **Start question**, speak,
  then choose **Finish and ask**. The keyboard shortcuts remain optional.
- This flow uses the selected **computer microphone**, Volcengine speech
  recognition and the configured AI answering service. It does not switch the
  normal Listener dictation input or change device key mappings.
- The QA panel runs in a separate webview and can be pinned or dismissed.
- The initial question window is 360 × 300 logical pixels and can be resized
  down to 320 × 260. Setup help and conversation content scroll separately from
  the fixed Close and recording controls; longer answers do not enlarge the
  window automatically.
- The main page and question window show separate configuration-presence
  indicators for Volcengine recognition and the actual active answering service.
  Start stays unavailable until both are configured; Finish remains available
  for a recording already in progress. Configured does not imply healthy
  networking, valid authentication or available quota.
- **Open provider settings** goes directly to the Providers page, including
  when the main window has not yet been created. Inline setup help uses the
  same APP ID, Access Token and Resource ID names as the settings fields.
- Interface-language changes synchronize to existing question/capsule windows
  without changing provider credentials or device settings. Missing QA speech
  configuration is reported in the current interface language before selection
  capture or microphone startup.
- Selection Ask uses current selection/context where platform support allows it.
- Closing the QA window clears the QA session state.
- On Windows, show and hide share one GUI-thread native visibility path. The
  latest visibility intent wins; a stale queued show must not undo Close. This
  avoids mixing raw HWND visibility with Tao's cached hidden state. Pointer
  controls do not activate the window or take the source app's text selection.

## Verification

```powershell
cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run
npm run build
```

## Known Limits

- Selection capture and non-activating floating windows are platform-sensitive.
- Hotkey availability depends on OS permissions and desktop environment behavior.
