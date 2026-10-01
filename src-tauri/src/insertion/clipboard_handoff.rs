//! Own the clipboard across asynchronous paste and final transcript retention.
//! A queued shortcut is not a receipt: only a readback of the same editor can
//! release its payload. All decisions are shared; native readers only observe.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::types::{InsertStatus, PasteShortcut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetentionResult {
    Stored,
    Superseded,
    Unavailable,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) struct ClipboardTicket {
    id: u64,
    revision: Option<u64>,
}

enum Command {
    Paste(String, bool, PasteShortcut, Sender<InsertStatus>),
    Copy(String, Sender<bool>),
    Retain(ClipboardTicket, String, bool, Sender<RetentionResult>),
}

static OWNER: Lazy<Mutex<u64>> = Lazy::new(|| Mutex::new(0));
static WORKER: Lazy<Sender<Command>> = Lazy::new(|| {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("clipboard-handoff".into())
        .spawn(move || run(receiver))
        .expect("clipboard worker must start");
    sender
});

pub(crate) fn ticket() -> ClipboardTicket {
    let owner = OWNER.lock();
    ClipboardTicket {
        id: *owner,
        revision: clipboard_revision(),
    }
}

// Restore and correction writes also invalidate any earlier retention request.
pub(super) fn begin_external_write() -> parking_lot::MutexGuard<'static, u64> {
    let mut owner = OWNER.lock();
    *owner = owner.wrapping_add(1);
    owner
}

pub(super) fn paste(text: &str, restore: bool, shortcut: PasteShortcut) -> InsertStatus {
    let (sender, receiver) = mpsc::channel();
    if WORKER
        .send(Command::Paste(text.into(), restore, shortcut, sender))
        .is_err()
    {
        return InsertStatus::Failed;
    }
    receiver.recv().unwrap_or(InsertStatus::Failed)
}

pub(super) fn copy(text: &str) -> bool {
    let (sender, receiver) = mpsc::channel();
    WORKER.send(Command::Copy(text.into(), sender)).is_ok() && receiver.recv().unwrap_or(false)
}

pub(crate) fn retain(ticket: ClipboardTicket, text: &str, needs_receipt: bool) -> RetentionResult {
    let (sender, receiver) = mpsc::channel();
    if WORKER
        .send(Command::Retain(ticket, text.into(), needs_receipt, sender))
        .is_err()
    {
        return RetentionResult::Failed;
    }
    receiver.recv().unwrap_or(RetentionResult::Failed)
}

struct PasteReceipt {
    ticket: u64,
    revision: Option<u64>,
    payload: String,
    reader: Option<crate::selection::PasteReader>,
    before: Option<(String, String)>,
}

struct PendingRetention {
    ticket: u64,
    text: String,
    started: Instant,
    reply: Sender<RetentionResult>,
}

fn finish(pending: &mut Option<PendingRetention>, result: RetentionResult) {
    if let Some(job) = pending.take() {
        let _ = job.reply.send(result);
    }
}

