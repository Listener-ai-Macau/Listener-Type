# QA And Selection Ask

## Status

- status: `implemented`
- scope: separate QA/selection question flow with its own hotkey and floating QA window

## Source Map

- Coordinator: `src-tauri/src/coordinator/qa.rs`, `src-tauri/src/coordinator.rs`
- Hotkey: `src-tauri/src/qa_hotkey.rs`, `src-tauri/src/shortcut_binding.rs`
- Selection capture: `src-tauri/src/selection.rs`
- UI: `src/pages/QaPanel.tsx`, `src/pages/SelectionAsk.tsx`
- Window positioning/lifecycle: `src-tauri/src/lib.rs`

## Behavior

- QA hotkey is independent from dictation hotkey and translation hotkey.
- The QA panel runs in a separate webview and can be pinned or dismissed.
- Selection Ask uses current selection/context where platform support allows it.
- Closing the QA window clears the QA session state.

## Verification

```powershell
cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run
npm run build
```

## Known Limits

- Selection capture and non-activating floating windows are platform-sensitive.
- Hotkey availability depends on OS permissions and desktop environment behavior.
