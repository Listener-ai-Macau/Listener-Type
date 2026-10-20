//! Own the clipboard across asynchronous paste and final transcript retention.
//! A queued shortcut is not a receipt: only an observed payload in the same
//! editor can release it. This is not acceptance of the entire session text.
//! Shared session ownership authorizes lossless final supplements. Native
//! adapters can select only the exact receipted range in the current editor.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::types::{InsertStatus, PasteShortcut};

#[cfg(all(test, target_os = "windows"))]
#[path = "clipboard_handoff/native_supplement_test.rs"]
mod native_supplement_test;

#[cfg(all(test, target_os = "windows"))]
#[path = "clipboard_handoff/native_recording_retention_test.rs"]
mod native_recording_retention_test;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetentionResult {
    Stored,
    Superseded,
    Unavailable,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipboardTicket {
    id: u64,
    revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PasteReceiptProof {
    ExactEdit,
    WholePayloadDocument,
    /// No shortcut was sent: the current owned session range was read back.
    OwnedSessionRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PasteReceiptOutcome {
    Confirmed(PasteReceiptProof),
    Unsupported,
    TimedOut,
    Superseded,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PasteReceiptCompletion {
    pub(crate) ticket: ClipboardTicket,
    pub(crate) observed_at: Instant,
    pub(crate) outcome: PasteReceiptOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PasteReceiptPoll {
    Pending,
    Ready(PasteReceiptCompletion),
    Consumed,
}

pub(crate) struct PasteReceiptHandle {
    ticket: ClipboardTicket,
    receiver: Option<Receiver<PasteReceiptCompletion>>,
}

impl std::fmt::Debug for PasteReceiptHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PasteReceiptHandle")
            .field("ticket", &self.ticket)
            .field("consumed", &self.receiver.is_none())
            .finish()
    }
}

impl PasteReceiptHandle {
    pub(crate) fn completed(completion: PasteReceiptCompletion) -> Self {
        let (sender, receiver) = mpsc::channel();
        let ticket = completion.ticket;
        let _ = sender.send(completion);
        Self { ticket, receiver: Some(receiver) }
    }
    pub(crate) fn ticket(&self) -> ClipboardTicket {
        self.ticket
    }

    /// Non-blocking, one-shot receipt observation. A completed value remains
    /// queued in this handle even if a newer paste supersedes the worker's
    /// current lease before the coordinator polls it.
    pub(crate) fn try_take(&mut self) -> PasteReceiptPoll {
        let Some(receiver) = self.receiver.as_ref() else {
            return PasteReceiptPoll::Consumed;
        };
        match receiver.try_recv() {
            Ok(completion) => {
                self.receiver = None;
                PasteReceiptPoll::Ready(completion)
            }
            Err(mpsc::TryRecvError::Empty) => PasteReceiptPoll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                PasteReceiptPoll::Ready(PasteReceiptCompletion {
                    ticket: self.ticket,
                    observed_at: Instant::now(),
                    outcome: PasteReceiptOutcome::WorkerUnavailable,
                })
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct PasteSubmission {
    pub(crate) status: InsertStatus,
    pub(crate) receipt: Option<PasteReceiptHandle>,
    pub(crate) range_permission: SessionRangePermission,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRangePermission {
    NotRequested,
    /// No session body has been written; the current target cannot acquire it.
    Unavailable,
    Unobserved,
    Owned,
    Revoked,
}

enum Command {
    Paste(String, bool, PasteShortcut, Option<String>, Option<SessionPasteScope>, Sender<PasteResult>),
    Copy(String, Sender<bool>),
    CopyBefore(String, Option<u64>, Instant, Sender<bool>),
    Retain(ClipboardTicket, String, bool, Sender<RetentionResult>),
    RetainRecording(ClipboardTicket, uuid::Uuid, String, bool, Sender<RetentionResult>),
    Revisable(String, PasteShortcut, SessionPasteScope, Sender<PasteResult>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalAppendDecision {
    Append,
    AlreadyPresent,
    Changed,
    Unavailable,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionPasteScope {
    pub session_id: uuid::Uuid,
    pub previous_body: String,
    pub replace: bool,
}

struct PasteResult {
    status: InsertStatus,
    terminal_decision: Option<TerminalAppendDecision>,
    receipt: Option<PasteReceiptHandle>,
    range_permission: SessionRangePermission,
}

fn paste_dispatch_allowed(focus: crate::selection::FocusedInputState) -> bool {
    focus != crate::selection::FocusedInputState::NonEditable
}

fn revisable_permission(existing_body: bool, chain: bool, observed_revocation: bool,
    facts: [Option<bool>; 4]) -> SessionRangePermission {
    if observed_revocation || facts.contains(&Some(false)) {
        if existing_body { SessionRangePermission::Revoked }
        else { SessionRangePermission::Unavailable }
    } else if chain && facts.iter().all(|fact| *fact == Some(true)) {
        SessionRangePermission::Owned
    } else {
        SessionRangePermission::Unobserved
    }
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
    paste_with_receipt(text, restore, shortcut).status
}

pub(crate) fn paste_with_receipt(
    text: &str,
    restore: bool,
    shortcut: PasteShortcut,
) -> PasteSubmission {
    paste_scoped_with_receipt(text, restore, shortcut, None)
}

pub(crate) fn paste_scoped_with_receipt(text: &str, restore: bool, shortcut: PasteShortcut,
    scope: Option<SessionPasteScope>) -> PasteSubmission {
    let (sender, receiver) = mpsc::channel();
    if WORKER
        .send(Command::Paste(text.into(), restore, shortcut, None, scope, sender))
        .is_err()
    {
        return PasteSubmission {
            status: InsertStatus::Failed,
            receipt: None,
            range_permission: SessionRangePermission::NotRequested,
        };
    }
    receiver
        .recv()
        .map(|result| PasteSubmission {
            status: result.status,
            receipt: result.receipt,
            range_permission: result.range_permission,
        })
        .unwrap_or(PasteSubmission {
            status: InsertStatus::Failed,
            receipt: None,
            range_permission: SessionRangePermission::NotRequested,
        })
}

/// A draft has no blind paste fallback. The worker must own and be able to
/// select precisely this session's range, including on the first dispatch.
pub(crate) fn revisable_session_with_receipt(text: &str, shortcut: PasteShortcut,
    scope: SessionPasteScope) -> PasteSubmission {
    let (reply, result) = mpsc::channel();
    if WORKER.send(Command::Revisable(text.into(), shortcut, scope, reply)).is_err() {
        return PasteSubmission { status: InsertStatus::Failed, receipt: None,
            range_permission: SessionRangePermission::Revoked };
    }
    result.recv().map(|result| PasteSubmission { status: result.status, receipt: result.receipt,
        range_permission: result.range_permission })
        .unwrap_or(PasteSubmission { status: InsertStatus::Failed, receipt: None,
            range_permission: SessionRangePermission::Revoked })
}

pub(crate) fn paste_terminal_suffix(expected: &str, suffix: &str, restore: bool, shortcut: PasteShortcut,
    scope: Option<SessionPasteScope>) -> Result<InsertStatus, TerminalAppendDecision>
{
    let (sender, receiver) = mpsc::channel();
    if WORKER.send(Command::Paste(suffix.into(), restore, shortcut, Some(expected.into()), scope, sender)).is_err() {
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

/// An explicit UI copy cannot wait on WebView focus/permission. Expired queued
/// requests must never publish later and overwrite a more recent user copy.
pub(super) fn copy_explicit(text: &str) -> bool {
    let (sender, receiver) = mpsc::channel();
    let deadline = Instant::now() + Duration::from_secs(1);
    let requested_revision = clipboard_revision();
    WORKER.send(Command::CopyBefore(text.into(), requested_revision, deadline, sender)).is_ok()
        && receiver.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
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

pub(crate) fn retain_recording(origin: ClipboardTicket, session: uuid::Uuid,
    text: &str, needs_receipt: bool) -> RetentionResult {
    let (sender, receiver) = mpsc::channel();
    if WORKER.send(Command::RetainRecording(origin, session, text.into(), needs_receipt, sender)).is_err() {
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
    completion: Option<Sender<PasteReceiptCompletion>>,
    // Kept only until the next paste/final retention, on this worker thread.
    // This is an editor ownership witness, never an ASR/speaker permission.
    session_body: Option<(uuid::Uuid, String)>,
    observed_document: Option<String>,
    expected_session_document: Option<String>,
    strict_session_edit: bool,
    staged_selection: bool,
    dispatch_session: Option<uuid::Uuid>,
}

#[derive(Default)]
struct PasteObservation {
    consumed: bool,
    proof: Option<PasteReceiptProof>,
    attempts: u32,
    last_shape: Option<(usize, usize, usize, bool)>,
    read_failed: bool,
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
    fn ticket(&self) -> ClipboardTicket {
        ClipboardTicket {
            id: self.ticket,
            revision: self.revision,
        }
    }

    fn complete(&mut self, observed_at: Instant, outcome: PasteReceiptOutcome) {
        if let Some(sender) = self.completion.take() {
            let _ = sender.send(PasteReceiptCompletion {
                ticket: self.ticket(),
                observed_at,
                outcome,
            });
        }
    }

    fn observing(&self) -> bool {
        !self.observation.consumed && self.reader.is_some() && self.before.is_some()
    }

    fn complete_unsupported_if_needed(&mut self, observed_at: Instant) {
        if self.completion.is_some() && !self.observing() && !self.observation.consumed {
            self.complete(observed_at, PasteReceiptOutcome::Unsupported);
        }
    }

    fn expire_if_due(&mut self, observed_at: Instant) -> bool {
        if self.completion.is_none()
            || observed_at.saturating_duration_since(self.dispatched_at) < Duration::from_secs(5)
        {
            return false;
        }
        log::warn!("[insertion] paste receipt unconfirmed ticket={} observations={} read_failed={} shape={:?} payload_chars={}",
            self.ticket, self.observation.attempts, self.observation.read_failed, self.observation.last_shape, self.payload.chars().count());
        self.complete(observed_at, PasteReceiptOutcome::TimedOut);
        self.reader = None;
        self.before = None;
        true
    }

    fn observe_session_document(&mut self, after: &str, caret_at_end: Option<bool>) {
        let exact_document = self.observation.consumed && self.session_body.as_ref().is_some_and(|(_, body)|
            session_document_proves_range(self.expected_session_document.as_deref(), body,
                &self.payload, self.observation.proof, after, self.strict_session_edit));
        if exact_document && caret_at_end != Some(false) {
            // The consumed paste and exact document are already positive
            // facts. An unavailable caret query cannot erase them. A later
            // revision still needs a fresh, positively observed end caret.
            self.observed_document = Some(after.into());
        } else {
            self.session_body = None;
            self.observed_document = None;
        }
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
                // Final replacement must preserve the entire observed user
                // prefix. A generic whole-payload receipt alone cannot prove
                // that an editor honored the exact selected session range.
                if self.strict_session_edit && !self.session_body.as_ref().is_some_and(|(_, body)|
                    session_document_proves_range(self.expected_session_document.as_deref(), body,
                        &self.payload, self.observation.proof, &after, true)) {
                    self.observation.consumed = false;
                    self.observation.proof = None;
                }
                if first_changed_read && !self.observation.consumed {
                    log::info!("[insertion] paste readback changed without receipt ticket={} shape={:?} before_lines={} after_lines={} payload_chars={}",
                        self.ticket, self.observation.last_shape, before.matches('\n').count(), after.matches('\n').count(), self.payload.chars().count());
                }
                if self.observation.consumed {
                    let caret = self.reader.as_ref().and_then(|r| r.caret_at_end().ok());
                    self.observe_session_document(&after, caret);
                }
            }
            Err(_) => {
                self.observation.attempts += 1;
                self.observation.read_failed = true;
            }
        }
        if self.observation.consumed {
            let observed_at = Instant::now();
            let proof = self
                .observation
                .proof
                .expect("consumed paste has a receipt proof");
            log::info!("[insertion] paste receipt observed ticket={} elapsed_ms={} observations={} payload_chars={} proof={:?}",
                self.ticket, observed_at.saturating_duration_since(self.dispatched_at).as_millis(), self.observation.attempts, self.payload.chars().count(), self.observation.proof);
            self.complete(observed_at, PasteReceiptOutcome::Confirmed(proof));
            if self.session_body.is_none() && self.dispatch_session.is_none() { self.reader = None; }
            self.before = None;
        } else {
            self.expire_if_due(Instant::now());
        }
    }
}

impl Drop for PasteReceipt {
    fn drop(&mut self) {
        self.complete(Instant::now(), PasteReceiptOutcome::Superseded);
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
    run_with_capture(receiver, initial_receipt, crate::selection::PasteReader::capture_with_focus)
}

fn run_with_capture(receiver: Receiver<Command>, initial_receipt: Option<PasteReceipt>,
    capture: fn() -> (Option<crate::selection::PasteReader>, crate::selection::FocusedInputState)) {
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
        let (command, revisable) = match command {
            Some(Command::Revisable(text, shortcut, scope, reply)) =>
                (Some(Command::Paste(text, false, shortcut, None, Some(scope), reply)), true),
            Some(Command::RetainRecording(origin, session, text, needs_receipt, reply)) => {
                // The worker's actual paste lease owns its original revision.
                // A later user copy must not be adopted by querying the live
                // clipboard here. No paste in this recording uses its start
                // ticket, never another recording's most recent paste.
                let ticket = paste_receipt.as_ref()
                    .filter(|receipt| receipt.dispatch_session == Some(session))
                    .map(PasteReceipt::ticket).unwrap_or(origin);
                (Some(Command::Retain(ticket, text, needs_receipt, reply)), false)
            }
            command => (command, false),
        };
        match command {
            Some(Command::Paste(text, restore, shortcut, expected, scope, reply)) => {
                let capture_started = Instant::now();
                let (captured_reader, focus) = capture();
                if !paste_dispatch_allowed(focus) {
                    log::info!("[insertion] paste deferred: focused target is a non-editor payload_chars={} capture_ms={}",
                        text.chars().count(), capture_started.elapsed().as_millis());
                    // No write, shortcut, owner generation, or source commit.
                    // Keep an older paste lease intact until its own receipt.
                    let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                        terminal_decision: expected.as_ref().map(|_| TerminalAppendDecision::Unavailable),
                        receipt: None, range_permission: if revisable {
                            revisable_permission(scope.as_ref().is_some_and(|s| !s.previous_body.is_empty()),
                                false, true, [None; 4]) }
                            else { SessionRangePermission::NotRequested } });
                    continue;
                }
                let mut reader = if restore && expected.is_none() && scope.is_none() { None } else { captured_reader };
                let mut before = reader.as_ref().and_then(|reader| match reader.before() {
                    Ok(snapshot) => Some(snapshot),
                    Err(error) => {
                        log::info!("[insertion] native paste baseline unavailable session_id={:?} reason={error}",
                            scope.as_ref().map(|scope| scope.session_id));
                        None
                    }
                });
                // Observe a prior dispatch before advancing the chain. Native
                // object identity, exact document, empty selection and end
                // caret all have to agree; matching text alone is not ownership.
                if paste_receipt.as_ref().is_some_and(|prior|
                    !still_owns_transport(prior.ticket, *OWNER.lock(), prior.revision, clipboard_revision())) {
                    paste_receipt = None;
                }
                if let Some(prior) = paste_receipt.as_mut() { prior.observe(); }
                // Ordinary final tails, including punctuation, have the same
                // immutable session owner as streamed appends. A fresh native
                // snapshot is still required; finalization is not a receipt.
                if before.is_none() && !revisable
                    && scope.as_ref().is_some_and(|scope| !scope.replace) {
                    let recovered = recover_scoped_paste_snapshot(
                        scope.as_ref().map(|scope| scope.session_id),
                        paste_receipt.as_ref().and_then(|prior| prior.reader.as_ref().map(|reader|
                            (prior.dispatch_session, prior.observation.consumed, reader))),
                        crate::selection::PasteReader::recover_current_paste_snapshot);
                    if let Some((recovered_reader, snapshot)) = recovered {
                        reader = Some(recovered_reader);
                        before = Some(snapshot);
                        log::info!("[insertion] recovered fresh paste readback from same-session native editor after lookup failure");
                    }
                }
                let staged = revisable && scope.as_ref().zip(paste_receipt.as_ref()).is_some_and(|(scope, prior)| {
                    prior.staged_selection && prior.observation.consumed
                        && prior.session_body.as_ref().is_some_and(|(id, body)| *id == scope.session_id && body == &scope.previous_body)
                        && prior.reader.as_ref().zip(reader.as_ref()).is_some_and(|(old,new)|
                            old.same_target_observation(new).ok() == Some(true))
                        && before.as_ref().is_some_and(|(doc, selected)|
                            prior.observed_document.as_ref() == Some(doc) && selected == &scope.previous_body)
                });
                let chain = staged || scope.as_ref().is_some_and(|scope| {
                    scope.previous_body.is_empty() && !scope.replace || paste_receipt.as_ref().is_some_and(|prior| {
                        let same_target = prior.reader.as_ref().zip(reader.as_ref())
                            .is_some_and(|(old, new)| old.same_target(new));
                        session_range_owned(prior.session_body.as_ref().map(|(id, body)| (*id, body.as_str())),
                            prior.observation.consumed, prior.observed_document.as_deref(),
                            scope.session_id, &scope.previous_body,
                            before.as_ref().map(|(doc, selected)| (doc.as_str(), selected.as_str())),
                            same_target, reader.as_ref().and_then(|r| r.caret_at_end().ok()))
                    })
                });
                let mut range_permission = SessionRangePermission::NotRequested;
                if revisable {
                    let identity = paste_receipt.as_ref().and_then(|prior|
                        prior.reader.as_ref().zip(reader.as_ref()).and_then(|(old,new)| old.same_target_observation(new).ok()));
                    let revoked = scope.as_ref().is_some_and(|scope| !scope.previous_body.is_empty()
                        && (paste_receipt.is_none() || paste_receipt.as_ref().is_some_and(|prior|
                            prior.session_body.as_ref().is_some_and(|(id, body)| *id != scope.session_id || body != &scope.previous_body)
                                || identity == Some(false)
                                || prior.observed_document.as_ref().zip(before.as_ref()).is_some_and(|(old,(doc,_))| old != doc))));
                    let current = reader.as_ref().and_then(|r| r.current_observation().ok());
                    let unchanged = reader.as_ref().and_then(|r| r.before().ok())
                        .zip(before.as_ref()).map(|(fresh, old)| &fresh == old);
                    let selection = before.as_ref().map(|(_,selected)| selected.is_empty() || staged);
                    let caret = if staged { Some(true) } else { reader.as_ref().and_then(|r| r.caret_at_end().ok()) };
                    range_permission = revisable_permission(scope.as_ref().is_some_and(|s| !s.previous_body.is_empty()),
                        chain, revoked, [current, unchanged, selection, caret]);
                }
                if revisable && (text.is_empty() || range_permission != SessionRangePermission::Owned
                    || focus != crate::selection::FocusedInputState::Editable) {
                    log::info!("[insertion] revisable acquisition deferred session_id={:?} permission={range_permission:?} focus={focus:?}",
                        scope.as_ref().map(|scope| scope.session_id));
                    let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                        terminal_decision: None, receipt: None, range_permission });
                    continue;
                }
                if revisable {
                    let agrees = reader.as_ref().zip(before.as_ref())
                        .and_then(|(r, (document, _))| r.revisable_document_agrees(document).ok());
                    if agrees != Some(true) {
                        log::info!("[insertion] revisable baseline deferred: text/value agreement={agrees:?}");
                        let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                            terminal_decision: None, receipt: None,
                            range_permission: revisable_permission(scope.as_ref().is_some_and(|s| !s.previous_body.is_empty()),
                                false, false, [agrees, None, None, None]) });
                        continue;
                    }
                }
                // A cloud confirmation may not change the draft's bytes.
                // Verify the *current* owned range instead of sending a second
                // paste, manufacturing an edit receipt, or recycling its age.
                if revisable && !staged && scope.as_ref().is_some_and(|s| s.previous_body == text) {
                    let completion = PasteReceiptCompletion {
                        ticket: paste_receipt.as_ref().expect("nonempty owned range").ticket(),
                        observed_at: Instant::now(),
                        outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::OwnedSessionRange),
                    };
                    let _ = reply.send(PasteResult { status: InsertStatus::Inserted,
                        terminal_decision: None, receipt: Some(PasteReceiptHandle::completed(completion)), range_permission });
                    continue;
                }
                if let Some(scope) = scope.as_ref().filter(|s| s.replace) {
                    // A failed revision has no append/Unicode fallback. Those
                    // would duplicate the body instead of supplementing it.
                    if revisable && chain {
                        if let Some(prior) = paste_receipt.as_mut() { prior.staged_selection = true; }
                    }
                    let selected = staged || chain && focus == crate::selection::FocusedInputState::Editable
                        && reader.as_ref().zip(before.as_ref()).is_some_and(|(r, (doc, _))|
                            r.select_owned_suffix(doc, &scope.previous_body).is_ok());
                    if !selected {
                        log::info!("[insertion] final supplement deferred: session editor ownership changed session_id={}", scope.session_id);
                        let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                            terminal_decision: None, receipt: None, range_permission: if revisable {
                                SessionRangePermission::Unobserved } else { range_permission } });
                        continue;
                    }
                    before = reader.as_ref().and_then(|r| r.before().ok());
                    let current = reader.as_ref().and_then(|r| r.current_observation().ok());
                    let unchanged = before.as_ref().map(|(doc, selected)| selected == &scope.previous_body
                        && paste_receipt.as_ref().and_then(|p| p.observed_document.as_ref()) == Some(doc));
                    if current != Some(true) || unchanged != Some(true) {
                        let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                            terminal_decision: None, receipt: None, range_permission: if revisable {
                                revisable_permission(true, true, false, [current, unchanged, Some(true), Some(true)])
                            } else { range_permission } });
                        continue;
                    }
                }
                let session_body = scope.as_ref().filter(|_| chain).map(|s| (s.session_id,
                    if s.replace { text.clone() } else { format!("{}{text}", s.previous_body) }));
                let strict_session_edit = revisable || scope.as_ref().is_some_and(|s| s.replace);
                let expected_session_document = session_body.as_ref().and_then(|_| {
                    reader.as_ref()?.expected_document_after_paste(before.as_ref()?, &text).ok()
                });
                if revisable && (text.is_empty() || expected_session_document.is_none()) {
                    let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                        terminal_decision: None, receipt: None, range_permission: SessionRangePermission::Unobserved });
                    continue;
                }
                let terminal_decision = expected.as_ref().map(|expected| {
                    let caret_at_end = reader.as_ref().and_then(|reader| reader.caret_at_end().ok());
                    terminal_append_decision(before.as_ref().map(|(doc, selected)| (doc.as_str(), selected.as_str())),
                        caret_at_end, expected, &text)
                });
                if terminal_decision.is_some_and(|decision| decision != TerminalAppendDecision::Append) {
                    log::info!("[insertion] terminal suffix not pasted decision={terminal_decision:?} suffix_chars={}", text.chars().count());
                    let _ = reply.send(PasteResult { status: InsertStatus::Failed, terminal_decision, receipt: None, range_permission });
                    continue;
                }
                finish(&mut pending, RetentionResult::Superseded);
                log::info!("[insertion] paste readback captured available={} capture_ms={} payload_chars={}",
                    before.is_some(), capture_started.elapsed().as_millis(), text.chars().count());
                // The clipboard writer and receipt share one owner generation.
                let mut owner = OWNER.lock();
                if revisable {
                    let current = reader.as_ref().and_then(|r| r.current_observation().ok());
                    let fresh = reader.as_ref().map(|r| r.before());
                    let unchanged = fresh.as_ref().and_then(|r| r.as_ref().ok()).zip(before.as_ref()).map(|(new,old)| new == old);
                    if current != Some(true) || unchanged != Some(true) {
                        range_permission = revisable_permission(scope.as_ref().is_some_and(|s| !s.previous_body.is_empty()),
                            true, false, [current, unchanged, Some(true), Some(true)]);
                        log::warn!("[insertion] revisable permission changed before dispatch current={current:?} snapshot_equal={unchanged:?} read_error={:?}",
                            fresh.as_ref().and_then(|r| r.as_ref().err()));
                        let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                            terminal_decision: None, receipt: None, range_permission });
                        continue;
                    }
                }
                if scope.as_ref().is_some_and(|s| s.replace) && !paste_receipt.as_ref().is_some_and(|prior|
                    still_owns_transport(prior.ticket, *owner, prior.revision, clipboard_revision())) {
                    log::warn!("[insertion] session transport permission changed before dispatch ticket={:?} owner={} previous_revision={:?} current_revision={:?}",
                        paste_receipt.as_ref().map(|p| p.ticket), *owner,
                        paste_receipt.as_ref().and_then(|p| p.revision), clipboard_revision());
                    let _ = reply.send(PasteResult { status: InsertStatus::DeferredNoTarget,
                        terminal_decision: None, receipt: None, range_permission: if revisable {
                            SessionRangePermission::Revoked } else { range_permission } });
                    continue;
                }
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
                let revision = clipboard_revision();
                let dispatched_at = Instant::now();
                let dispatched = matches!(result, InsertStatus::PasteSent | InsertStatus::Inserted);
                let (completion, receipt_handle) = if dispatched {
                    let (completion, receiver) = mpsc::channel();
                    let ticket = ClipboardTicket {
                        id: current_ticket,
                        revision,
                    };
                    (
                        Some(completion),
                        Some(PasteReceiptHandle {
                            ticket,
                            receiver: Some(receiver),
                        }),
                    )
                } else {
                    (None, None)
                };
                let mut receipt = PasteReceipt {
                    ticket: current_ticket,
                    revision,
                    payload: text,
                    reader,
                    before,
                    dispatched_at,
                    observation: PasteObservation::default(),
                    completion,
                    session_body,
                    observed_document: None,
                    expected_session_document,
                    strict_session_edit,
                    staged_selection: false,
                    dispatch_session: scope.as_ref().map(|scope| scope.session_id),
                };
                receipt.complete_unsupported_if_needed(dispatched_at);
                paste_receipt = Some(receipt);
                drop(owner);
                let _ = reply.send(PasteResult {
                    status: result,
                    terminal_decision,
                    receipt: receipt_handle,
                    range_permission,
                });
            }
            Some(Command::Copy(text, reply)) => {
                finish(&mut pending, RetentionResult::Superseded);
                paste_receipt = None;
                let _owner = begin_external_write();
                let _ = reply.send(super::copy_to_clipboard_now(&text));
            }
            Some(Command::CopyBefore(text, requested_revision, deadline, reply)) => {
                // Check both queue delay and ownership-lock delay before any write.
                if Instant::now() >= deadline || requested_revision != clipboard_revision() {
                    let _ = reply.send(false);
                    continue;
                }
                let Some(mut owner) = OWNER.try_lock_until(deadline) else {
                    let _ = reply.send(false);
                    continue;
                };
                if Instant::now() >= deadline {
                    let _ = reply.send(false);
                    continue;
                }
                *owner = owner.wrapping_add(1);
                finish(&mut pending, RetentionResult::Superseded);
                paste_receipt = None;
                let _ = reply.send(super::copy_to_clipboard_now(&text));
            }
            Some(Command::Retain(ticket, text, needs_receipt, reply)) => {
                finish(&mut pending, RetentionResult::Superseded);
                if ticket.id != *OWNER.lock() || ticket.revision != clipboard_revision() {
                    let _ = reply.send(RetentionResult::Superseded);
                    continue;
                }
                // The worker owns paste leases across all callers/sessions.
                // A final no-target/copy-only decision cannot release a still
                // active earlier asynchronous paste by passing false here.
                let needs_receipt = needs_receipt || paste_receipt.as_ref()
                    .is_some_and(|receipt| receipt.ticket == ticket.id && receipt.revision == ticket.revision);
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
            Some(Command::Revisable(..)) => unreachable!("revisable command normalized before dispatch"),
            Some(Command::RetainRecording(..)) => unreachable!("recording retention normalized before dispatch"),
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

/// A native interface is a capability to query again, not a text/receipt
/// substitute. Only the same logical recording's consumed predecessor may
/// supply it, and recovery must produce a new snapshot with fresh identity.
fn recover_scoped_paste_snapshot<R>(session: Option<uuid::Uuid>,
    previous: Option<(Option<uuid::Uuid>, bool, &R)>,
    recover: impl FnOnce(&R) -> Option<(R, (String, String))>) -> Option<(R, (String, String))> {
    let session = session?;
    let (previous_session, consumed, reader) = previous?;
    if previous_session != Some(session) || !consumed { return None; }
    recover(reader)
}

fn session_document_proves_range(expected: Option<&str>, body: &str, payload: &str,
    proof: Option<PasteReceiptProof>, after: &str, strict: bool) -> bool {
    (expected == Some(after) || (!strict && body == payload && after == payload
        && proof == Some(PasteReceiptProof::WholePayloadDocument)))
        && (after.ends_with(body) || after.trim_end_matches(['\r', '\n']).ends_with(body))
}

/// Mutation authorization is stricter than a historical paste receipt. A
/// consumed message can release its clipboard after send, but cannot be edited.
fn session_range_owned(owned: Option<(uuid::Uuid, &str)>, consumed: bool,
    observed_document: Option<&str>, session: uuid::Uuid, expected: &str,
    current: Option<(&str, &str)>, same_target: bool, caret_end: Option<bool>) -> bool {
    consumed && same_target && caret_end == Some(true) && !expected.is_empty()
        && owned == Some((session, expected))
        && current.is_some_and(|(doc, selected)| selected.is_empty()
            && Some(doc) == observed_document
            && (doc.ends_with(expected) || doc.trim_end_matches(['\r', '\n']).ends_with(expected)))
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
) -> Option<PasteReceiptProof> {
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
        return Some(PasteReceiptProof::WholePayloadDocument);
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
            return Some(PasteReceiptProof::ExactEdit);
        }
    }
    None
}

