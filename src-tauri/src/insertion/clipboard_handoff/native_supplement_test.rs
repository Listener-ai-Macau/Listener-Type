//! Explicit native I/O regression for the saved middle-omission failure.
//! The unchanged operator renderer supplies the editor; no voice or cloud run.

use super::*;
use crate::selection::{FocusedInputState, PasteReader};
use crate::transcript_boundary::{lossless_final_supplement, TimedTranscript, TimedTranscriptWord};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

static UNAVAILABLE_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn fixture_capture_with_one_unavailable_observation() -> (Option<PasteReader>, FocusedInputState) {
    if UNAVAILABLE_ONCE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        (None, FocusedInputState::Unknown)
    } else { fixture_capture() }
}

fn fixture_capture() -> (Option<PasteReader>, FocusedInputState) {
    let expected: u32 = std::env::var("LISTENER_NATIVE_EDITOR_PID").unwrap().parse().unwrap();
    let mut actual = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut actual)); }
    if actual != expected { return (None, FocusedInputState::NonEditable); }
    PasteReader::capture_with_focus()
}

fn send(commands: &Sender<Command>, text: &str, scope: Option<SessionPasteScope>) -> PasteResult {
    let (reply, result) = mpsc::channel();
    commands.send(Command::Paste(text.into(), false, PasteShortcut::CtrlV, None, scope, reply)).unwrap();
    result.recv_timeout(Duration::from_secs(8)).expect("native paste dispatch completed")
}

fn confirmed(result: PasteResult, written_revision: &std::cell::Cell<Option<u64>>) -> PasteReceiptCompletion {
    assert!(matches!(result.status, InsertStatus::PasteSent | InsertStatus::Inserted), "{:?}", result.status);
    let handle = result.receipt.expect("actual native receipt handle");
    written_revision.set(handle.ticket.revision);
    let receipt = handle.receiver.unwrap().recv_timeout(Duration::from_secs(7)).expect("native editor receipt");
    assert!(matches!(receipt.outcome, PasteReceiptOutcome::Confirmed(_)), "{:?}", receipt.outcome);
    receipt
}

fn read_editor() -> String {
    let (reader, focus) = fixture_capture();
    assert_eq!(focus, FocusedInputState::Editable, "only the explicit fixture can receive test text");
    reader.unwrap().document().unwrap().trim_end_matches(['\r', '\n']).into()
}