fn run(receiver: Receiver<Command>) {
    // Native UIA/AX objects are constructed, used and dropped on this worker.
    // In particular, no COM pointer or apartment guard crosses threads.
    let mut paste_receipt: Option<PasteReceipt> = None;
    let mut pending: Option<PendingRetention> = None;
    loop {
        let command = if pending.is_some() {
            match receiver.recv_timeout(Duration::from_millis(40)) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        match command {
            Some(Command::Paste(text, restore, shortcut, reply)) => {
                finish(&mut pending, RetentionResult::Superseded);
                let capture_started = Instant::now();
                let reader = if restore {
                    None
                } else {
                    crate::selection::PasteReader::capture()
                };
                let before = reader.as_ref().and_then(|reader| reader.before().ok());
                log::info!("[insertion] paste readback captured available={} capture_ms={} payload_chars={}",
                    before.is_some(), capture_started.elapsed().as_millis(), text.chars().count());
                // The clipboard writer and receipt share one owner generation.
                let mut owner = OWNER.lock();
                *owner = owner.wrapping_add(1);
                let current_ticket = *owner;
                #[cfg(not(target_os = "macos"))]
                let result = super::insert_with_clipboard_restore_now(&text, restore, shortcut);
                #[cfg(target_os = "macos")]
                let result = {
                    let _ = (restore, shortcut);
                    if super::copy_to_clipboard_now(&text) {
                        super::macos_insert_status_after_paste(super::simulate_paste())
                    } else {
                        InsertStatus::Failed
                    }
                };
                paste_receipt = Some(PasteReceipt {
                    ticket: current_ticket,
                    revision: clipboard_revision(),
                    payload: text,
                    reader,
                    before,
                });
                drop(owner);
                let _ = reply.send(result);
            }
            Some(Command::Copy(text, reply)) => {
                finish(&mut pending, RetentionResult::Superseded);
                paste_receipt = None;
                let _owner = begin_external_write();
                let _ = reply.send(super::copy_to_clipboard_now(&text));
            }
            Some(Command::Retain(ticket, text, needs_receipt, reply)) => {
                finish(&mut pending, RetentionResult::Superseded);
                if ticket.id != *OWNER.lock() || ticket.revision != clipboard_revision() {
                    let _ = reply.send(RetentionResult::Superseded);
                    continue;
                }
                if !needs_receipt {
                    let mut owner = OWNER.lock();
                    if *owner != ticket.id || ticket.revision != clipboard_revision() {
                        let _ = reply.send(RetentionResult::Superseded);
                        continue;
                    }
                    *owner = owner.wrapping_add(1);
                    paste_receipt = None;
                    let result = if super::copy_to_clipboard_now(&text) {
                        RetentionResult::Stored
                    } else {
                        RetentionResult::Failed
                    };
                    let _ = reply.send(result);
                    continue;
                }
                let Some(receipt) = paste_receipt
                    .as_ref()
                    .filter(|receipt| receipt.ticket == ticket.id)
                else {
                    let _ = reply.send(RetentionResult::Unavailable);
                    continue;
                };
                if text == receipt.payload && ticket.revision == receipt.revision {
                    // The transport already contains the entire body. There
                    // is no clipboard mutation to wait for or repeat.
                    let _ = reply.send(RetentionResult::Stored);
                    continue;
                }
                if receipt.before.is_none() || receipt.reader.is_none() {
                    log::warn!("[insertion] full clipboard retention unavailable: editor has no paste readback ticket={}", ticket.id);
                    let _ = reply.send(RetentionResult::Unavailable);
                    continue;
                }
                pending = Some(PendingRetention {
                    ticket: ticket.id,
                    text,
                    started: Instant::now(),
                    reply,
                });
            }
            None => {}
        }
        let (Some(job), Some(receipt)) = (pending.as_ref(), paste_receipt.as_ref()) else {
            continue;
        };
        // A newer dictation, user copy, or old clipboard restoration wins.
        let mut owner = OWNER.lock();
        if !still_owns_transport(job.ticket, *owner, receipt.revision, clipboard_revision()) {
            drop(owner);
            finish(&mut pending, RetentionResult::Superseded);
            continue;
        }
        let current_payload = arboard::Clipboard::new().and_then(|mut board| board.get_text());
        if current_payload.as_deref().ok() != Some(receipt.payload.as_str()) {
            drop(owner);
            finish(&mut pending, RetentionResult::Superseded);
            continue;
        }
        let consumed = receipt
            .before
            .as_ref()
            .zip(receipt.reader.as_ref())
            .and_then(|((before, selected), reader)| {
                reader
                    .document()
                    .ok()
                    .map(|after| paste_was_consumed(before, selected, &receipt.payload, &after))
            })
            .unwrap_or(false);
        if consumed {
            // Recheck after the native read, which may have yielded to the user.
            if clipboard_revision() != receipt.revision {
                drop(owner);
                finish(&mut pending, RetentionResult::Superseded);
                continue;
            }
            *owner = owner.wrapping_add(1);
            let result = if super::copy_to_clipboard_now(&job.text) {
                log::info!("[insertion] full clipboard retained after target paste receipt ticket={} chars={}", job.ticket, job.text.chars().count());
                RetentionResult::Stored
            } else {
                RetentionResult::Failed
            };
            drop(owner);
            finish(&mut pending, result);
            paste_receipt = None;
        } else if job.started.elapsed() >= Duration::from_secs(5) {
            // Timeout only abandons the job; it never authorizes overwriting a
            // payload that an asynchronous target may still be about to read.
            drop(owner);
            log::warn!("[insertion] full clipboard retention unconfirmed ticket={} reason=target_receipt_unavailable", job.ticket);
            finish(&mut pending, RetentionResult::Unavailable);
        }
    }
}

fn still_owns_transport(
    ticket: u64,
    owner: u64,
    written_revision: Option<u64>,
    current_revision: Option<u64>,
) -> bool {
    ticket == owner && written_revision == current_revision
}

fn clipboard_revision() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        Some(unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() } as u64)
    }
    #[cfg(target_os = "macos")]
    {
        Some(unsafe { objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() } as u64)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

fn paste_was_consumed(before: &str, selected: &str, payload: &str, after: &str) -> bool {
    let normalize = |value: &str| value.replace("\r\n", "\n").replace('\r', "\n");
    let (before, selected, payload, after) = (
        normalize(before),
        normalize(selected),
        normalize(payload),
        normalize(after),
    );
    if payload.is_empty()
        || before == after
        || selected.len() > before.len()
        || before.len() - selected.len() + payload.len() != after.len()
    {
        return false;
    }
    let prefix = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    // Verify the entire edit rather than searching for a matching phrase that
    // may already have existed in this document before the shortcut was sent.
    for offset in before
        .char_indices()
        .map(|(offset, _)| offset)
        .chain([before.len()])
    {
        if offset > prefix {
            break;
        }
        if before[offset..].starts_with(&selected)
            && after[offset..].starts_with(&payload)
            && before[offset + selected.len()..] == after[offset + payload.len()..]
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_punctuation_receipt_releases_whole_body_without_reinsertion() {
        assert!(!paste_was_consumed("已经上屏", "", "。", "已经上屏"));
        assert!(paste_was_consumed("已经上屏", "", "。", "已经上屏。"));
        assert!(!paste_was_consumed("已经上屏。", "", "。", "已经上屏。"));
    }

    #[test]
    fn paste_receipt_rejects_existing_phrase_or_unrelated_user_edit() {
        assert!(!paste_was_consumed("原文。", "", "。", "修改原文。"));
        assert!(!paste_was_consumed("原文。", "", "。", "原文。"));
        assert!(!paste_was_consumed("abc", "", "x", "abcc"));
    }

    #[test]
    fn receipt_handles_selection_unicode_line_endings_and_repeated_prefix() {
        assert!(paste_was_consumed(
            "开头旧稿结尾",
            "旧稿",
            "新稿。",
            "开头新稿。结尾"
        ));
        assert!(paste_was_consumed("abcd", "", "abca", "abcaabcd"));
        assert!(paste_was_consumed("a\r\nb", "", "语音\n", "a\n语音\nb"));
        assert!(!paste_was_consumed("原文", "不存在", "x", "x"));
    }

    #[test]
    fn deferred_retention_cannot_overwrite_new_session_or_user_copy() {
        assert!(still_owns_transport(4, 4, Some(81), Some(81)));
        assert!(
            !still_owns_transport(4, 5, Some(81), Some(81)),
            "a newer app write cancels the old job"
        );
        assert!(
            !still_owns_transport(4, 4, Some(81), Some(82)),
            "a user copy cancels even when its text happens to be the same"
        );
        assert!(!still_owns_transport(4, 5, Some(81), Some(82)));
    }
}