#[cfg(test)]
fn paste_was_consumed(before: &str, selected: &str, payload: &str, after: &str) -> bool {
    paste_consumption_proof(before, selected, payload, after).is_some()
}

#[cfg(test)]
pub(crate) fn pending_receipt_handle_for_test(
    id: u64,
) -> (PasteReceiptHandle, Sender<PasteReceiptCompletion>) {
    let ticket = ClipboardTicket {
        id,
        revision: Some(id + 100),
    };
    let (completion, receiver) = mpsc::channel();
    (
        PasteReceiptHandle {
            ticket,
            receiver: Some(receiver),
        },
        completion,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_capture_failure_recovers_a_fresh_snapshot_without_reusing_old_text_or_receipt() {
        let session = uuid::Uuid::new_v4();
        let original_document = "用户原来的前文，已上屏的第一段。";
        let fresh_document = "用户后来编辑的前文，已上屏的第一段。";
        let payload = "这一段在点击勾之前已经说完。";
        let native_capability = 51_u64;
        let recovered = recover_scoped_paste_snapshot(Some(session),
            Some((Some(session), true, &native_capability)), |reader| {
                Some((*reader, (fresh_document.into(), String::new())))
            }).expect("a fresh same-editor snapshot must survive a transient discovery failure");
        assert_eq!(recovered.1.0, fresh_document);
        assert_ne!(recovered.1.0, original_document, "old receipted text is never the new baseline");
        let mut observation = PasteObservation::default();
        observation.observe(&recovered.1.0, &recovered.1.1, payload, fresh_document);
        assert!(!observation.consumed, "recovering the native query cannot release clipboard transport");
        observation.observe(&recovered.1.0, &recovered.1.1, payload, &format!("{fresh_document}{payload}"));
        assert!(observation.consumed, "only this new payload's observed edit releases full retention");
        assert_eq!(observation.proof, Some(PasteReceiptProof::ExactEdit));
    }

    #[test]
    fn final_append_and_punctuation_recovery_need_a_new_edit_receipt() {
        let session = uuid::Uuid::new_v4();
        let capability = 73_u64;
        // These are the two finalization producers which previously dropped
        // this session scope (ordinary tail) or excluded recovery (punctuation).
        for (before, payload, whole) in [
            ("Already delivered. ", "Final words.", "Already delivered. Final words."),
            ("最后一句", "。", "最后一句。"),
        ] {
            let scope = SessionPasteScope { session_id: session,
                previous_body: before.into(), replace: false };
            let recovered = recover_scoped_paste_snapshot(Some(scope.session_id),
                Some((Some(session), true, &capability)), |reader|
                    Some((*reader, (before.to_string(), String::new())))).unwrap();
            if crate::transcript_boundary::terminal_punctuation_only(payload) {
                assert_eq!(terminal_append_decision(Some((&recovered.1.0, &recovered.1.1)),
                    Some(true), &scope.previous_body, payload), TerminalAppendDecision::Append);
            }
            let mut observation = PasteObservation::default();
            observation.observe(&recovered.1.0, &recovered.1.1, payload, before);
            assert!(!observation.consumed, "old consumed prefix is not the new tail's receipt");
            observation.observe(&recovered.1.0, &recovered.1.1, payload, whole);
            assert!(observation.consumed, "new observed tail releases complete same-session recovery");
            assert_ne!(whole, payload, "retained History contains more than the transport suffix");
        }
    }

    #[test]
    fn paste_recovery_requires_same_session_consumed_predecessor_and_fresh_native_success() {
        let session = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let reader = 51;
        for (current, previous, consumed) in [(None,Some(session),true),
            (Some(session),None,true),(Some(session),Some(other),true),
            (Some(session),Some(session),false)] {
            let result = recover_scoped_paste_snapshot(current,Some((previous,consumed,&reader)),
                |_| -> Option<(i32,(String,String))> { panic!("unqualified old capability must not be queried") });
            assert!(result.is_none());
        }
        assert!(recover_scoped_paste_snapshot(Some(session),Some((Some(session),true,&reader)),
            |_| -> Option<(i32,(String,String))> { None }).is_none(),
            "unavailable/mismatching native identity or text remains unknown");
        let mut observation = PasteObservation::default();
        observation.observe("prefix", "", "new tail", "different input prefixnew tail");
        assert!(!observation.consumed, "a substring in another document cannot release clipboard transport");
    }

    #[test]
    fn noneditable_stream_and_final_do_not_borrow_clipboard_or_confirm_a_receipt() {
        let before = ticket();
        let (commands, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || run_with_capture(receiver, None,
            || (None, crate::selection::FocusedInputState::NonEditable)));
        let session = uuid::Uuid::new_v4();
        for (body, previous) in [("streamed words", ""), ("final words", "old prefix") ] {
            let (reply, result) = mpsc::channel();
            commands.send(Command::Paste(body.into(), false, PasteShortcut::CtrlV,
                None, Some(SessionPasteScope { session_id: session,
                    previous_body: previous.into(), replace: false }), reply)).unwrap();
            let result = result.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(result.status, InsertStatus::DeferredNoTarget);
            assert!(result.receipt.is_none(), "readonly Codex document cannot create a fake paste lease");
            assert_eq!(ticket(), before, "deferring a non-editor cannot write clipboard or acquire its owner");
        }
        drop(commands);
        worker.join().unwrap();
    }

    #[test]
    fn unavailable_first_target_does_not_revoke_a_range_that_was_never_written() {
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || run_with_capture(receive, None,
            || (None, crate::selection::FocusedInputState::NonEditable)));
        let session_id = uuid::Uuid::new_v4();
        for (body, permission) in [("", SessionRangePermission::Unavailable),
            ("previously written body", SessionRangePermission::Revoked)] {
            let (reply, result) = mpsc::channel();
            send.send(Command::Revisable("next body".into(), PasteShortcut::CtrlV,
                SessionPasteScope { session_id, previous_body: body.into(), replace: !body.is_empty() }, reply)).unwrap();
            let result = result.recv().unwrap();
            assert_eq!(result.status, InsertStatus::DeferredNoTarget);
            assert_eq!(result.range_permission, permission);
            assert!(result.receipt.is_none());
        }
        drop(send);
        worker.join().unwrap();
        for fact in 0..4 {
            let mut facts = [Some(true); 4];
            facts[fact] = Some(false);
            assert_eq!(revisable_permission(false, true, false, facts), SessionRangePermission::Unavailable);
            assert_eq!(revisable_permission(true, true, false, facts), SessionRangePermission::Revoked);
        }
        assert_eq!(revisable_permission(false, true, false, [None; 4]), SessionRangePermission::Unobserved);
        assert_eq!(revisable_permission(false, true, false, [Some(true); 4]), SessionRangePermission::Owned);
    }

    fn pending_receipt(
        id: u64,
        dispatched_at: Instant,
    ) -> (PasteReceipt, PasteReceiptHandle) {
        let ticket = ClipboardTicket {
            id,
            revision: Some(id + 100),
        };
        let (completion, receiver) = mpsc::channel();
        (
            PasteReceipt {
                ticket: ticket.id,
                revision: ticket.revision,
                payload: format!("payload-{id}"),
                reader: None,
                before: None,
                dispatched_at,
                observation: PasteObservation::default(),
                completion: Some(completion), session_body: None, observed_document: None, expected_session_document: None, strict_session_edit: false, staged_selection: false, dispatch_session: None,
            },
            PasteReceiptHandle {
                ticket,
                receiver: Some(receiver),
            },
        )
    }

    #[test]
    fn exact_receipt_survives_unavailable_caret_without_authorizing_an_unobserved_revision() {
        let session = uuid::Uuid::new_v4();
        let prefix = "用户原有文字🔒：\r\n";
        let body = "明天上午9点，我们先讨论产品的使用体验。";
        let document = format!("{prefix}{body}");
        let (mut receipt, _handle) = pending_receipt(71, Instant::now());
        receipt.payload = body.into();
        receipt.session_body = Some((session, body.into()));
        receipt.expected_session_document = Some(document.clone());
        receipt.strict_session_edit = true;
        receipt.observation.observe(prefix, "", body, &document);
        assert_eq!(receipt.observation.proof, Some(PasteReceiptProof::ExactEdit));

        // Reproduce the observed-paste/unavailable-native-query boundary.
        receipt.observe_session_document(&document, None);
        assert_eq!(receipt.session_body.as_ref().map(|(id, text)| (*id, text.as_str())), Some((session, body)));
        assert_eq!(receipt.observed_document.as_deref(), Some(document.as_str()));
        let owns = |current: &str, same_target: bool, caret| session_range_owned(
            receipt.session_body.as_ref().map(|(id, text)| (*id, text.as_str())),
            receipt.observation.consumed, receipt.observed_document.as_deref(),
            session, body, Some((current, "")), same_target, caret);
        assert!(!owns(&document, true, None));
        assert!(owns(&document, true, Some(true)), "a fresh native observation may resume the exact existing range");
        assert!(!owns(&document, false, Some(true)));
        assert!(!owns("", true, Some(true)));
        assert!(!owns(&format!("{document} user edit"), true, Some(true)));

        receipt.observe_session_document(&document, Some(false));
        assert!(receipt.session_body.is_none(), "a positively moved caret retires the range");
        assert!(receipt.observation.consumed, "later range retirement cannot retract the original paste receipt");
    }

    #[test]
    fn unavailable_caret_cannot_rescue_a_receipt_with_a_changed_document() {
        let session = uuid::Uuid::new_v4();
        let (mut receipt, _handle) = pending_receipt(72, Instant::now());
        receipt.payload = "body".into();
        receipt.session_body = Some((session, "body".into()));
        receipt.expected_session_document = Some("user prefix body".into());
        receipt.strict_session_edit = true;
        receipt.observation.observe("user prefix ", "", "body", "user prefix body");
        receipt.observe_session_document("body", None);
        assert!(receipt.session_body.is_none());
        assert!(receipt.observed_document.is_none());
        assert!(receipt.observation.consumed);
    }

    #[test]
    fn confirmed_receipt_is_pollable_once_and_survives_later_lease_drop() {
        let dispatched_at = Instant::now();
        let (mut receipt, mut handle) = pending_receipt(41, dispatched_at);
        assert_eq!(handle.try_take(), PasteReceiptPoll::Pending);
        let observed_at = dispatched_at + Duration::from_millis(53);
        receipt.complete(
            observed_at,
            PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
        );
        // Dropping or superseding the worker-side lease after confirmation
        // cannot replace the already queued positive fact.
        drop(receipt);
        let PasteReceiptPoll::Ready(completion) = handle.try_take() else {
            panic!("confirmed receipt must remain queued");
        };
        assert_eq!(completion.ticket, handle.ticket());
        assert_eq!(completion.observed_at, observed_at);
        assert_eq!(
            completion.outcome,
            PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit)
        );
        assert_eq!(handle.try_take(), PasteReceiptPoll::Consumed);
    }

    #[test]
    fn receipt_handle_distinguishes_unsupported_timeout_and_supersession() {
        let started = Instant::now();

        let (mut unsupported, mut unsupported_handle) = pending_receipt(51, started);
        unsupported.complete_unsupported_if_needed(started);
        assert!(matches!(
            unsupported_handle.try_take(),
            PasteReceiptPoll::Ready(PasteReceiptCompletion {
                outcome: PasteReceiptOutcome::Unsupported,
                ..
            })
        ));

        let (mut timed_out, mut timeout_handle) = pending_receipt(52, started);
        assert!(!timed_out.expire_if_due(started + Duration::from_millis(4_999)));
        assert_eq!(timeout_handle.try_take(), PasteReceiptPoll::Pending);
        assert!(timed_out.expire_if_due(started + Duration::from_secs(5)));
        assert!(matches!(
            timeout_handle.try_take(),
            PasteReceiptPoll::Ready(PasteReceiptCompletion {
                outcome: PasteReceiptOutcome::TimedOut,
                ..
            })
        ));

        let (superseded, mut superseded_handle) = pending_receipt(53, started);
        drop(superseded);
        assert!(matches!(
            superseded_handle.try_take(),
            PasteReceiptPoll::Ready(PasteReceiptCompletion {
                outcome: PasteReceiptOutcome::Superseded,
                ..
            })
        ));
    }

    #[test]
    fn an_old_receipt_cannot_identify_a_new_paste_ticket() {
        let started = Instant::now();
        let (mut old_receipt, mut old_handle) = pending_receipt(61, started);
        let (mut new_receipt, new_handle) = pending_receipt(62, started);
        old_receipt.complete(
            started + Duration::from_millis(40),
            PasteReceiptOutcome::Confirmed(PasteReceiptProof::WholePayloadDocument),
        );
        new_receipt.complete(
            started + Duration::from_millis(80),
            PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
        );
        let PasteReceiptPoll::Ready(old_completion) = old_handle.try_take() else {
            panic!("old completion missing");
        };
        assert_eq!(old_completion.ticket, old_handle.ticket());
        assert_ne!(old_completion.ticket, new_handle.ticket());
    }

    #[test]
    fn a_focused_non_editor_must_not_create_a_paste_lease_for_clipboard_only_dictation() {
        use crate::selection::FocusedInputState;
        // The affected recording was sent to a focused Window, not a text
        // field. A queued paste there stranded every clause in transport-only
        // clipboard history and made whole-body retention await no receiver.
        assert!(!paste_dispatch_allowed(FocusedInputState::NonEditable));
        assert!(paste_dispatch_allowed(FocusedInputState::Editable));
        // A provider failure alone is not evidence of a non-editor. Keep the
        // existing compatibility path and delayed-consumption safeguard.
        assert!(paste_dispatch_allowed(FocusedInputState::Unknown));
    }

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
        let final_body = std::env::var_os("LISTENER_CLIPBOARD_NATIVE_TEXT_FILE")
            .map(|path| std::fs::read_to_string(path).expect("recorded owner text fixture"))
            .unwrap_or_else(|| format!("Listener complete body {}", uuid::Uuid::new_v4()));
        assert!(!final_body.trim().is_empty());
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
            let worker = std::thread::spawn(move || run_with_capture(worker_rx, Some(PasteReceipt {
                ticket,
                revision: transport_revision,
                payload: worker_payload,
                reader: None,
                before: None,
                dispatched_at: Instant::now(),
                observation: PasteObservation::default(),
                completion: None, session_body: None, observed_document: None, expected_session_document: None, strict_session_edit: false, staged_selection: false, dispatch_session: None,
            }), || (None, crate::selection::FocusedInputState::NonEditable)));
            let (reply, results) = mpsc::channel();
            commands.send(Command::Retain(ClipboardTicket { id: ticket, revision: transport_revision }, final_body.clone(), true, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), RetentionResult::Unavailable);
            assert_eq!(board.get_text().unwrap(), seed);
            assert!(unsafe { IsClipboardFormatAvailable(marker) }.is_ok());
            // An empty/non-editor target does not dispatch another shortcut,
            // alter the clipboard or consume/replace a previous paste lease.
            let (reply, results) = mpsc::channel();
            commands.send(Command::Paste(temporary_second.clone(), false,
                PasteShortcut::CtrlV, None, None, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap().status, InsertStatus::DeferredNoTarget);
            assert_eq!(clipboard_revision(), transport_revision);
            assert_eq!(*OWNER.lock(), ticket);
            assert_eq!(board.get_text().unwrap(), seed);
            let (reply, results) = mpsc::channel();
            commands.send(Command::Retain(ClipboardTicket { id: ticket, revision: transport_revision }, final_body.clone(), false, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), RetentionResult::Unavailable);
            assert_eq!(board.get_text().unwrap(), seed);
            let (reply, results) = mpsc::channel();
            commands.send(Command::Retain(ClipboardTicket { id: ticket, revision: transport_revision }, seed.clone(), true, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), RetentionResult::Stored);
            last_written_revision = clipboard_revision();
            assert_ne!(last_written_revision, transport_revision);
            assert_eq!(board.get_text().unwrap(), seed);
            assert!(unsafe { IsClipboardFormatAvailable(marker) }.is_err());
            std::thread::sleep(Duration::from_millis(500));

            // With no outstanding transport lease, an entire no-editor
            // recording can publish its final body without a paste receipt.
            let no_paste_ticket = self::ticket();
            let (reply, results) = mpsc::channel();
            commands.send(Command::Paste(temporary_tail.clone(), false,
                PasteShortcut::CtrlV, None, None, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap().status, InsertStatus::DeferredNoTarget);
            assert_eq!(clipboard_revision(), no_paste_ticket.revision);
            let (reply, results) = mpsc::channel();
            commands.send(Command::Retain(no_paste_ticket, final_body.clone(), false, reply)).unwrap();
            assert_eq!(results.recv_timeout(Duration::from_secs(3)).unwrap(), RetentionResult::Stored);
            last_written_revision = clipboard_revision();
            assert_eq!(board.get_text().unwrap(), final_body);
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
                "non_editor_paste": "DeferredNoTarget: clipboard and owner generation unchanged",
                "non_editor_whole_body": "Stored: exact Unicode body, history exclusion absent",
                "older_paste_lease_with_no_target_caller": "Unavailable: transport preserved",
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
        assert_eq!(receipt.proof, Some(PasteReceiptProof::WholePayloadDocument));
        // A submitted message clears the editor, but not its observed receipt.
        receipt.observe("An accessible prompt\n", "", "。", "An accessible prompt\n");
        assert!(receipt.consumed);
        assert_eq!(receipt.proof, Some(PasteReceiptProof::WholePayloadDocument));
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
            Some(PasteReceiptProof::ExactEdit)
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


#[cfg(test)]
mod session_range_tests {
    use super::*;
    #[test]
    fn final_supplement_range_cannot_adopt_identical_preexisting_suffix_or_lost_user_prefix() {
        let exact=Some(PasteReceiptProof::ExactEdit);
        let whole=Some(PasteReceiptProof::WholePayloadDocument);
        assert!(session_document_proves_range(Some("user prefix body"),"body","body",exact,"user prefix body",false));
        assert!(!session_document_proves_range(None,"body","body",exact,"body inserted elsewhere body",false));
        assert!(!session_document_proves_range(Some("user prefix body"),"body","body",whole,"body",true));
        assert!(session_document_proves_range(None,"body","body",whole,"body",false),
            "an inconsistent initial editor snapshot can still own an exact whole payload document");
        assert!(!session_document_proves_range(None,"first. tail.","tail.",whole,"tail.",false),
            "a later isolated payload does not preserve the earlier session body");
        let document="用户原有文字🔒：\r\nfirst. tail.";
        assert!(session_document_proves_range(Some(document),"first. tail.","tail.",exact,document,false));
        assert!(!session_document_proves_range(Some("用户原有文字🔒：first. tail."),"first. tail.","tail.",exact,document,false),
            "a receipted paste must not invent a document with the user's newline removed");
        let marked=format!("{document}\r\n");
        assert!(session_document_proves_range(Some(&marked),"first. tail.","first. tail.",exact,&marked,true));
        assert!(!session_document_proves_range(Some(document),"first. tail.","first. tail.",exact,&marked,true),
            "native trailing text must also be represented in the exact edit witness");
    }

    #[test]
    fn final_supplement_range_requires_receipted_same_session_native_target_and_unchanged_document() {
        let id=uuid::Uuid::new_v4();
        let body="first. last.";
        let document=format!("existing user text {body}");
        let owns=|owned, consumed, observed, session, expected, current, same, caret|
            session_range_owned(owned,consumed,observed,session,expected,current,same,caret);
        assert!(owns(Some((id,body)),true,Some(document.as_str()),id,body,Some((document.as_str(),"")),true,Some(true)));
        assert!(!owns(Some((id,body)),false,Some(document.as_str()),id,body,Some((document.as_str(),"")),true,Some(true)));
        assert!(!owns(Some((id,body)),true,Some(document.as_str()),uuid::Uuid::new_v4(),body,Some((document.as_str(),"")),true,Some(true)));
        assert!(!owns(Some((id,body)),true,Some(document.as_str()),id,body,Some((document.as_str(),"")),false,Some(true)));
        assert!(!owns(Some((id,body)),true,Some(document.as_str()),id,body,Some((document.as_str(),"")),true,None));
        assert!(!owns(Some((id,body)),true,Some(document.as_str()),id,body,Some((document.as_str(),"selected")),true,Some(true)));
        for changed in ["", "sent and cleared", "same body in another document", "user edits first. last."] {
            assert!(!owns(Some((id,body)),true,Some(document.as_str()),id,body,Some((changed,"")),true,Some(true)));
        }
        assert!(!owns(None,true,Some(document.as_str()),id,body,Some((document.as_str(),"")),true,Some(true)),
            "matching pasted text alone cannot establish session ownership");
    }
}
