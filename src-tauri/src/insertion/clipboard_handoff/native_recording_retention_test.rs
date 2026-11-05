use super::*;

/// Reproduce a copy made after the worker's initial ownership check but before
/// its final publication. This test uses only its private station clipboard.
#[test]
#[ignore = "requires an isolated clipboard window station"]
fn retained_publication_preserves_copy_between_check_and_write() {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn GetProcessWindowStation() -> *mut c_void;
        fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
            info: *mut c_void, length: u32, needed: *mut u32) -> i32;
    }
    let mut name = [0u16; 128];
    let mut needed = 0;
    assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
        name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerClipboardCase_"));

    let mut board = arboard::Clipboard::new().unwrap();
    for newer in ["newer user copy", "transport fragment"] {
        board.set_text("transport fragment").unwrap();
        let checked_revision = clipboard_revision();
        assert_eq!(checked_revision, clipboard_revision()); // worker's earlier check
        board.set_text(newer).unwrap(); // another app copies before publication
        assert_ne!(checked_revision, clipboard_revision());
        assert_eq!(publish_retained_if_current("complete final text", checked_revision),
            RetentionResult::Superseded);
        assert_eq!(board.get_text().unwrap(), newer);
    }
    let current = clipboard_revision();
    assert_eq!(publish_retained_if_current("完整文字 English 𝄞", current), RetentionResult::Stored);
    assert_eq!(board.get_text().unwrap(), "完整文字 English 𝄞");
}

/// A controlled counterexample: the product worker's actual queue and the
/// station's real clipboard, with paste dispatch replaced by a fake target
/// that intentionally has not read yet. Never sends keys or accesses the
/// interactive desktop. This remains ignored outside a private window station.
#[test]
#[ignore = "requires an isolated clipboard window station"]
fn delayed_previous_paste_does_not_read_the_next_fragment() {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn GetProcessWindowStation() -> *mut c_void;
        fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
            info: *mut c_void, length: u32, needed: *mut u32) -> i32;
    }
    let mut name = [0u16; 128];
    let mut needed = 0;
    assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
        name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerClipboardCase_"));

    fn no_editor() -> (Option<crate::selection::PasteReader>, crate::selection::FocusedInputState) {
        (None, crate::selection::FocusedInputState::Unknown)
    }
    fn fake_dispatch(text: &str, _restore: bool, _shortcut: PasteShortcut) -> InsertStatus {
        arboard::Clipboard::new().unwrap().set_text(text).unwrap();
        InsertStatus::PasteSent
    }
    let (send, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || run_with_capture_and_dispatch(receive, None, no_editor, fake_dispatch));
    let submit = |text: &str| {
        let (reply, result) = mpsc::channel();
        send.send(Command::Paste(text.into(), false, PasteShortcut::CtrlV, None, None, reply)).unwrap();
        result.recv_timeout(Duration::from_secs(3)).unwrap().status
    };
    assert_eq!(submit("first fragment"), InsertStatus::PasteSent);
    assert_eq!(submit("second fragment"), InsertStatus::PasteSent);
    let delayed_read = arboard::Clipboard::new().unwrap().get_text().unwrap();
    drop(send);
    worker.join().unwrap();
    assert_eq!(delayed_read, "first fragment", "the first target had not read when the second dispatch replaced its payload");
}

