//! A hardware question uses the existing BLE capture and endpoint, but owns a
//! question card instead of an editor. The answer lane outlives capture without
//! blocking the BLE packet actor, and every event remains generation-bound.
use super::*;
use crate::voice_command::VoiceAction;

pub(super) fn begin_card(inner: &Arc<Inner>, owner: SessionId) {
    let front_app = inner.state.lock().front_app.clone();
    {
        let mut qa = inner.qa_state.lock();
        if qa.capture_owner == Some(owner) && qa.session_id == owner { return; }
        // An explicit follow-up from this card keeps its conversation; a new
        // voice command starts a fresh question via the caller's reset flag.
        qa.panel_visible = true;
        qa.phase = QaPhase::Recording;
        qa.cancelled = false;
        qa.session_id = owner;
        qa.capture_owner = Some(owner);
        qa.capture_visible = false;
        qa.input_source = qa::QaInputSource::Listener;
        qa.selection = None;
        qa.front_app = front_app;
    }
    inner.qa_stream_cancelled.store(false, Ordering::SeqCst);
    if let Some(app) = inner.app.lock().clone() {
        crate::show_qa_window(&app, "recording");
        if let Some(capsule) = app.get_webview_window("capsule") { let _ = capsule.hide(); }
        let messages = inner.qa_state.lock().messages.clone();
        let _ = app.emit_to("qa", "qa:state", serde_json::json!({
            "kind":"recording", "input_source":"listener", "session_id":owner.to_string(), "messages":messages,
        }));
    }
}

pub(super) fn reset_voice_conversation(inner: &Arc<Inner>) {
    let mut qa = inner.qa_state.lock();
    qa.messages.clear();
    qa.last_answered_selection = None;
    qa.selection = None;
}

pub(super) fn acknowledge_visible(inner: &Arc<Inner>, owner: SessionId) {
    {
        let mut qa = inner.qa_state.lock();
        if qa.session_id != owner || qa.capture_owner != Some(owner) || !qa.panel_visible { return; }
        qa.capture_visible = true;
    }
    super::acknowledge_automatic_wake_capsule_visible(inner, owner);
}

/// Called only for the exact capture owner. A late close/error from an old
/// question cannot take over a newer dictation or a newer question card.
pub(super) fn present_capture(
    inner: &Arc<Inner>, owner: SessionId, state: CapsuleState,
    level: f32, message: Option<String>,
) {
    let mut qa = inner.qa_state.lock();
    if qa.session_id != owner || qa.capture_owner != Some(owner) || !qa.panel_visible { return; }
    if state == CapsuleState::Recording {
        if let Some(app) = inner.app.lock().clone() {
            let _ = app.emit_to("qa", "qa:level", serde_json::json!({"level":level}));
        }
    } else {
        let kind = match state {
                CapsuleState::Transcribing | CapsuleState::Polishing => {
                    qa.phase = QaPhase::Processing; "loading"
                }
                CapsuleState::Error => { qa.phase = QaPhase::Idle; qa.capture_owner = None; "error" }
                CapsuleState::Cancelled => { qa.phase = QaPhase::Idle; qa.capture_owner = None; "idle" }
                _ => return,
        };
        if let Some(app) = inner.app.lock().clone() {
            let _ = app.emit_to("qa", "qa:state", serde_json::json!({
                "kind":kind,"input_source":"listener","error":message,"messages":qa.messages.clone(),
            }));
        }
    }
}

pub(super) fn handoff_answer(inner: &Arc<Inner>, owner: SessionId, question: String, duration_ms: u64) {
    {
        let mut qa = inner.qa_state.lock();
        if qa.session_id != owner || qa.capture_owner != Some(owner) || qa.cancelled { return; }
        qa.phase = QaPhase::Processing;
        qa.capture_owner = None;
    }
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn(async move {
        let _ = answer_transcribed_qa_question(&inner, owner, &question, duration_ms).await;
    });
}

pub(super) async fn record_edge(inner: &Arc<Inner>) -> Result<(), String> {
    let (phase, owner) = {
        let qa = inner.qa_state.lock(); (qa.phase, qa.capture_owner)
    };
    match (phase, owner) {
        (QaPhase::Recording, Some(owner)) => {
            let current = inner.state.lock().session_id == owner;
            if !current { return Err("qa_action_failed".into()); }
            request_embedded_ble_recording_stop_from_host(inner, "question_card_stop").await?;
        }
        (QaPhase::Idle, _) => {
            ensure_qa_service_setup()?;
            if inner.state.lock().phase != SessionPhase::Idle { return Err("qa_dictation_busy".into()); }
            super::request_embedded_ble_recording_start_for_action(
                inner, "question_card_followup", VoiceAction::Question).await?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn question_card_rejects_stale_capture_error_and_visibility() {
        let coordinator = Coordinator::new();
        let old = new_session_id();
        let current = new_session_id();
        begin_card(&coordinator.inner, current);
        acknowledge_visible(&coordinator.inner, old);
        present_capture(&coordinator.inner, old, CapsuleState::Error, 0.0, Some("old".into()));
        {
            let qa = coordinator.inner.qa_state.lock();
            assert_eq!(qa.capture_owner, Some(current));
            assert_eq!(qa.phase, QaPhase::Recording);
            assert!(!qa.capture_visible);
        }
        acknowledge_visible(&coordinator.inner, current);
        assert!(coordinator.inner.qa_state.lock().capture_visible);
        present_capture(&coordinator.inner, current, CapsuleState::Error, 0.0, Some("current".into()));
        let qa = coordinator.inner.qa_state.lock();
        assert_eq!(qa.phase, QaPhase::Idle);
        assert_eq!(qa.capture_owner, None);
    }
}
