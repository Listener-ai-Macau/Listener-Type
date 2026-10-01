//! Own the clipboard across asynchronous paste and final transcript retention.
//! A queued shortcut is not a receipt: only an observed payload in the same
//! editor can release it. This is not acceptance of the entire session text.
//! All decisions are shared; native readers only observe.

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
    Paste(String, bool, PasteShortcut, Option<String>, Sender<PasteResult>),
    Copy(String, Sender<bool>),
    Retain(ClipboardTicket, String, bool, Sender<RetentionResult>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalAppendDecision {
    Append,
    AlreadyPresent,
    Changed,
    Unavailable,
}

struct PasteResult {
    status: InsertStatus,
    terminal_decision: Option<TerminalAppendDecision>,
}

/// An old paste receipt permits clipboard retention, not a new insertion.
/// A punctuation-only continuation needs the current editor and caret to
/// still contain this session's committed body. Missing readback is distinct
/// from an observed send/clear/edit; neither authorizes a standalone suffix.
fn terminal_append_decision(
    snapshot: Option<(&str, &str)>,
    caret_at_end: Option<bool>,
    expected: &str,
    suffix: &str,
) -> TerminalAppendDecision {
    let Some((document, selected)) = snapshot else {
        return TerminalAppendDecision::Unavailable;
    };
    if expected.is_empty() || !crate::transcript_boundary::terminal_punctuation_only(suffix)
        || !selected.is_empty() || caret_at_end == Some(false) {
        return TerminalAppendDecision::Changed;
    }
    if caret_at_end.is_none() {
        return TerminalAppendDecision::Unavailable;
    }
    // Native editor document ranges can include a trailing paragraph marker.
    // Other user whitespace and edits remain significant.
    let document = document.trim_end_matches(['\r', '\n']);
    if document.ends_with(&format!("{expected}{suffix}")) {
        TerminalAppendDecision::AlreadyPresent
    } else if document.ends_with(expected) {
        TerminalAppendDecision::Append
    } else {
        TerminalAppendDecision::Changed
    }
}

pub(crate) fn current_terminal_append_decision(expected: &str, suffix: &str) -> TerminalAppendDecision {
    let reader = crate::selection::PasteReader::capture();
    let before = reader.as_ref().and_then(|reader| reader.before().ok());
    let caret_at_end = reader.as_ref().and_then(|reader| reader.caret_at_end().ok());
    terminal_append_decision(before.as_ref().map(|(doc, selected)| (doc.as_str(), selected.as_str())),
        caret_at_end, expected, suffix)
}

static OWNER: Lazy<Mutex<u64>> = Lazy::new(|| Mutex::new(0));
static WORKER: Lazy<Sender<Command>> = Lazy::new(|| {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("clipboard-handoff".into())
        .spawn(move || run(receiver, None))
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
        .send(Command::Paste(text.into(), restore, shortcut, None, sender))
        .is_err()
    {
        return InsertStatus::Failed;
    }
    receiver.recv().map(|result| result.status).unwrap_or(InsertStatus::Failed)
}

pub(crate) fn paste_terminal_suffix(expected: &str, suffix: &str, restore: bool, shortcut: PasteShortcut)
    -> Result<InsertStatus, TerminalAppendDecision>
{
    let (sender, receiver) = mpsc::channel();
    if WORKER.send(Command::Paste(suffix.into(), restore, shortcut, Some(expected.into()), sender)).is_err() {
        return Ok(InsertStatus::Failed);
    }
    match receiver.recv() {
        Ok(result) if result.terminal_decision == Some(TerminalAppendDecision::Append) => Ok(result.status),
        Ok(result) => Err(result.terminal_decision.unwrap_or(TerminalAppendDecision::Unavailable)),
        Err(_) => Ok(InsertStatus::Failed),
    }
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
    dispatched_at: Instant,
    observation: PasteObservation,
}

#[derive(Default)]
struct PasteObservation {
    consumed: bool,
    proof: Option<PasteConsumptionProof>,
    attempts: u32,
    last_shape: Option<(usize, usize, usize, bool)>,
    read_failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteConsumptionProof {
    ExactEdit,
    WholePayloadDocument,
}

impl PasteObservation {
    fn observe(&mut self, before: &str, selected: &str, payload: &str, after: &str) {
        if self.consumed {
            return;
        }
        self.attempts += 1;
        self.last_shape = Some((
            before.chars().count(),
            selected.chars().count(),
            after.chars().count(),
            before == after,
        ));
        self.read_failed = false;
        // This receipt is a fact about the dispatched paste, not about whatever
        // the editor contains at finalization. Sending or editing the message
        // later cannot undo an already observed consumption of this payload.
        self.proof = paste_consumption_proof(before, selected, payload, after);
        self.consumed = self.proof.is_some();
    }
}

impl PasteReceipt {
    fn observing(&self) -> bool {
        !self.observation.consumed && self.reader.is_some() && self.before.is_some()
    }

    fn observe(&mut self) {
        if !self.observing() {
            return;
        }
        let (before, selected) = self.before.as_ref().expect("snapshot checked");
        match self.reader.as_ref().expect("reader checked").document() {
            Ok(after) => {
                let first_changed_read = self.observation.last_shape.is_none_or(|shape| shape.3) && before != &after;
                self.observation.observe(before, selected, &self.payload, &after);
                if first_changed_read && !self.observation.consumed {
                    log::info!("[insertion] paste readback changed without receipt ticket={} shape={:?} before_lines={} after_lines={} payload_chars={}",
                        self.ticket, self.observation.last_shape, before.matches('\n').count(), after.matches('\n').count(), self.payload.chars().count());
                }
            }
            Err(_) => {
                self.observation.attempts += 1;
                self.observation.read_failed = true;
            }
        }
        if self.observation.consumed {
            log::info!("[insertion] paste receipt observed ticket={} elapsed_ms={} observations={} payload_chars={} proof={:?}",
                self.ticket, self.dispatched_at.elapsed().as_millis(), self.observation.attempts, self.payload.chars().count(), self.observation.proof);
            self.reader = None;
            self.before = None;
        } else if self.dispatched_at.elapsed() >= Duration::from_secs(5) {
            log::warn!("[insertion] paste receipt unconfirmed ticket={} observations={} read_failed={} shape={:?} payload_chars={}",
                self.ticket, self.observation.attempts, self.observation.read_failed, self.observation.last_shape, self.payload.chars().count());
            self.reader = None;
            self.before = None;
        }
    }
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

fn run(receiver: Receiver<Command>, initial_receipt: Option<PasteReceipt>) {
    // Native UIA/AX objects are constructed, used and dropped on this worker.
    // In particular, no COM pointer or apartment guard crosses threads.
    let mut paste_receipt = initial_receipt;
    let mut pending: Option<PendingRetention> = None;
    loop {
        // Observe from dispatch onward, including the interval before a final
        // retention request. Waiting until the end misses the editor state
        // after a successfully consumed paste if the user has sent the message.
        let command =
            if pending.is_some() || paste_receipt.as_ref().is_some_and(PasteReceipt::observing) {
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
            Some(Command::Paste(text, restore, shortcut, expected, reply)) => {
                let capture_started = Instant::now();
                let reader = if restore && expected.is_none() {
                    None
                } else {
                    crate::selection::PasteReader::capture()
                };
                let before = reader.as_ref().and_then(|reader| reader.before().ok());
                let terminal_decision = expected.as_ref().map(|expected| {
                    let caret_at_end = reader.as_ref().and_then(|reader| reader.caret_at_end().ok());
                    terminal_append_decision(before.as_ref().map(|(doc, selected)| (doc.as_str(), selected.as_str())),
                        caret_at_end, expected, &text)
                });
                if terminal_decision.is_some_and(|decision| decision != TerminalAppendDecision::Append) {
                    log::info!("[insertion] terminal suffix not pasted decision={terminal_decision:?} suffix_chars={}", text.chars().count());
                    let _ = reply.send(PasteResult { status: InsertStatus::Failed, terminal_decision });
                    continue;
                }
                finish(&mut pending, RetentionResult::Superseded);
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
                    if super::copy_transport_to_clipboard_now(&text) {
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
                    dispatched_at: Instant::now(),
                    observation: PasteObservation::default(),
                });
                drop(owner);
                let _ = reply.send(PasteResult { status: result, terminal_decision });
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
                    // Equal text still needs publication: the paste payload
                    // was explicitly excluded from history. Rewriting the
                    // same bytes as retained text cannot change what a delayed
                    // paste reads and requires no second shortcut or receipt.
                    let mut owner = OWNER.lock();
                    if !still_owns_transport(ticket.id, *owner, receipt.revision, clipboard_revision()) {
                        let _ = reply.send(RetentionResult::Superseded);
                        continue;
                    }
                    *owner = owner.wrapping_add(1);
                    let result = if super::copy_to_clipboard_now(&text) {
                        log::info!("[insertion] full clipboard published from identical transport ticket={} chars={}", ticket.id, text.chars().count());
                        RetentionResult::Stored
                    } else {
                        RetentionResult::Failed
                    };
                    paste_receipt = None;
                    let _ = reply.send(result);
                    continue;
                }
                if !receipt.observation.consumed && !receipt.observing() {
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
        if let Some(receipt) = paste_receipt.as_mut() {
            if still_owns_transport(
                receipt.ticket,
                *OWNER.lock(),
                receipt.revision,
                clipboard_revision(),
            ) {
                receipt.observe();
            } else {
                paste_receipt = None;
                finish(&mut pending, RetentionResult::Superseded);
            }
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
        if receipt.observation.consumed {
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

fn paste_consumption_proof(
    before: &str,
    selected: &str,
    payload: &str,
    after: &str,
) -> Option<PasteConsumptionProof> {
    let normalize = |value: &str| value.replace("\r\n", "\n").replace('\r', "\n");
    let (before, selected, payload, after) = (
        normalize(before),
        normalize(selected),
        normalize(payload),
        normalize(after),
    );
    if payload.is_empty() || before == after {
        return None;
    }
    // Some accessibility providers expose an empty editor's prompt as document
    // text. Do not guess which text is a prompt or rewrite the native snapshot.
    // A changed, complete document equal to this generation's payload proves
    // that the payload is already present, even without a reconstructable edit.
    // This authorizes clipboard handoff only, never earlier-text preservation
    // or whole-session delivery acceptance. A substring is not this proof.
    if selected.is_empty() && after == payload {
        return Some(PasteConsumptionProof::WholePayloadDocument);
    }
    if selected.len() > before.len()
        || before.len() - selected.len() + payload.len() != after.len()
    {
        return None;
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
            return Some(PasteConsumptionProof::ExactEdit);
        }
    }
    None
}

#[cfg(test)]
fn paste_was_consumed(before: &str, selected: &str, payload: &str, after: &str) -> bool {
    paste_consumption_proof(before, selected, payload, after).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_suffix_requires_current_body_and_caret_not_an_old_paste_receipt() {
        let body = "已完成的最后一句";
        assert_eq!(terminal_append_decision(Some((body, "")), Some(true), body, "。"), TerminalAppendDecision::Append);
        assert_eq!(terminal_append_decision(Some((&format!("旧消息\n{body}\r\n"), "")), Some(true), body, "。"), TerminalAppendDecision::Append);
        assert_eq!(terminal_append_decision(Some((&format!("{body}。"), "")), Some(true), body, "。"), TerminalAppendDecision::AlreadyPresent);
        // Enter sent the body and the host reused its editor for a new message.
        assert_eq!(terminal_append_decision(Some(("", "")), Some(true), body, "。"), TerminalAppendDecision::Changed);
        assert_eq!(terminal_append_decision(Some(("另一条消息", "")), Some(true), body, "。"), TerminalAppendDecision::Changed);
        assert_eq!(terminal_append_decision(Some((body, "")), Some(false), body, "。"), TerminalAppendDecision::Changed);
        assert_eq!(terminal_append_decision(Some((body, "一句")), Some(true), body, "。"), TerminalAppendDecision::Changed);
        assert_eq!(terminal_append_decision(None, None, body, "。"), TerminalAppendDecision::Unavailable);
        assert_eq!(terminal_append_decision(Some((body, "")), None, body, "。"), TerminalAppendDecision::Unavailable);
        assert_eq!(terminal_append_decision(Some((body, "")), Some(true), "", "。"), TerminalAppendDecision::Changed);
        assert_eq!(terminal_append_decision(Some((body, "")), Some(true), body, ".14"), TerminalAppendDecision::Changed);
    }

    /// Runs explicitly while dictation is idle. This uses the real clipboard,
    /// no keyboard injection or editor automation, and exercises the worker's
    /// single-clause publication as well as supersession/readback guards.
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires idle app and exclusive access to the real clipboard"]
    fn native_history_single_clause_publication_and_supersession() {
        use super::super::{set_clipboard_text, ClipboardWritePurpose};
        use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
        use windows::core::w;

        assert_eq!(std::env::var("LISTENER_CLIPBOARD_NATIVE_TEST").as_deref(), Ok("1"));
        assert!(super::super::clipboard_transport_is_reversible());
        let mut board = arboard::Clipboard::new().expect("native clipboard");
        let original = super::super::snapshot_clipboard(&mut board);
        let marker = unsafe { RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")) };
        assert_ne!(marker, 0);
        let seed = format!("Listener temporary clause {}", uuid::Uuid::new_v4());
        let final_body = format!("Listener complete body {}", uuid::Uuid::new_v4());
        let temporary_second = format!("{seed} transport-only second");
        let temporary_tail = format!("{seed} transport-only tail。");
        let newer_copy = format!("Listener newer user copy {}", uuid::Uuid::new_v4());
        let mut last_written_revision = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Three temporary clauses keep their ordinary readable text but
            // carry the native exclusion. The final body removes the marker.
            for clause in [&seed, &temporary_second, &temporary_tail] {
                set_clipboard_text(&mut board, clause, ClipboardWritePurpose::PasteTransport).unwrap();
                last_written_revision = clipboard_revision();
                assert_eq!(board.get_text().unwrap(), clause.as_str());
                assert!(unsafe { IsClipboardFormatAvailable(marker) }.is_ok());
                // Give the asynchronous system history monitor a chance to
                // inspect each item. This is test settling, never a paste or
                // retention delay in the production worker.
                std::thread::sleep(Duration::from_millis(400));
            }
            set_clipboard_text(&mut board, &final_body, ClipboardWritePurpose::RetainedText).unwrap();
            last_written_revision = clipboard_revision();
            assert_eq!(board.get_text().unwrap(), final_body);
            assert!(unsafe { IsClipboardFormatAvailable(marker) }.is_err());
            std::thread::sleep(Duration::from_millis(500));

            // Single-clause completion cannot take the old equal-text no-op:
            // it must promote the same payload into user clipboard history.
            set_clipboard_text(&mut board, &seed, ClipboardWritePurpose::PasteTransport).unwrap();
            let transport_revision = clipboard_revision();
            last_written_revision = transport_revision;
            let ticket = {
                let mut owner = OWNER.lock();
                *owner = owner.wrapping_add(1);
                *owner
            };
            let (commands, worker_rx) = mpsc::channel();
            let worker_payload = seed.clone();
            let worker = std::thread::spawn(move || run(worker_rx, Some(PasteReceipt {
                ticket,
                revision: transport_revision,
                payload: worker_payload,
                reader: None,
                before: None,
                dispatched_at: Instant::now(),
                observation: PasteObservation::default(),
            })));
            let (reply, results) = mpsc::channel();
            commands.send(Command::Retain(ClipboardTicket { id: ticket, revision: transport_revision }, final_body.clone(), true, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), RetentionResult::Unavailable);
            assert_eq!(board.get_text().unwrap(), seed);
            assert!(unsafe { IsClipboardFormatAvailable(marker) }.is_ok());
            let (reply, results) = mpsc::channel();
            commands.send(Command::Retain(ClipboardTicket { id: ticket, revision: transport_revision }, seed.clone(), true, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), RetentionResult::Stored);
            last_written_revision = clipboard_revision();
            assert_ne!(last_written_revision, transport_revision);
            assert_eq!(board.get_text().unwrap(), seed);
            assert!(unsafe { IsClipboardFormatAvailable(marker) }.is_err());
            std::thread::sleep(Duration::from_millis(500));

            // The old generation cannot publish after a newer user copy.
            set_clipboard_text(&mut board, &newer_copy, ClipboardWritePurpose::RetainedText).unwrap();
            last_written_revision = clipboard_revision();
            let (reply, results) = mpsc::channel();
            commands.send(Command::Retain(ClipboardTicket { id: ticket, revision: transport_revision }, final_body.clone(), true, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), RetentionResult::Superseded);
            assert_eq!(board.get_text().unwrap(), newer_copy);
            std::thread::sleep(Duration::from_millis(500));
            drop(commands);
            worker.join().unwrap();
        }));
        // Never restore over a copy the user made during this explicit check.
        if clipboard_revision() == last_written_revision {
            match original {
                super::super::ClipboardSnapshot::Text(text) => {
                    set_clipboard_text(&mut board, &text, ClipboardWritePurpose::PasteTransport).unwrap();
                }
                super::super::ClipboardSnapshot::Image(image) => {
                    use arboard::SetExtWindows;
                    board.set().exclude_from_history().image(image).unwrap();
                }
                super::super::ClipboardSnapshot::Absent => { board.clear().unwrap(); }
            }
        }
        if let Err(error) = result { std::panic::resume_unwind(error); }
        if let Ok(path) = std::env::var("LISTENER_CLIPBOARD_NATIVE_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
                "passed": true,
                "transport_only": [temporary_second, temporary_tail],
                "retained": [seed, final_body, newer_copy],
                "keyboard_injection": false,
                "unconfirmed_different_text": "Unavailable",
                "single_clause_publication": "Stored",
                "older_ticket_after_user_copy": "Superseded"
            })).unwrap()).unwrap();
        }
    }

    #[test]
    fn final_punctuation_receipt_releases_whole_body_without_reinsertion() {
        assert!(!paste_was_consumed("已经上屏", "", "。", "已经上屏"));
        assert!(paste_was_consumed("已经上屏", "", "。", "已经上屏。"));
        assert!(!paste_was_consumed("已经上屏。", "", "。", "已经上屏。"));
    }

    #[test]
    fn whole_payload_readback_releases_paste_with_inconsistent_empty_editor_snapshot() {
        let mut receipt = PasteObservation::default();
        receipt.observe("An accessible prompt\n", "", "。", "An accessible prompt\n");
        assert!(!receipt.consumed);
        receipt.observe("An accessible prompt\n", "", "。", "。");
        assert!(receipt.consumed);
        assert_eq!(receipt.proof, Some(PasteConsumptionProof::WholePayloadDocument));
        // A submitted message clears the editor, but not its observed receipt.
        receipt.observe("An accessible prompt\n", "", "。", "An accessible prompt\n");
        assert!(receipt.consumed);
        assert_eq!(receipt.proof, Some(PasteConsumptionProof::WholePayloadDocument));
    }

    #[test]
    fn payload_document_receipt_requires_new_exact_whole_payload() {
        assert!(!paste_was_consumed("。", "", "。", "。"));
        assert!(!paste_was_consumed("原文。", "", "。", "修改原文。"));
        assert!(!paste_was_consumed("提示\n", "", "正文。", "正文"));
        assert!(!paste_was_consumed("提示\n", "", "正文。", "正文。另一次编辑"));
        assert!(!paste_was_consumed("提示\n", "", "正文。", ""));
        assert!(!paste_was_consumed("提示\n", "不存在的选区", "正文。", "正文。"));
        assert_eq!(
            paste_consumption_proof("first clause", "", " final clause", "first clause final clause"),
            Some(PasteConsumptionProof::ExactEdit)
        );
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

    #[test]
    fn consumed_paste_receipt_survives_editor_submit_before_final_retention() {
        let mut receipt = PasteObservation::default();
        receipt.observe("第一句。", "", "最后一句。", "第一句。");
        assert!(
            !receipt.consumed,
            "a queued shortcut has not consumed its payload"
        );
        receipt.observe("第一句。", "", "最后一句。", "第一句。最后一句。");
        assert!(receipt.consumed);
        // The user sends the message while the three-second endpoint window
        // is still running. The clipboard must still retain the whole body.
        receipt.observe("第一句。", "", "最后一句。", "");
        assert!(
            receipt.consumed,
            "final retention uses the observed paste receipt"
        );
    }

    #[test]
    fn unobserved_or_superseded_paste_cannot_borrow_a_previous_receipt() {
        let mut first = PasteObservation::default();
        first.observe("前文", "", "第一段。", "前文第一段。");
        assert!(first.consumed);
        let mut second = PasteObservation::default();
        second.observe("前文第一段。", "", "第二段。", "");
        assert!(
            !second.consumed,
            "clearing the editor is not a paste acknowledgment"
        );
        assert!(
            !still_owns_transport(1, 2, Some(81), Some(82)),
            "a cached first receipt cannot authorize a newer write"
        );
        assert!(
            !still_owns_transport(2, 2, Some(82), Some(83)),
            "a user copy supersedes even a consumed receipt"
        );
    }
}
