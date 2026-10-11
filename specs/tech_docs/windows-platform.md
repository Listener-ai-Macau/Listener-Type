# Windows Platform Notes

Windows support spans packaging, global hotkeys, microphone privacy, clipboard behavior and TSF IME insertion.

## Source Areas

- `src-tauri/src/platform_hotkey.rs`
- `src-tauri/src/windows_ime.rs`
- `src-tauri/src/windows_ime_profile.rs`
- `windows-ime/**`
- `scripts/windows-*.ps1`
- `scripts/windows-*.test.mjs`

## Rules

- Keep Listener Type TSF CLSID/Profile GUID aligned across Rust, C++ and installer smoke scripts.
- Keep `ListenerTypeIme.dll` naming aligned across NSIS, WiX, Visual Studio project and smoke scripts.
- On non-Windows hosts, run JS static checks and record the missing runtime coverage.
- On Windows, verify install, registration, dictation insertion and uninstall cleanup.

## Live text completion

A stable clause already pasted during recording is final for the editor.
Later ASR revisions may update the provisional preview. Finalization normally
appends speech beyond the committed boundary. Raw paste sessions can supplement
middle omissions through the shared lossless native-word planner and a
continuous chain of actual same-editor paste receipts. UIA compares the native
focused elements, document and end caret, selects only the exact owned suffix,
and confirms the resulting edit. Existing delivered characters and user text
are preserved. It never sends Ctrl+C, selects the entire editor by shortcut,
restores focus or replays a full transcript when this witness is unavailable.
Ambiguous revisions, edits, sends, clears and changed targets cannot authorize
replacement or a fallback append.
The native source plan classifies a pure tail separately: only its unseen suffix
uses the ordinary current-cursor delivery route. It never selects or rewrites
the old range. Failed range repair retains the complete lossless recognized body
for clipboard recovery after the existing transport receipt permits closeout.

The expected resulting document comes from UIA's actual prefix, selection and
suffix ranges. Native range partitioning preserves both user-authored newlines
and UIA paragraph markers; trimming a document ending cannot manufacture an
owned session range. Prediction is tied to the unchanged captured document.

During dictation, each paste and final submit targets the currently focused
window. Delivery must not call `SetForegroundWindow`, restore the window that
had focus when recording began, or infer a target from its title. If the user
switches apps during an optional TSF composition, cancel that composition and
continue with the current foreground editor. A delayed post-dictation shortcut
may fire only while the editor that received the final text still has focus.

An accessibility Edit role does not imply write permission. Before preparing
a clipboard paste, the Windows adapter checks the focused control's native
ValuePattern read-only property and enabled state, then rechecks focus. A
known read-only or disabled field is NonEditable, so the shared delivery path
defers it before clipboard mutation and leaves the source prefix unconsumed.
Missing accessibility properties remain Unknown and retain the existing paste
fallback; they cannot establish either an editor receipt or non-editability.