#[test]
#[ignore = "requires an isolated clipboard window station"]
fn explicit_history_copy_does_not_wait_for_focus_or_publish_expired_work() {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn GetProcessWindowStation() -> *mut c_void;
        fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
            info: *mut c_void, length: u32, needed: *mut u32) -> i32;
    }
    let mut name = [0u16; 128]; let mut needed = 0;
    assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
        name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerClipboardCase_"));
    let mut clipboard = arboard::Clipboard::new().unwrap();
    clipboard.set_text("newer user copy must survive").unwrap();
    // A more recent real clipboard copy supersedes even an unexpired request.
    let owner = OWNER.lock();
    let (reply, result) = mpsc::channel();
    WORKER.send(Command::CopyBefore("older queued action".into(), clipboard_revision(),
        Instant::now() + Duration::from_secs(1), reply)).unwrap();
    clipboard.set_text("new user copy while queued").unwrap();
    drop(owner);
    assert!(!result.recv_timeout(Duration::from_secs(2)).unwrap());
    assert_eq!(clipboard.get_text().unwrap(), "new user copy while queued");
    // The worker reaches the ownership lock but cannot acquire it in time.
    let owner = OWNER.lock();
    assert!(!copy_explicit("expired History action"));
    drop(owner);
    assert_eq!(clipboard.get_text().unwrap(), "new user copy while queued");
    assert!(copy_explicit("完整历史正文。Exact final sentence. 最後の文。"));
    assert_eq!(clipboard.get_text().unwrap(), "完整历史正文。Exact final sentence. 最後の文。");
}

/// The actual clipboard/worker with seeded dispatch facts, without sending a
/// shortcut or obtaining an editor's focus. The parent owns a private station.
#[test]
#[ignore = "requires an isolated clipboard window station"]
fn recording_retention_uses_only_its_own_paste_lease() {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn GetProcessWindowStation() -> *mut c_void;
        fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
            info: *mut c_void, length: u32, needed: *mut u32) -> i32;
    }
    let mut name = [0u16; 128];
    let mut needed = 0;
    assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
        name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerClipboardCase_"));

    for (completion_first, consumed, foreign, new_copy, expected) in [
        (false, true, false, false, RetentionResult::Stored),
        (false, false, false, false, RetentionResult::Unavailable),
        (false, true, false, true, RetentionResult::Superseded),
        (false, true, true, false, RetentionResult::Superseded),
        (true, false, false, false, RetentionResult::StoredWithoutPasteConfirmation),
        (true, true, false, false, RetentionResult::Stored),
        (true, false, false, true, RetentionResult::Superseded),
        (true, false, true, false, RetentionResult::Superseded),
    ] {
        let session = uuid::Uuid::new_v4();
        let mut board = arboard::Clipboard::new().unwrap();
        board.set_text("before recording").unwrap();
        let origin = ticket();
        // These are the same native transport and generation facts as a sent
        // paste, but no keyboard injection is needed to test retention.
        let transport = "submitted prefix";
        let dispatch_ticket = {
            let owner = begin_external_write();
            super::super::set_clipboard_text(&mut board, transport,
                super::super::ClipboardWritePurpose::PasteTransport).unwrap();
            ClipboardTicket { id: *owner, revision: clipboard_revision() }
        };
        if new_copy { board.set_text("new user copy").unwrap(); }
        let (commands, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
        // Construct even an empty native-reader slot on its owning thread;
        // actual COM reader types intentionally cannot be sent across threads.
        let receipt = PasteReceipt {
            ticket: dispatch_ticket.id, revision: dispatch_ticket.revision,
            payload: transport.into(), reader: None, before: None, value_before: None, last_value_probe: None,
            dispatched_at: Instant::now() - if completion_first && !consumed {
                PASTE_RECEIPT_OBSERVATION_BUDGET - Duration::from_millis(300)
            } else { Duration::ZERO },
            observation: PasteObservation { consumed,
                proof: consumed.then_some(PasteReceiptProof::ExactEdit),
                ..Default::default() },
            completion: None, session_body: None, observed_document: None,
            expected_session_document: None, strict_session_edit: false,
            staged_selection: false,
            dispatch_session: Some(if foreign { uuid::Uuid::new_v4() } else { session }),
        };
        run_with_capture_and_dispatch(receiver, Some(receipt),
            || (None, crate::selection::FocusedInputState::NonEditable),
            |_, _, _| panic!("final clipboard publication must never dispatch another paste"));
        });
        let (reply, results) = mpsc::channel();
        let final_text = "submitted prefix plus complete final sentence".into();
        commands.send(if completion_first {
            Command::PublishRecording(origin, session, final_text, false, reply)
        } else {
            Command::RetainRecording(origin, session, final_text, false, reply)
        }).unwrap();
        if completion_first && !consumed && !foreign && !new_copy {
            assert!(matches!(results.recv_timeout(Duration::from_millis(70)), Err(mpsc::RecvTimeoutError::Timeout)),
                "a final copy cannot replace the just-dispatched fragment immediately");
            assert_eq!(board.get_text().unwrap(), transport,
                "a delayed native clipboard read must still receive only its submitted fragment");
        }
        assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), expected,
            "completion_first={completion_first} consumed={consumed} foreign={foreign} new_copy={new_copy}");
        assert_eq!(board.get_text().unwrap(), match expected {
            RetentionResult::Stored | RetentionResult::StoredWithoutPasteConfirmation => "submitted prefix plus complete final sentence",
            _ if new_copy => "new user copy",
            _ => transport,
        });
        drop(commands);
        worker.join().unwrap();
    }
}

