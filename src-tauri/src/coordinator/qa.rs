use std::sync::Arc;

use tauri::Emitter;

use crate::coordinator_state::{initial_session_id, new_session_id, SessionId, SessionPhase};
use crate::selection::SelectionContext;
use crate::types::CapsuleState;

use super::{
    begin_qa_session, cancel_qa_session, capture_frontmost_app, emit_capsule_for_session,
    end_qa_session, Inner,
};

pub(super) fn validate_qa_setup(status: &crate::types::CredentialsStatus) -> Result<(), String> {
    if !status.volcengine_configured {
        Err("qa_speech_setup_required".into())
    } else if !status.llm_configured {
        Err("qa_answer_setup_required".into())
    } else {
        Ok(())
    }
}

// QA and dictation share the capsule, but have independent recording lifecycles.
// A QA close/error/level event must never take display ownership from dictation.
// Bind every admitted QA request to the observed idle dictation generation so
// the presentation cache can also reject a recording started after this check.
pub(super) fn emit_qa_capsule(
    inner: &Arc<Inner>,
    state: CapsuleState,
    level: f32,
    message: Option<String>,
) -> Option<SessionId> {
    let owner = {
        let dictation = inner.state.lock();
        if dictation.phase != SessionPhase::Idle {
            return None;
        }
        dictation.session_id
    };
    emit_capsule_for_session(inner, owner, state, level, 0, message, None);
    Some(owner)
}