#[test]
#[ignore = "explicit original popup and saved complete provider replay; real clipboard/editor writes, no voice or cloud"]
fn saved_revisable_frames_write_only_the_owned_range_and_confirm_without_second_paste() {
    assert_eq!(std::env::var("LISTENER_NATIVE_EDITOR_TEST").as_deref(), Ok("1"));
    let _ = env_logger::builder().is_test(true).try_init();
    let root = std::path::PathBuf::from(std::env::var_os("LISTENER_REVISABLE_REPLAY").unwrap());
    let source: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("source-replay.json")).unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while fixture_capture().1 != FocusedInputState::Editable {
        assert!(Instant::now() < deadline, "original input field is not focused");
        std::thread::sleep(Duration::from_millis(40));
    }
    assert_eq!(fixture_capture().0.unwrap().document().unwrap(), "", "native empty range must not expose its label as text");
    assert!(fixture_capture().0.unwrap().caret_at_end().unwrap());
    let mut board = arboard::Clipboard::new().unwrap();
    let original = super::super::snapshot_clipboard(&mut board);
    let written_revision = std::cell::Cell::new(None);
    let (commands, receiver) = mpsc::channel();
    UNAVAILABLE_ONCE.store(false, std::sync::atomic::Ordering::SeqCst);
    let worker = std::thread::spawn(move || run_with_capture(receiver, None, fixture_capture_with_one_unavailable_observation));
    let send_draft_once = |text: &str, session, previous: &str| {
        let (reply, result) = mpsc::channel();
        commands.send(Command::Revisable(text.into(), PasteShortcut::CtrlV, SessionPasteScope {
            session_id: session, previous_body: previous.into(), replace: !previous.is_empty(),
        }, reply)).unwrap();
        result.recv_timeout(Duration::from_secs(8)).unwrap()
    };
    let send_draft = |text: &str, session, previous: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let result = send_draft_once(text, session, previous);
            if result.status != InsertStatus::DeferredNoTarget
                || result.range_permission != SessionRangePermission::Unobserved { break result; }
            assert!(Instant::now() < deadline, "native observation did not recover");
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let session = uuid::Uuid::new_v4();
        let mut previous = String::new();
        let mut observations = Vec::new();
        let mut first_editor_body_ms = None;
        for frame in source["frames"].as_array().unwrap() {
            let Some(text) = frame["revisable"].as_str().and_then(|text| text.strip_prefix("开始录音")) else { continue; };
            let text = text.trim_start_matches(['。', '，', ',', ' ']);
            if crate::transcript_boundary::content_key(text).is_empty() || text == previous { continue; }
            if observations.len() == 1 {
                let revision = clipboard_revision();
                UNAVAILABLE_ONCE.store(true, std::sync::atomic::Ordering::SeqCst);
                let missing = send_draft_once(text, session, &previous);
                assert_eq!(missing.status, InsertStatus::DeferredNoTarget);
                assert_eq!(missing.range_permission, SessionRangePermission::Unobserved);
                assert!(missing.receipt.is_none());
                assert_eq!(clipboard_revision(), revision);
                assert_eq!(read_editor(), previous);
            }
            let receipt = confirmed(send_draft(text, session, &previous), &written_revision);
            assert_eq!(read_editor(), text, "every saved revision must replace, not append, its provisional range");
            first_editor_body_ms.get_or_insert(frame["elapsedMs"].as_u64().unwrap());
            observations.push(serde_json::json!({"frame":frame["frame"], "proof":format!("{:?}",receipt.outcome), "text":text}));
            previous = text.into();
        }
        assert_eq!(first_editor_body_ms, source["firstRevisableBodyMs"].as_u64());
        assert!(first_editor_body_ms.unwrap() < source["firstConfirmedBodyMs"].as_u64().unwrap());
        // Identical recognition confirmation is a fresh native range read,
        // not another keyboard paste or a recycled historical receipt.
        let revision = clipboard_revision();
        let owner = *OWNER.lock();
        let confirmation = confirmed(send_draft(&previous, session, &previous), &written_revision);
        assert_eq!(confirmation.outcome, PasteReceiptOutcome::Confirmed(PasteReceiptProof::OwnedSessionRange));
        assert_eq!(clipboard_revision(), revision);
        assert_eq!(*OWNER.lock(), owner);
        assert_eq!(read_editor(), previous);
        // Change the fixture's value exactly as a user edit/send would. The
        // next revision must leave both document and clipboard untouched.
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId};
        use windows::core::BSTR;
        let reader = fixture_capture().0.unwrap();
        let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let element = unsafe { automation.GetFocusedElement() }.unwrap();
        let value: IUIAutomationValuePattern = unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.unwrap();
        for changed in [format!("{previous} user edit"), String::new()] {
            unsafe { value.SetValue(&BSTR::from(changed.as_str())) }.unwrap();
            let before = reader.document().unwrap();
            let blocked = send_draft("A late cloud revision", session, &previous);
            assert_eq!(blocked.status, InsertStatus::DeferredNoTarget);
            assert!(blocked.receipt.is_none());
            assert_eq!(reader.document().unwrap(), before);
            assert_eq!(clipboard_revision(), revision);
        }
        std::fs::write(root.join("native-replay.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "sourceSessionId":source["sourceSessionId"], "sourceFrames":source["frames"].as_array().unwrap().len(),
            "firstRevisableBodySourceMs":first_editor_body_ms, "firstConfirmedBodySourceMs":source["firstConfirmedBodyMs"],
            "identicalConfirmationProof":format!("{:?}", confirmation.outcome),
            "duplicatePastes":0, "userEditRejected":true, "sendClearRejected":true,
            "transientMissingReadbackPreservedRange":true,
            "productionWorker":true, "productionNativeAdapter":true, "installedAppSession":false,
            "scope":"native replay of saved responses, not measured live provider latency", "observations":observations,
        })).unwrap()).unwrap();
    }));
    drop(commands);
    worker.join().unwrap();
    if written_revision.get().is_some() && clipboard_revision() == written_revision.get() {
        use super::super::{ClipboardSnapshot, ClipboardWritePurpose, set_clipboard_text};
        match original {
            ClipboardSnapshot::Text(text) => { set_clipboard_text(&mut board, &text, ClipboardWritePurpose::PasteTransport).unwrap(); }
            ClipboardSnapshot::Image(image) => { use arboard::SetExtWindows; board.set().exclude_from_history().image(image).unwrap(); }
            ClipboardSnapshot::Absent => { board.clear().unwrap(); }
        }
    }
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[test]
#[ignore = "requires the unchanged original popup, an explicit process id, saved native response, and an idle app"]
fn saved_final_supplement_writes_native_range_without_duplicate_or_prefix_loss() {
    assert_eq!(std::env::var("LISTENER_NATIVE_EDITOR_TEST").as_deref(), Ok("1"));
    let _ = env_logger::builder().is_test(true).try_init();
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(
        std::env::var_os("LISTENER_FINAL_SUPPLEMENT_FIXTURE").expect("explicit saved provider response")
    ).unwrap()).unwrap();
    let native = TimedTranscript {
        text: fixture["response"]["text"].as_str().unwrap().into(),
        words: fixture["response"]["utterances"].as_array().unwrap().iter()
            .flat_map(|u| u["words"].as_array().into_iter().flatten())
            .map(|w| TimedTranscriptWord { text: w["text"].as_str().unwrap().into(),
                start_ms: w["start_time"].as_u64(), end_ms: w["end_time"].as_u64() }).collect(),
    }.native_word_view().unwrap();
    let delivered = fixture["delivered"].as_str().unwrap();
    let body = native.text.strip_prefix("开始录音").unwrap();
    let repaired = lossless_final_supplement(delivered, &native, body).expect("saved final insertion plan");
    let prefix = "用户原有文字🔒：\r\n";
    // Startup waits only for this known fixture; never activate another app.
    let startup_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fixture_capture().1 == FocusedInputState::Editable { break; }
        assert!(Instant::now() < startup_deadline, "original editor did not become ready");
        std::thread::sleep(Duration::from_millis(40));
    }
    // An empty WinForms TextPattern may expose its accessible label. The
    // native ValuePattern independently confirms that this fixture is empty.
    let initial_reader = fixture_capture().0.unwrap();
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId};
    let initial_automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
    let initial_element = unsafe { initial_automation.GetFocusedElement() }.unwrap();
    let initial_value: IUIAutomationValuePattern = unsafe { initial_element.GetCurrentPatternAs(UIA_ValuePatternId) }.unwrap();
    assert_eq!(unsafe { initial_value.CurrentValue() }.unwrap().to_string(), "", "fixture must be empty before replay");
    drop((initial_value, initial_element, initial_automation, initial_reader));
    let mut board = arboard::Clipboard::new().unwrap();
    let original = super::super::snapshot_clipboard(&mut board);
    let written_revision = std::cell::Cell::new(None);
    let (commands, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || run_with_capture(receiver, None, fixture_capture));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        confirmed(send(&commands, prefix, None), &written_revision);
        let session = uuid::Uuid::new_v4();
        let mut previous = String::new();
        let mut receipt_count = 0;
        for clause in delivered.split_inclusive('。') {
            confirmed(send(&commands, clause, Some(SessionPasteScope {
                session_id: session, previous_body: previous.clone(), replace: false,
            })), &written_revision);
            previous.push_str(clause);
            assert_eq!(read_editor(), format!("{prefix}{previous}"));
            receipt_count += 1;
        }
        assert_eq!(previous, delivered);
        let receipt = confirmed(send(&commands, &repaired, Some(SessionPasteScope {
            session_id: session, previous_body: delivered.into(), replace: true,
        })), &written_revision);
        let actual = read_editor();
        assert_eq!(actual, format!("{prefix}{repaired}"));
        assert!(actual.contains("已经要完成的内容要保留时间就这么"));
        assert_eq!(actual.matches("已经要完成的内容要保留时间就这么").count(), 1);
        assert_eq!(repaired.replace("已经要完成的内容要保留时间就这么", ""), delivered);
        assert!(lossless_final_supplement(&repaired, &native, body).is_none());

        // Sending/clearing a consumed message cannot transfer its old edit
        // authorization to a new editor value. Mutate only the known fixture.
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId};
        use windows::core::BSTR;
        let reader = fixture_capture().0.unwrap(); // keeps this thread's COM apartment alive
        assert!(reader.is_current());
        let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let element = unsafe { automation.GetFocusedElement() }.unwrap();
        let value: IUIAutomationValuePattern = unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.unwrap();
        let changed = format!("{actual}用户自己的修改");
        unsafe { value.SetValue(&BSTR::from(changed.as_str())) }.unwrap();
        let changed_before = read_editor();
        assert_eq!(changed_before, changed);
        let revision_before = clipboard_revision();
        let blocked = send(&commands, &repaired, Some(SessionPasteScope {
            session_id: session, previous_body: repaired.clone(), replace: true,
        }));
        assert_eq!(blocked.status, InsertStatus::DeferredNoTarget);
        assert!(blocked.receipt.is_none());
        assert_eq!(clipboard_revision(), revision_before);
        assert_eq!(read_editor(), changed_before);
        unsafe { value.SetValue(&BSTR::from("")) }.unwrap();
        let cleared_document = reader.document().unwrap();
        let blocked = send(&commands, &repaired, Some(SessionPasteScope {
            session_id: session, previous_body: repaired.clone(), replace: true,
        }));
        assert_eq!(blocked.status, InsertStatus::DeferredNoTarget);
        assert_eq!(clipboard_revision(), revision_before);
        assert_eq!(reader.document().unwrap(), cleared_document);
        assert_eq!(unsafe { value.CurrentValue() }.unwrap().to_string(), "");
        std::fs::write(std::env::var_os("LISTENER_NATIVE_EDITOR_RESULT").unwrap(),
            serde_json::to_vec_pretty(&serde_json::json!({
                "savedSessionId": fixture["sessionId"], "streamedReceipts": receipt_count,
                "supplementReceipt": format!("{:?}", receipt.outcome),
                "actualBeforeUserEdit": actual, "preservedPrefix": prefix,
                "singleSupplement": true, "userEditRejected": true, "sendClearRejected": true,
                "productionWorker": true, "productionNativeAdapter": true,
                "installedAppSession": false,
            })).unwrap()).unwrap();
    }));
    drop(commands);
    worker.join().unwrap();
    // Restore only a clipboard revision still owned by this isolated worker.
    if written_revision.get().is_some() && clipboard_revision() == written_revision.get() {
        use super::super::{ClipboardSnapshot, ClipboardWritePurpose, set_clipboard_text};
        match original {
            ClipboardSnapshot::Text(text) => { set_clipboard_text(&mut board, &text, ClipboardWritePurpose::PasteTransport).unwrap(); }
            ClipboardSnapshot::Image(image) => { use arboard::SetExtWindows; board.set().exclude_from_history().image(image).unwrap(); }
            ClipboardSnapshot::Absent => { board.clear().unwrap(); }
        }
    }
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}
