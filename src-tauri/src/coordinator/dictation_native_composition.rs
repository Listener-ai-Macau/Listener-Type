// Reversible text in ordinary accessible editors. Recognition finality stays
// in PauseEarlyDeliveryLedger; native range ownership stays in the clipboard
// worker. A draft neither consumes ASR words nor rearms the endpoint.

#[derive(Debug, Default)]
pub(super) struct NativeCompositionState {
    session_id: Option<SessionId>,
    source: Option<crate::asr::volcengine::LiveDeliverySource>,
    rendered: String,
    ever_sent: bool,
    disabled: bool,
    lost: bool,
    receipt: Option<crate::insertion::PasteReceiptHandle>,
    completion: Option<crate::insertion::PasteReceiptCompletion>,
    last_update_at: Option<Instant>,
    ticks: u64,
    submit_attempts: u64,
    accepted_transactions: u64,
    confirmed_readbacks: u64,
    last_gate: Option<&'static str>,
    last_input_status: Option<InsertStatus>,
    last_range_permission: Option<crate::insertion::SessionRangePermission>,
}

impl NativeCompositionState {
    fn poll(&mut self) {
        use crate::insertion::{PasteReceiptOutcome, PasteReceiptPoll};
        let Some(receipt) = self.receipt.as_mut() else {
            return;
        };
        match receipt.try_take() {
            PasteReceiptPoll::Pending => return,
            PasteReceiptPoll::Ready(done) => {
                if matches!(done.outcome, PasteReceiptOutcome::Confirmed(_)) {
                    self.confirmed_readbacks = self.confirmed_readbacks.saturating_add(1);
                    self.completion = Some(done);
                } else {
                    self.lost = true;
                    self.last_gate = Some("ReceiptUnconfirmed");
                    log::warn!(
                        "[native-composition] range receipt lost session_id={:?} outcome={:?}",
                        self.session_id,
                        done.outcome
                    );
                }
            }
            PasteReceiptPoll::Consumed => {
                self.lost = true;
                self.last_gate = Some("ReceiptAlreadyConsumed");
            }
        }
        self.receipt = None;
    }

    fn submit(
        &mut self,
        session_id: SessionId,
        confirmed: &str,
        text: &str,
        shortcut: crate::types::PasteShortcut,
    ) -> bool {
        self.submit_with(session_id, confirmed, text, shortcut,
            crate::insertion::revisable_session_with_receipt)
    }

    fn submit_with(
        &mut self,
        session_id: SessionId,
        confirmed: &str,
        text: &str,
        shortcut: crate::types::PasteShortcut,
        dispatch: impl FnOnce(&str, crate::types::PasteShortcut, crate::insertion::SessionPasteScope)
            -> crate::insertion::PasteSubmission,
    ) -> bool {
        self.poll();
        if self.session_id != Some(session_id)
            || self.disabled
            || self.lost
            || self.receipt.is_some()
            || text.is_empty()
        {
            return false;
        }
        let previous_body = if self.ever_sent {
            &self.rendered
        } else {
            confirmed
        };
        let submission = dispatch(
            text,
            shortcut,
            crate::insertion::SessionPasteScope {
                session_id,
                previous_body: previous_body.into(),
                replace: !previous_body.is_empty(),
            },
        );
        self.submit_attempts = self.submit_attempts.saturating_add(1);
        self.last_input_status = Some(submission.status);
        self.last_range_permission = Some(submission.range_permission);
        self.last_update_at = Some(Instant::now());
        if !matches!(
            submission.status,
            InsertStatus::PasteSent | InsertStatus::Inserted
        ) || submission.receipt.is_none()
        {
            // Once a draft was dispatched, a blind append/full final would
            // duplicate it or overwrite a user edit. Before the first draft,
            // unsupported editors keep ordinary confirmed-segment insertion.
            let revoked =
                submission.range_permission == crate::insertion::SessionRangePermission::Revoked;
            self.lost = self.ever_sent && revoked;
            self.disabled = !self.ever_sent && revoked;
            self.last_gate = Some("InputUnavailable");
            log::info!(
                "[native-composition] unavailable session_id={session_id} written={} status={:?} permission={:?}",
                self.ever_sent,
                submission.status, submission.range_permission
            );
            return false;
        }
        self.ever_sent = true;
        self.accepted_transactions = self.accepted_transactions.saturating_add(1);
        self.last_gate = Some("TransactionAccepted");
        self.rendered = text.into();
        self.completion = None;
        self.receipt = submission.receipt;
        self.last_update_at = Some(Instant::now());
        true
    }
}

