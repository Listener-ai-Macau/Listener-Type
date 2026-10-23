use std::sync::Arc;

use tauri::Emitter;

use crate::coordinator_state::{initial_session_id, new_session_id, SessionId, SessionPhase};
use crate::selection::SelectionContext;
use crate::types::CapsuleState;

use super::{
    begin_qa_session, cancel_qa_session, capture_frontmost_app, emit_capsule_for_session,
    end_qa_session, Inner,
};

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
        }
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
