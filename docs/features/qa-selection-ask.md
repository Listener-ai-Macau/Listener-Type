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
