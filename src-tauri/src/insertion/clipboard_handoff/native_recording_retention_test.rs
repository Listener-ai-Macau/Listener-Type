use super::*;

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
    // The worker reaches the ownership lock but cannot acquire it in time.
    let owner = OWNER.lock();
    assert!(!copy_explicit("expired History action"));
    drop(owner);
    assert_eq!(clipboard.get_text().unwrap(), "newer user copy must survive");
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

    for (consumed, foreign, new_copy, expected) in [
        (true, false, false, RetentionResult::Stored),
        (false, false, false, RetentionResult::Unavailable),
        (true, false, true, RetentionResult::Superseded),
        (true, true, false, RetentionResult::Superseded),
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
            payload: transport.into(), reader: None, before: None,
            dispatched_at: Instant::now(),
            observation: PasteObservation { consumed,
                proof: consumed.then_some(PasteReceiptProof::ExactEdit),
                ..Default::default() },
            completion: None, session_body: None, observed_document: None,
            expected_session_document: None, strict_session_edit: false,
            staged_selection: false,
            dispatch_session: Some(if foreign { uuid::Uuid::new_v4() } else { session }),
        };
        run_with_capture(receiver, Some(receipt),
            || (None, crate::selection::FocusedInputState::NonEditable));
        });
        let (reply, results) = mpsc::channel();
        commands.send(Command::RetainRecording(origin, session,
            "submitted prefix plus complete final sentence".into(), false, reply)).unwrap();
        assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), expected,
            "consumed={consumed} foreign={foreign} new_copy={new_copy}");
        assert_eq!(board.get_text().unwrap(), match expected {
            RetentionResult::Stored => "submitted prefix plus complete final sentence",
            _ if new_copy => "new user copy",
            _ => transport,
        });
        drop(commands);
        worker.join().unwrap();
    }
}