pub(super) fn schedule_qa_capsule_idle(
    inner: &Arc<Inner>,
    delay_ms: u64,
    qa_owner: SessionId,
    dictation_owner: SessionId,
) -> tauri::async_runtime::JoinHandle<bool> {
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        let qa = inner.qa_state.lock();
        if qa.session_id != qa_owner || qa.phase != QaPhase::Idle {
            return false;
        }
        {
            let dictation = inner.state.lock();
            if dictation.session_id != dictation_owner || dictation.phase != SessionPhase::Idle {
                return false;
            }
        }
        emit_capsule_for_session(
            &inner, dictation_owner, CapsuleState::Idle, 0.0, 0, None, None,
        );
        true
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QaPhase {
    Idle,
    Recording,
    Processing,
}

pub(super) struct QaSessionState {
    pub(super) phase: QaPhase,
    pub(super) cancelled: bool,
    pub(super) selection: Option<SelectionContext>,
    pub(super) front_app: Option<String>,
    /// 用于忽略迟到的 RMS / runtime error。
    pub(super) session_id: SessionId,
    /// QA 浮窗是否被用户钉住（pinned）。pinned=true 时不自动隐藏。
    pub(super) pinned: bool,
    /// 浮窗是否对用户可见。Cmd+Shift+; 边沿 toggle 此 flag；
    /// 主听写 hotkey（rightOption）边沿来时，看这个 flag 决定是走 QA 还是走 dictation。
    /// 详见 issue #118 v2。
    pub(super) panel_visible: bool,
    /// 多轮对话累积。每轮 user→assistant 加两条；关浮窗清空。
    pub(super) messages: Vec<crate::types::QaChatMessage>,
    /// Last selection belonging to a successfully answered question. Failed
    /// requests do not advance this baseline, so retry includes the new text.
    pub(super) last_answered_selection: Option<String>,
}

impl Default for QaSessionState {
    fn default() -> Self {
        Self {
            phase: QaPhase::Idle,
            cancelled: false,
            selection: None,
            front_app: None,
            session_id: initial_session_id(),
            pinned: false,
            panel_visible: false,
            messages: Vec::new(),
            last_answered_selection: None,
        }
    }
}

impl QaSessionState {
    pub(super) fn append_question(&mut self, question: &str) {
        let content = super::qa_question_context::question_user_content(
            self.messages.is_empty(),
            self.last_answered_selection.as_deref(),
            self.selection.as_ref().map(|selection| selection.text.as_str()),
            question,
        );
        self.messages.push(crate::types::QaChatMessage { role: "user".into(), content });
    }

    pub(super) fn append_answer(&mut self, answer: &str) {
        // Advance only after this answer succeeds. Cancellation/error removes
        // the pending user message and must leave the previous baseline intact.
        if let Some(selected) = self.selection.as_ref().map(|selection| selection.text.trim()) {
            if !selected.is_empty() {
                self.last_answered_selection = Some(selected.to_string());
            }
        }
        self.messages.push(crate::types::QaChatMessage {
            role: "assistant".into(), content: answer.to_string(),
        });
    }
}

pub(super) async fn handle_qa_hotkey_pressed(inner: &Arc<Inner>) {
    // QA hotkey（默认 Cmd+Shift+;）现在只 toggle 浮窗可见性。
    // 浮窗内的录音 / 提问由 Option 边沿驱动（handle_pressed_edge → handle_qa_option_edge）。
    let visible = inner.qa_state.lock().panel_visible;
    log::info!("[coord] QA hotkey edge (panel_visible={visible})");
    if visible {
        close_qa_panel(inner);
    } else {
        open_qa_panel(inner);
    }
}

pub(super) async fn handle_qa_option_edge(inner: &Arc<Inner>) {
    let phase = inner.qa_state.lock().phase;
    log::info!("[coord] QA option edge (phase={phase:?})");
    match phase {
        QaPhase::Idle => {
            let _ = begin_qa_session(inner).await;
        }
        QaPhase::Recording => {
            let _ = end_qa_session(inner).await;
        }
        // Processing 阶段再次按键忽略（避免与正在跑的 LLM 冲突）。
        QaPhase::Processing => {}
    }
}

pub(super) fn open_qa_panel(inner: &Arc<Inner>) {
    {
        let mut state = inner.qa_state.lock();
        state.session_id = new_session_id();
        state.panel_visible = true;
        state.phase = QaPhase::Idle;
        state.cancelled = false;
        state.messages.clear();
        state.last_answered_selection = None;
        state.selection = None;
        state.front_app = capture_frontmost_app();
    }
    // 主听写 phase 是 Idle 才需要 sweep capsule —— 这里的语义是清掉「上一次 dictation
    // Done 状态残留」的 message / insertedChars，让 QA 自己的 capsule 状态从干净起跑
    // （否则 capsule UI 会出现 "已粘贴这个 0" 之类把上一次 inserted_chars 错误复用的
    // 显示）。但如果 dictation 当前正处于 Recording / Polishing / Inserting / Done toast
    // 显示中，强行 emit Idle 会把用户没看完的反馈抹掉、或者把 Polishing 中的进度条
    // 卡死。审计 3.3.4。
    emit_qa_capsule(inner, CapsuleState::Idle, 0.0, None);
    if let Some(app) = inner.app.lock().clone() {
        crate::show_qa_window(&app, "idle");
        let _ = app.emit_to(
            "qa",
            "qa:state",
            serde_json::json!({
                "kind": "idle",
                "messages": Vec::<crate::types::QaChatMessage>::new(),
            }),
        );
    }
    log::info!("[coord] QA panel opened (awaiting Option to record)");
}

pub(super) fn close_qa_panel(inner: &Arc<Inner>) {
    cancel_qa_session(inner);
    {
        let mut state = inner.qa_state.lock();
        state.session_id = new_session_id();
        state.panel_visible = false;
        state.pinned = false;
        state.messages.clear();
        state.last_answered_selection = None;
        state.selection = None;
        state.front_app = None;
        state.phase = QaPhase::Idle;
        state.cancelled = false;
    }
    if let Some(app) = inner.app.lock().clone() {
        crate::hide_qa_window(&app);
    }
    // 胶囊一同收掉，避免浮窗关了胶囊还在显示。
    emit_qa_capsule(inner, CapsuleState::Idle, 0.0, None);
    log::info!("[coord] QA panel closed, history cleared");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::Coordinator;

    #[test]
    fn qa_start_requires_both_services_shown_by_the_ui() {
        for (speech, answers, expected) in [
            (false, false, Some("qa_speech_setup_required")),
            (false, true, Some("qa_speech_setup_required")),
            (true, false, Some("qa_answer_setup_required")),
            (true, true, None),
        ] {
            let status = crate::types::CredentialsStatus {
                active_asr_provider: "volcengine".into(),
                active_llm_provider: "deepseek".into(),
                asr_configured: speech,
                llm_configured: answers,
                volcengine_configured: speech,
                ark_configured: answers,
            };
            assert_eq!(validate_qa_setup(&status).err().as_deref(), expected);
        }
    }

    fn selected(text: &str) -> Option<SelectionContext> {
        Some(SelectionContext { text: text.into(), source_app: None })
    }

    #[test]
    fn qa_changed_selection_survives_failed_answer_and_retry() {
        let coordinator = Coordinator::new();
        let owner = new_session_id();
        {
            let mut state = coordinator.inner.qa_state.lock();
            state.panel_visible = true;
            state.session_id = owner;
            state.selection = selected("原来的条款");
            state.append_question("解释这一条");
            state.append_answer("第一条回答");
            state.selection = selected("新的合同条款");
            state.phase = QaPhase::Processing;
            state.append_question("这一条呢？");
            assert!(state.messages.last().unwrap().content.contains("新的合同条款"));
            assert_eq!(state.last_answered_selection.as_deref(), Some("原来的条款"));
        }
        assert!(super::super::finish_qa_for_session(&coordinator.inner, owner, Some("test failure".into())));
        let mut state = coordinator.inner.qa_state.lock();
        assert_eq!(state.messages.len(), 2, "real error closeout removes only the unanswered question");
        assert_eq!(state.last_answered_selection.as_deref(), Some("原来的条款"));
        state.selection = selected("新的合同条款");
        state.append_question("这一条呢？");
        assert!(state.messages.last().unwrap().content.contains("新的合同条款"));
        state.append_answer("第二条回答");
        assert_eq!(state.last_answered_selection.as_deref(), Some("新的合同条款"));
    }

    #[test]
    fn qa_unselected_followup_preserves_answered_selection() {
        let mut state = QaSessionState::default();
        state.selection = selected("参考正文");
        state.append_question("概括");
        state.append_answer("概括结果");
        state.selection = None;
        state.append_question("再简短一点");
        assert_eq!(state.messages.last().unwrap().content, "再简短一点");
        state.append_answer("更短的结果");
        assert_eq!(state.last_answered_selection.as_deref(), Some("参考正文"));
        state.selection = selected(" 参考正文\n");
        state.append_question("举例");
        assert_eq!(state.messages.last().unwrap().content, "举例");
    }

    #[test]
    fn qa_selection_baseline_is_cleared_when_panel_is_closed_or_reopened() {
        let coordinator = Coordinator::new();
        coordinator.inner.qa_state.lock().last_answered_selection = Some("旧窗口选区".into());
        close_qa_panel(&coordinator.inner);
        assert!(coordinator.inner.qa_state.lock().last_answered_selection.is_none());
        coordinator.inner.qa_state.lock().last_answered_selection = Some("旧窗口选区".into());
        open_qa_panel(&coordinator.inner);
        assert!(coordinator.inner.qa_state.lock().last_answered_selection.is_none());
        assert!(coordinator.inner.qa_state.lock().messages.is_empty());
    }
}