/// The committed prefix always comes from the delivery ledger. A provider
/// rewrite cannot replace it while revising the unconfirmed tail.
fn native_composition_rendering(
    candidate: &str,
    confirmed: &str,
    source_key: &str,
) -> Option<String> {
    let remainder = pause_early_final_remainder(candidate, confirmed, source_key)?;
    Some(format!("{confirmed}{remainder}"))
}

fn native_composition_written(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let state = inner.native_composition.lock();
    state.session_id == Some(session_id) && state.ever_sent
}

fn native_composition_diagnostic(inner: &Arc<Inner>, session_id: SessionId) -> serde_json::Value {
    let state = inner.native_composition.lock();
    if state.session_id != Some(session_id) {
        return serde_json::json!({"ticks": 0, "submitAttempts": 0, "lastGate": "NotReached"});
    }
    serde_json::json!({
        "ticks": state.ticks,
        "submitAttempts": state.submit_attempts,
        "acceptedTransactions": state.accepted_transactions,
        "confirmedReadbacks": state.confirmed_readbacks,
        "lastGate": state.last_gate,
        "lastInputStatus": state.last_input_status.map(|value| format!("{value:?}")),
        "lastRangePermission": state.last_range_permission.map(|value| format!("{value:?}")),
        "rangeLost": state.lost,
        "disabled": state.disabled,
    })
}

// Output failure retires only output authority. It is never evidence of
// speaker silence and cannot request a physical recording STOP.
async fn native_composition_update_tick(
    inner: &Arc<Inner>,
    session_id: SessionId,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
) {
    {
        let session = inner.state.lock();
        if session.session_id != session_id
            || session.cancelled
            || session.phase != SessionPhase::Listening
        {
            return;
        }
    }
    {
        let mut state = inner.native_composition.lock();
        if state.session_id != Some(session_id) {
            *state = NativeCompositionState { session_id: Some(session_id), ..Default::default() };
        }
        state.ticks = state.ticks.saturating_add(1);
        state.poll();
    }
    if streaming_composition_active(inner, session_id) {
        inner.native_composition.lock().last_gate = Some("TsfCompositionActive");
        return;
    }
    if std::env::var("LISTENER_DISABLE_REVISABLE_EDITOR").as_deref() == Ok("1") {
        inner.native_composition.lock().last_gate = Some("DisabledByEnvironment");
        return;
    }
    // This path remains an explicit experiment until native readback is
    // accepted in real editors. Recognition and ordinary confirmed submission
    // continue normally; source/unit checks alone cannot enable editor writes.
    if std::env::var("LISTENER_ENABLE_REVISABLE_EDITOR").as_deref() != Ok("1") {
        inner.native_composition.lock().last_gate = Some("NativeAcceptanceRequired");
        return;
    }
    let Some(snapshot) = asr.revisable_owner_snapshot() else {
        inner.native_composition.lock().last_gate = Some("NoAdmittedSnapshot");
        return;
    };
    let Some(display) = live_delivery_display_text(inner, session_id, &snapshot.text, true) else {
        inner.native_composition.lock().last_gate = Some("DisplayPolicy");
        return;
    };
    let (confirmed, _) = pause_early_delivery_session_state(inner, session_id);
    let source_key = pause_early_delivery_source_key(inner, session_id);
    let Some(rendered) = native_composition_rendering(&display, &confirmed, &source_key) else {
        inner.native_composition.lock().last_gate = Some("ConfirmedPrefixMismatch");
        return;
    };
    if embedded_audio_partial_preview_stability_key(&rendered).is_empty() {
        inner.native_composition.lock().last_gate = Some("NoBody");
        return;
    }
    let mut state = inner.native_composition.lock();
    state.source.get_or_insert(snapshot.source);
    state.poll();
    if state
        .source
        .is_some_and(|source| !source.same_capture(snapshot.source))
    {
        state.lost = state.ever_sent;
        state.disabled = true;
        state.last_gate = Some("CaptureChanged");
        return;
    }
    if state.disabled
        || state.lost
        || state.receipt.is_some()
        || state.rendered == rendered
        || state
            .last_update_at
            .is_some_and(|at| at.elapsed() < STREAMING_COMPOSITION_MIN_UPDATE_INTERVAL)
    {
        return;
    }
    if state.submit(
        session_id,
        &confirmed,
        &rendered,
        inner.prefs.get().paste_shortcut,
    ) {
        log::info!("[native-composition] draft dispatched session_id={session_id} chars={} confirmed_chars={}",
            rendered.chars().count(), confirmed.chars().count());
    }
}