#[test]
#[ignore = "requires an isolated clipboard window station"]
fn completion_first_waiting_handoff_preserves_new_user_copy() {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn GetProcessWindowStation() -> *mut c_void;
        fn GetUserObjectInformationW(handle: *mut c_void, index: i32,
            info: *mut c_void, length: u32, needed: *mut u32) -> i32;
    }
    let mut name = [0u16; 128]; let mut needed = 0;
    assert_ne!(unsafe { GetUserObjectInformationW(GetProcessWindowStation(), 2,
        name.as_mut_ptr().cast(), (name.len() * 2) as u32, &mut needed) }, 0);
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    assert!(String::from_utf16_lossy(&name[..end]).starts_with("ListenerClipboardCase_"));

    for newer in ["new user copy", "last tail"] {
        let session = uuid::Uuid::new_v4();
        let mut board = arboard::Clipboard::new().unwrap();
        board.set_text("before recording").unwrap();
        let origin = ticket();
        let dispatch_ticket = {
            let owner = begin_external_write();
            super::super::set_clipboard_text(&mut board, "last tail",
                super::super::ClipboardWritePurpose::PasteTransport).unwrap();
            ClipboardTicket { id: *owner, revision: clipboard_revision() }
        };
        let (commands, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let receipt = PasteReceipt {
                ticket: dispatch_ticket.id, revision: dispatch_ticket.revision,
                payload: "last tail".into(), reader: None, before: None, value_before: None,
                last_value_probe: None,
                dispatched_at: Instant::now() - (PASTE_RECEIPT_OBSERVATION_BUDGET - Duration::from_millis(600)),
                observation: PasteObservation::default(), completion: None, session_body: None,
                observed_document: None, expected_session_document: None, strict_session_edit: false,
                staged_selection: false, dispatch_session: Some(session),
            };
            run_with_capture_and_dispatch(receiver, Some(receipt),
                || (None, crate::selection::FocusedInputState::Unknown),
                |_, _, _| panic!("copy must never send another paste"));
        });
        let (reply, results) = mpsc::channel();
        commands.send(Command::PublishRecording(origin, session, "complete final body and last tail".into(), true, reply)).unwrap();
        assert!(matches!(results.recv_timeout(Duration::from_millis(70)), Err(mpsc::RecvTimeoutError::Timeout)));
        assert_eq!(board.get_text().unwrap(), "last tail");
        board.set_text(newer).unwrap();
        assert_eq!(results.recv_timeout(Duration::from_secs(2)).unwrap(), RetentionResult::Superseded);
        assert_eq!(board.get_text().unwrap(), newer, "even an equal-text newer copy owns a new revision");
        drop(commands);
        worker.join().unwrap();
    }
}