async fn native_composition_wait(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> Option<crate::insertion::PasteReceiptCompletion> {
    loop {
        {
            let mut state = inner.native_composition.lock();
            if state.session_id != Some(session_id) {
                return None;
            }
            state.poll();
            if state.lost || state.disabled {
                return None;
            }
            if state.receipt.is_none() {
                return state.completion;
            }
        }
        // The worker owns its existing native receipt expiry. Waiting here
        // cannot certify text, retract a word, or start the three-second clock.
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

async fn native_composition_commit_stable(
    inner: &Arc<Inner>,
    session_id: SessionId,
    delta: &str,
    confirmed: &str,
    full_key: &str,
    source_key: &str,
    consumed: crate::asr::volcengine::LiveBodyConsumption,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
) -> bool {
    if native_composition_wait(inner, session_id).await.is_none() {
        return false;
    }
    let new_confirmed = format!("{confirmed}{delta}");
    let desired = {
        let state = inner.native_composition.lock();
        // Preserve an already rendered provisional tail when the newly
        // certified words match it. Otherwise the certified rendering owns
        // the mutable range; no previously confirmed word can be removed.
        native_composition_rendering(&state.rendered, &new_confirmed, full_key)
            .unwrap_or_else(|| new_confirmed.clone())
    };
    pause_early_delivery_reserve_source(
        inner,
        session_id,
        new_confirmed,
        full_key.into(),
        source_key.into(),
        Some(consumed),
    );
    let sequence = endpoint_clock
        .lock()
        .begin_text_submission(Instant::now(), delta);
    let submitted = inner.native_composition.lock().submit(
        session_id,
        confirmed,
        &desired,
        inner.prefs.get().paste_shortcut,
    );
    let completion = if submitted {
        native_composition_wait(inner, session_id).await
    } else {
        None
    };
    let Some(completion) = completion else {
        pause_early_delivery_rollback(inner, session_id);
        endpoint_clock
            .lock()
            .settle_text_submission(sequence, false, None);
        return false;
    };
    pause_early_delivery_confirm(inner, session_id);
    inner
        .embedded_audio_pause_early_delivery
        .lock()
        .paste_delivered_session = Some(session_id);
    endpoint_clock.lock().settle_text_submission(
        sequence,
        true,
        Some(crate::insertion::PasteReceiptHandle::completed(completion)),
    );
    log::info!(
        "[native-composition] confirmed session_id={session_id} chars={} proof={:?}",
        desired.chars().count(),
        completion.outcome
    );
    true
}

/// Finalization owns the same mutable range. Failure never falls through to
/// a second full paste. Clipboard/history can still retain the final result.
async fn native_composition_finalize(
    inner: &Arc<Inner>,
    session_id: SessionId,
    confirmed: &str,
    desired: &str,
) -> Option<DeliverySubmission> {
    if !native_composition_written(inner, session_id) {
        return None;
    }
    let ready = native_composition_wait(inner, session_id).await.is_some();
    let sent = ready
        && inner.native_composition.lock().submit(
            session_id,
            confirmed,
            desired,
            inner.prefs.get().paste_shortcut,
        );
    let completion = if sent {
        native_composition_wait(inner, session_id).await
    } else {
        None
    };
    let accepted = completion.is_some();
    let mut state = inner.native_composition.lock();
    // Retire on every terminal outcome. A subsequent recording cannot borrow
    // this session's authority, even if its text happens to be identical.
    state.disabled = true;
    let submitted_text = if accepted {
        desired.into()
    } else {
        state.rendered.clone()
    };
    log::info!(
        "[native-composition] finalized session_id={session_id} confirmed={accepted} chars={}",
        submitted_text.chars().count()
    );
    Some(DeliverySubmission {
        status: if accepted {
            InsertStatus::Inserted
        } else {
            InsertStatus::SubmittedUnconfirmed
        },
        target_confirmed: accepted,
        route: DeliveryRoute::Paste,
        submitted_text: Some(submitted_text),
    })
}

fn native_composition_retire(inner: &Arc<Inner>, session_id: SessionId) {
    let mut state = inner.native_composition.lock();
    if state.session_id == Some(session_id) {
        // Explicit cancellation retires permission. Keep already rendered
        // characters, as with ordinary early insertion; never erase user text.
        state.disabled = true;
    }
}

#[cfg(test)]
mod native_composition_tests {
    use super::*;
    use crate::transcript_boundary::content_key;

    #[test]
    fn deferred_first_acquisition_can_resume_but_lost_written_range_cannot() {
        use crate::insertion::{PasteSubmission, SessionRangePermission as Permission};
        let session = crate::coordinator_state::new_session_id();
        let mut state = NativeCompositionState { session_id: Some(session), ..Default::default() };
        assert!(!state.submit_with(session, "", "是的", crate::types::PasteShortcut::CtrlV,
            |_, _, scope| {
                assert!(scope.previous_body.is_empty());
                PasteSubmission { status: InsertStatus::DeferredNoTarget, receipt: None,
                    range_permission: Permission::Unavailable }
            }));
        assert!(!state.disabled && !state.lost && !state.ever_sent);
        let (handle, sender) = crate::insertion::pending_receipt_handle_for_test(301);
        let ticket = handle.ticket();
        assert!(state.submit_with(session, "", "是的，就现在先这样子。", crate::types::PasteShortcut::CtrlV,
            |_, _, scope| {
                assert!(!scope.replace && scope.previous_body.is_empty());
                PasteSubmission { status: InsertStatus::PasteSent, receipt: Some(handle), range_permission: Permission::Owned }
            }));
        sender.send(crate::insertion::PasteReceiptCompletion { ticket, observed_at: Instant::now(),
            outcome: crate::insertion::PasteReceiptOutcome::Confirmed(crate::insertion::PasteReceiptProof::ExactEdit) }).unwrap();
        state.poll();
        assert_eq!(state.confirmed_readbacks, 1);
        assert!(!state.submit_with(session, "", "Continued body", crate::types::PasteShortcut::CtrlV,
            |_, _, scope| {
                assert_eq!(scope.previous_body, "是的，就现在先这样子。");
                assert!(scope.replace);
                PasteSubmission { status: InsertStatus::DeferredNoTarget, receipt: None, range_permission: Permission::Revoked }
            }));
        assert!(state.lost);
        assert!(!state.submit_with(session, "", "Continued body", crate::types::PasteShortcut::CtrlV,
            |_, _, _| panic!("a revoked written range cannot restart as a new full paste")));
    }

    #[tokio::test]
    #[ignore = "saved provider frames through actual delivery tick; no native writes, capture, models or cloud"]
    async fn saved_timeout_keeps_recording_and_all_recognized_body_pending() {
        use crate::asr::volcengine::{VolcengineCredentials, VolcengineStreamingASR};
        let path = std::path::PathBuf::from(std::env::var_os("LISTENER_FAILED_NATIVE_REPLAY").unwrap());
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let coordinator = crate::coordinator::Coordinator::new();
        let inner = &coordinator.inner;
        let session = crate::coordinator_state::new_session_id();
        { let mut active = inner.state.lock(); active.session_id = session; active.phase = SessionPhase::Listening; }
        let asr = Arc::new(VolcengineStreamingASR::new(VolcengineCredentials {
            app_id: "offline".into(), access_token: "offline".into(), resource_id: "offline".into(),
        }, Vec::new()).with_owner_activity_only(true));
        arm_accepted_automatic_wake_text_guard(inner, session, "开始录音".into(), 0, true);
        let endpoint = Arc::new(Mutex::new(SettledTargetEndpointClock { automatic_wake_session: true, ..Default::default() }));
        let (handle, sender) = crate::insertion::pending_receipt_handle_for_test(302);
        sender.send(crate::insertion::PasteReceiptCompletion { ticket: handle.ticket(), observed_at: Instant::now(),
            outcome: crate::insertion::PasteReceiptOutcome::TimedOut }).unwrap();
        *inner.native_composition.lock() = NativeCompositionState { session_id: Some(session),
            ever_sent: true, rendered: "是的".into(), receipt: Some(handle), ..Default::default() };
        let mut frames = 0;
        for entry in saved["trace"]["entries"].as_array().unwrap().iter()
            .filter(|entry| entry["stage"] == "normalized_result") {
            asr.apply_saved_native_result_for_test(&entry["provider_result"], entry["final_frame"] == true);
            update_live_body_delivery(inner, session, &asr, &endpoint).await;
            assert_eq!(inner.state.lock().phase, SessionPhase::Listening, "an output receipt cannot stop capture");
            assert!(endpoint.lock().last_body_delivery_at.is_none(), "timeout cannot manufacture body delivery");
            assert!(pause_early_delivery_session_state(inner, session).0.is_empty(), "failed draft cannot consume source words");
            frames += 1;
        }
        assert!(frames > 5);
        assert!(inner.native_composition.lock().lost);
        assert!(saved["finalCoordinatorText"].as_str().unwrap().chars().count() > 10);
        assert!(asr.provider_timed_transcript_snapshot().is_some_and(|timing| timing.text.chars().count() > 10),
            "recognized source remains available after output failure");
    }

    #[test]
    fn provisional_wake_prefix_is_never_editor_body() {
        let coordinator = crate::coordinator::Coordinator::new();
        let inner = &coordinator.inner;
        let session = crate::coordinator_state::new_session_id();
        arm_accepted_automatic_wake_text_guard(inner, session, "开始录音".into(), 0, true);
        for prefix in ["开始。", "开始录", "开始录音。"] {
            assert!(live_delivery_display_text(inner, session, prefix, true).is_none());
            assert!(!automatic_wake_body_started(inner, session));
        }
        assert_eq!(
            live_delivery_display_text(inner, session, "开始录音，明天上午。", true),
            Some("明天上午。".into())
        );
        assert!(automatic_wake_body_started(inner, session));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    #[ignore = "explicit original popup and saved complete native responses; actual editor writes, no capture/models/cloud"]
    async fn saved_provider_to_coordinator_to_editor_preserves_body_and_receipt_clock() {
        use crate::asr::volcengine::{VolcengineCredentials, VolcengineStreamingASR};
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        assert_eq!(std::env::var("LISTENER_ENABLE_REVISABLE_EDITOR").as_deref(), Ok("1"),
            "native experiments require explicit opt-in");
        assert_eq!(
            std::env::var("LISTENER_NATIVE_EDITOR_TEST").as_deref(),
            Ok("1")
        );
        let _ = env_logger::builder().is_test(true).try_init();
        let expected_pid: u32 = std::env::var("LISTENER_NATIVE_EDITOR_PID")
            .unwrap()
            .parse()
            .unwrap();
        let reader = || {
            let mut actual_pid = 0;
            unsafe {
                GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut actual_pid));
            }
            assert_eq!(
                actual_pid, expected_pid,
                "only the explicit original fixture may receive replay text"
            );
            let (reader, focus) = crate::selection::PasteReader::capture_with_focus();
            assert_eq!(focus, crate::selection::FocusedInputState::Editable);
            reader.unwrap()
        };
        assert_eq!(reader().document().unwrap(), "");
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_REVISABLE_REPLAY").unwrap());
        let archive: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("source.asr-trace.json")).unwrap())
                .unwrap();
        let coordinator = crate::coordinator::Coordinator::new();
        let inner = &coordinator.inner;
        let session = crate::coordinator_state::new_session_id();
        {
            let mut state = inner.state.lock();
            state.session_id = session;
            state.phase = SessionPhase::Listening;
        }
        arm_accepted_automatic_wake_text_guard(inner, session, "开始录音".into(), 0, true);
        let asr = Arc::new(
            VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "offline-fixture".into(),
                    access_token: "offline-fixture".into(),
                    resource_id: "offline-fixture".into(),
                },
                Vec::new(),
            )
            .with_owner_activity_only(true),
        );
        let endpoint = Arc::new(Mutex::new(SettledTargetEndpointClock {
            automatic_wake_session: true,
            ..Default::default()
        }));
        let mut first_editor_source_ms = None;
        let mut first_confirmed_source_ms = None;
        let mut frames = 0;
        let mut observations = Vec::new();
        for entry in archive["trace"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["stage"] == "normalized_result")
        {
            reader();
            asr.apply_saved_native_result_for_test(
                &entry["provider_result"],
                entry["final_frame"] == true,
            );
            if let Some(snapshot) = asr.revisable_owner_snapshot() {
                update_embedded_audio_partial_preview(inner, session, snapshot.text);
            }
            native_composition_update_tick(inner, session, &asr).await;
            if native_composition_written(inner, session) {
                assert!(native_composition_wait(inner, session).await.is_some());
                first_editor_source_ms.get_or_insert(entry["elapsed_ms"].as_u64().unwrap());
            }
            pause_early_delivery_tick(inner, session, &asr, &endpoint).await;
            endpoint.lock().poll_body_receipts();
            let confirmed = pause_early_delivery_session_state(inner, session).0;
            if !confirmed.is_empty() {
                first_confirmed_source_ms.get_or_insert(entry["elapsed_ms"].as_u64().unwrap());
            }
            let actual = reader().document().unwrap();
            if !confirmed.is_empty() {
                assert!(
                    actual.starts_with(&confirmed),
                    "confirmed owner body cannot be retracted"
                );
            }
            if confirmed.is_empty() {
                assert!(
                    endpoint.lock().last_body_delivery_at.is_none(),
                    "draft cannot start endpoint continuation"
                );
            }
            observations.push(
                serde_json::json!({"frame":entry["frame"], "sourceMs":entry["elapsed_ms"],
                "editor":actual, "confirmed":confirmed}),
            );
            frames += 1;
            tokio::time::sleep(
                STREAMING_COMPOSITION_MIN_UPDATE_INTERVAL + Duration::from_millis(10),
            )
            .await;
        }
        let confirmed = pause_early_delivery_session_state(inner, session).0;
        let desired = archive["finalCoordinatorText"].as_str().unwrap();
        let final_result = native_composition_finalize(inner, session, &confirmed, desired)
            .await
            .unwrap();
        assert!(
            final_result.target_confirmed,
            "final must receive an actual same-editor acknowledgement"
        );
        assert_eq!(
            reader().document().unwrap().trim_end_matches(['\r', '\n']),
            desired
        );
        assert_eq!(frames, 66);
        assert!(first_editor_source_ms.unwrap() < first_confirmed_source_ms.unwrap());
        assert!(endpoint.lock().last_body_delivery_at.is_some());
        for words in [
            "讨论产品的使用体验",
            "客户反馈",
            "已经完成的内容也要保留",
            "这就是我今天全部内容",
        ] {
            assert!(desired.contains(words));
        }
        std::fs::write(root.join("coordinator-native-replay.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "sourceSessionId":archive["sessionId"], "frames":frames,
            "firstEditorBodySourceMs":first_editor_source_ms, "firstConfirmedBodySourceMs":first_confirmed_source_ms,
            "actualFinalEditor":desired, "finalTargetConfirmed":final_result.target_confirmed,
            "draftDidNotStartEndpointClock":true, "committedBodyPreserved":true,
            "cloudCalls":0, "modelRuns":0, "installedAppSession":false,
            "scope":"saved provider responses through production coordinator and native worker; not live ASR timing or physical STOP", "observations":observations,
        })).unwrap()).unwrap();
        // Clear only this explicitly owned test field for the next native
        // same-popup regression; no focus changes or keyboard copy fallback.
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId,
        };
        let _reader = reader();
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let element = unsafe { automation.GetFocusedElement() }.unwrap();
        let value: IUIAutomationValuePattern =
            unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.unwrap();
        unsafe { value.SetValue(&windows::core::BSTR::from("")) }.unwrap();
    }

    #[test]
    fn drafts_do_not_start_body_clock_and_identical_confirmation_uses_fresh_native_receipt() {
        use crate::insertion::{
            pending_receipt_handle_for_test, PasteReceiptCompletion, PasteReceiptOutcome,
            PasteReceiptProof,
        };
        let now = Instant::now();
        let (receipt, sender) = pending_receipt_handle_for_test(9_001);
        let ticket = receipt.ticket();
        let mut draft = NativeCompositionState {
            ever_sent: true,
            receipt: Some(receipt),
            rendered: "Words still being revised".into(),
            ..Default::default()
        };
        let mut clock = SettledTargetEndpointClock {
            automatic_wake_session: true,
            ..Default::default()
        };
        sender
            .send(PasteReceiptCompletion {
                ticket,
                observed_at: now,
                outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
            })
            .unwrap();
        draft.poll();
        assert!(
            clock.last_body_delivery_at.is_none(),
            "draft readback is not confirmed-body delivery"
        );
        let commit_at = now + Duration::from_millis(1_000);
        let observed_at = commit_at + Duration::from_millis(45);
        let sequence = clock.begin_text_submission(commit_at, "Words still being revised");
        clock.settle_text_submission(
            sequence,
            true,
            Some(crate::insertion::PasteReceiptHandle::completed(
                PasteReceiptCompletion {
                    ticket,
                    observed_at,
                    outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::OwnedSessionRange),
                },
            )),
        );
        clock.poll_body_receipts();
        assert_eq!(clock.last_body_delivery_at, Some(observed_at));
        let punctuation = clock.begin_text_submission(observed_at + Duration::from_secs(1), "...");
        assert!(punctuation.is_none());
        assert_eq!(
            clock.last_body_delivery_at,
            Some(observed_at),
            "punctuation cannot postpone the full three seconds"
        );
    }

    #[test]
    fn mutable_tail_can_correct_words_without_retracting_committed_text() {
        let committed = "下午请把客户反馈整理出来。";
        assert_eq!(
            native_composition_rendering(
                &format!("{committed}会议一共12个人"),
                committed,
                &content_key(committed)
            )
            .unwrap(),
            format!("{committed}会议一共12个人")
        );
        assert_eq!(
            native_composition_rendering(
                &format!("{committed}会一共12个人。"),
                committed,
                &content_key(committed)
            )
            .unwrap(),
            format!("{committed}会一共12个人。")
        );
        assert!(
            native_composition_rendering(
                "下午整理资料。会一共12个人。",
                committed,
                &content_key(committed)
            )
            .is_none(),
            "provider rewrites cannot replace committed body"
        );
    }

    #[test]
    fn rendering_preserves_delivered_display_and_maps_the_provider_coordinate() {
        let committed = "We meet at 9. ";
        let source = "we meet at nine";
        assert_eq!(
            native_composition_rendering(
                "we meet at nine. Keep the feedback.",
                committed,
                &content_key(source)
            ),
            Some("We meet at 9. Keep the feedback.".into())
        );
        assert_eq!(
            native_composition_rendering("静かな声で話しています", "", ""),
            Some("静かな声で話しています".into())
        );
    }

    #[tokio::test]
    async fn lost_draft_range_blocks_terminal_full_replay_and_keeps_committed_body() {
        let coordinator = crate::coordinator::Coordinator::new();
        let inner = &coordinator.inner;
        let session = crate::coordinator_state::new_session_id();
        pause_early_delivery_reserve(
            inner,
            session,
            "Confirmed owner words. ".into(),
            content_key("Confirmed owner words."),
            content_key("Confirmed owner words."),
        );
        pause_early_delivery_confirm(inner, session);
        *inner.native_composition.lock() = NativeCompositionState {
            session_id: Some(session),
            ever_sent: true,
            lost: true,
            rendered: "Confirmed owner words. Provisional tail".into(),
            ..Default::default()
        };
        let result = native_composition_finalize(
            inner,
            session,
            "Confirmed owner words. ",
            "Confirmed owner words. Final tail.",
        )
        .await
        .unwrap();
        assert!(!result.target_confirmed);
        assert_eq!(result.status, InsertStatus::SubmittedUnconfirmed);
        assert_eq!(
            result.submitted_text.as_deref(),
            Some("Confirmed owner words. Provisional tail")
        );
        assert_eq!(
            pause_early_delivery_session_state(inner, session).0,
            "Confirmed owner words. "
        );
        assert!(
            inner.native_composition.lock().disabled,
            "terminal loss retires mutation permission"
        );
    }
}
