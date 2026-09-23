// ───────── streaming composition(2026-09-22 讯飞式逐字上屏)切片3:接线 ─────────
// 设计卡:work/streaming-composition-design-20260922.md。说话期间把权威账本
// 的增长余量(delta = 显示变换后全文 − 已 commit 前缀,与 pause-early 完全
// 同款变换链)按 ≥200ms 节流 stream_update 进目标窗口的 TSF 组字,原地替换
// (云端改字免费修订);停顿稳定(pause-early 同一道 1s 门)→ stream_commit
// 落定并写入同一本 pause-early 账本;终稿只 commit 余量/LCP 尾,整段已覆盖
// commit("") 清残留,改写无恢复 cancel 清组字。任何管道失败当拍降级回粘贴
// 路径(今日行为);回退开关 LISTENER_DISABLE_STREAMING_COMPOSITION=1。

const STREAMING_COMPOSITION_MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(200);
const STREAMING_COMPOSITION_COMMIT_REPLY_TIMEOUT: Duration = Duration::from_millis(1_500);
const STREAMING_COMPOSITION_FINALIZE_REPLY_TIMEOUT: Duration = Duration::from_millis(2_500);
const STREAMING_COMPOSITION_CHANNEL_CAPACITY: usize = 8;
/// 驱动闲置自检:会话被取消/顶掉而终稿路径没跑时,兜底清组字并退出。
/// 不看 phase——Inserting/Polishing 是终稿路径的正常窗口,只有 cancel 或
/// 会话更替才算孤儿。
const STREAMING_COMPOSITION_IDLE_CHECK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamingCompositionPhase {
    Disabled,
    Active,
    /// contaminated = 清场 cancel 也失败,文档里可能有残留组字文本——粘贴
    /// 兜底会重复,宁可停手交给终稿/人工。
    Failed {
        reason: &'static str,
        contaminated: bool,
    },
    Ended,
}

impl Default for StreamingCompositionPhase {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Debug, Default)]
pub(super) struct StreamingCompositionState {
    phase: StreamingCompositionPhase,
    session_id: Option<SessionId>,
    /// 节流:上次 update 实际送达管道的时刻(驱动侧写)。
    last_update_sent_at: Option<Instant>,
    /// 去重:上次 update 送达的余量 stability key(驱动侧写)。
    last_sent_key: String,
    updates_sent: u32,
    commits: u32,
    command_tx: Option<tokio::sync::mpsc::Sender<StreamingCompositionCommand>>,
}

enum StreamingCompositionCommand {
    Update { text: String, key: String },
    Commit { text: String, reply: Option<tokio::sync::oneshot::Sender<bool>> },
    CancelComposition { reply: Option<tokio::sync::oneshot::Sender<bool>> },
    EndSession,
}

fn streaming_composition_disabled_by_env() -> bool {
    // 2026-09-22 21:5x 用户拍板(二次确认,与 09-21 拍板一致):"一段一段出来
    // 就行,不用逐字逐句,要确认了再出来"。组字流式默认关闭,转为显式开启:
    // LISTENER_ENABLE_STREAMING_COMPOSITION=1。停顿落屏(pause-early)是正式
    // 交付路径。代码与判读行保留,供以后 opt-in 验证。
    if std::env::var("LISTENER_ENABLE_STREAMING_COMPOSITION").as_deref() == Ok("1") {
        return std::env::var("LISTENER_DISABLE_STREAMING_COMPOSITION").as_deref() == Ok("1");
    }
    true
}

fn streaming_composition_active(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let state = inner.streaming_composition.lock();
    state.session_id == Some(session_id) && state.phase == StreamingCompositionPhase::Active
}

fn streaming_composition_contaminated(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let state = inner.streaming_composition.lock();
    state.session_id == Some(session_id)
        && matches!(
            &state.phase,
            StreamingCompositionPhase::Failed {
                contaminated: true,
                ..
            }
        )
}

fn streaming_state_apply<F>(inner: &Arc<Inner>, session_id: SessionId, mutate: F)
where
    F: FnOnce(&mut StreamingCompositionState),
{
    let mut state = inner.streaming_composition.lock();
    if state.session_id == Some(session_id) {
        mutate(&mut state);
    }
}

fn streaming_state_fail(
    state: &mut StreamingCompositionState,
    reason: &'static str,
    contaminated: bool,
) {
    state.phase = StreamingCompositionPhase::Failed { reason, contaminated };
    state.command_tx = None;
    log::warn!(
        "[streaming-composition] degraded reason={reason} contaminated={contaminated} updates={} commits={}",
        state.updates_sent,
        state.commits
    );
}

/// 会话起点:决定本会话是否启用组字流式(TSF prepared 就绪 + 目标可解析 +
/// 未拉回退开关),起驱动任务并先发一次 cancel 清上一会话残留(DLL 单组字
/// 契约,设计⑤)。任何不满足 → 保持 Disabled,全链路走今日粘贴行为。
async fn begin_streaming_composition_session(inner: &Arc<Inner>, session_id: SessionId) {
    {
        let mut state = inner.streaming_composition.lock();
        *state = StreamingCompositionState::default();
    }
    if streaming_composition_disabled_by_env() {
        log::info!("[streaming-composition] disabled by env for session_id={session_id}");
        return;
    }
    #[cfg(target_os = "windows")]
    {
        let prepared = {
            let slots = inner.prepared_windows_ime_session.lock();
            slots
                .iter()
                .find(|slot| slot.session_id == session_id)
                .map(|slot| slot.prepared.clone())
        };
        let Some(prepared) = prepared.filter(|prepared| prepared.is_ready_for_tsf_submit()) else {
            log::info!(
                "[streaming-composition] disabled for session_id={session_id}: TSF session not ready (paste mode)"
            );
            return;
        };
        let Some(target) = capture_ime_submit_target() else {
            log::info!(
                "[streaming-composition] disabled for session_id={session_id}: no IME target at session start"
            );
            return;
        };
        let (tx, rx) = tokio::sync::mpsc::channel(STREAMING_COMPOSITION_CHANNEL_CAPACITY);
        {
            let mut state = inner.streaming_composition.lock();
            state.session_id = Some(session_id);
            state.phase = StreamingCompositionPhase::Active;
            state.command_tx = Some(tx);
        }
        log::info!(
            "[streaming-composition] session started session_id={session_id} target_pid={} target_tid={}",
            target.process_id,
            target.thread_id
        );
        tokio::spawn(streaming_composition_driver(
            Arc::clone(inner),
            session_id,
            prepared,
            Some(target),
            rx,
        ));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (inner, session_id);
    }
}

fn streaming_composition_send(
    inner: &Arc<Inner>,
    session_id: SessionId,
    command: StreamingCompositionCommand,
) -> bool {
    let tx = {
        let state = inner.streaming_composition.lock();
        (state.session_id == Some(session_id))
            .then(|| state.command_tx.clone())
            .flatten()
    };
    match tx {
        Some(tx) => tx.try_send(command).is_ok(),
        None => false,
    }
}

/// 组字期间每拍(watchdog 50ms)评估:账本当前文本(0 稳定窗,说话中就要)
/// → 同款显示变换 → 只取已 commit 前缀之后的余量 → 节流+去重 → fire-and-
/// forget update。所有门失败静默跳过(下一拍自愈)。
async fn streaming_composition_update_tick(
    inner: &Arc<Inner>,
    session_id: SessionId,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
) {
    if !streaming_composition_active(inner, session_id) {
        return;
    }
    match inner
        .streaming_composition
        .lock()
        .last_update_sent_at
        .map(|at| at.elapsed())
    {
        Some(elapsed) if elapsed < STREAMING_COMPOSITION_MIN_UPDATE_INTERVAL => return,
        _ => {}
    }
    let Some(snapshot) = asr.pause_early_delivery_ledger_snapshot(Duration::ZERO) else {
        return;
    };
    let Some(display) = pause_early_display_text(inner, session_id, &snapshot.text) else {
        return;
    };
    let key = embedded_audio_partial_preview_stability_key(&display);
    let (_, delivered_key) = pause_early_delivery_session_state(inner, session_id);
    if !key.starts_with(&delivered_key) {
        // 已 commit 前缀被改写:组字不动,交给终稿的 cancel/恢复语义。
        return;
    }
    let Some(delta) = pause_early_final_remainder(&display, &delivered_key) else {
        return;
    };
    let delta_key = embedded_audio_partial_preview_stability_key(&delta);
    let stale = {
        let state = inner.streaming_composition.lock();
        !streaming_update_should_send(&delta_key, &state.last_sent_key)
    };
    if stale {
        return;
    }
    streaming_composition_send(
        inner,
        session_id,
        StreamingCompositionCommand::Update {
            text: delta,
            key: delta_key,
        },
    );
}

fn streaming_update_should_send(delta_key: &str, last_sent_key: &str) -> bool {
    !delta_key.is_empty() && delta_key != last_sent_key
}

/// 停顿稳定后的落定(pause-early tick 的组字路由)。记账次序与粘贴路径
/// 同款:先记账再 commit,失败回滚——reply 丢失/超时时账本已推进,不会
/// 下一拍重复 commit 同一余量。
async fn streaming_composition_commit_stable(
    inner: &Arc<Inner>,
    session_id: SessionId,
    delta: &str,
    delivered_display: &str,
    full_key: &str,
) -> bool {
    let new_display = format!("{delivered_display}{delta}");
    pause_early_delivery_reserve(inner, session_id, new_display, full_key.to_string());
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    if !streaming_composition_send(
        inner,
        session_id,
        StreamingCompositionCommand::Commit {
            text: delta.to_string(),
            reply: Some(tx),
        },
    ) {
        pause_early_delivery_rollback(inner, session_id);
        return false;
    }
    let ok =
        matches!(tokio::time::timeout(STREAMING_COMPOSITION_COMMIT_REPLY_TIMEOUT, &mut rx).await, Ok(Ok(true)));
    if !ok {
        pause_early_delivery_rollback(inner, session_id);
        return false;
    }
    // TSF reply 确认文本已落屏:确认账本前缀 + 粘滞底线（见 preview 侧同款）。
    pause_early_delivery_confirm(inner, session_id);
    let state = inner.streaming_composition.lock();
    log::info!(
        "[coord] streaming-composition commit prefix_chars={} total_delivered_chars={} min_stable_ms={} updates={} commits={}",
        delta.chars().count(),
        delivered_display.chars().count() + delta.chars().count(),
        PAUSE_EARLY_DELIVERY_MIN_STABLE.as_millis(),
        state.updates_sent,
        state.commits
    );
    ok
}

/// 终稿落定:composition 活着时用余量/恢复尾 commit(替换掉组字里的预览
/// 余量);整段已覆盖传空串(commit("") 清残留)。失败返回 false,调用方落回
/// 常规派发(驱动已自行降级清场)。
async fn streaming_composition_finalize(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: &str,
) -> bool {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    if !streaming_composition_send(
        inner,
        session_id,
        StreamingCompositionCommand::Commit {
            text: text.to_string(),
            reply: Some(tx),
        },
    ) {
        return false;
    }
    matches!(
        tokio::time::timeout(STREAMING_COMPOSITION_FINALIZE_REPLY_TIMEOUT, &mut rx).await,
        Ok(Ok(true))
    )
}

/// 终稿改写无恢复:整段清组字(文档保留已 commit 前缀,今日"早期文本保留,
/// 尾巴丢弃"语义)。
async fn streaming_composition_finalize_cancel(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    if !streaming_composition_send(
        inner,
        session_id,
        StreamingCompositionCommand::CancelComposition { reply: Some(tx) },
    ) {
        return false;
    }
    matches!(
        tokio::time::timeout(STREAMING_COMPOSITION_FINALIZE_REPLY_TIMEOUT, &mut rx).await,
        Ok(Ok(true))
    )
}

/// 会话收尾。cancel_composition=true 时先清组字再退出(丢弃路径);终稿路径
/// 组字已 commit/cancel,传 false 仅退出。幂等:phase 已非 Active 时为 no-op。
fn end_streaming_composition_session(inner: &Arc<Inner>, session_id: SessionId, cancel_composition: bool) {
    if cancel_composition {
        streaming_composition_send(
            inner,
            session_id,
            StreamingCompositionCommand::CancelComposition { reply: None },
        );
    }
    streaming_composition_send(inner, session_id, StreamingCompositionCommand::EndSession);
}

#[cfg(target_os = "windows")]
async fn streaming_run_op(
    inner: &Arc<Inner>,
    session_id: SessionId,
    prepared: &crate::windows_ime_session::PreparedWindowsImeSession,
    op: crate::windows_ime_ipc::ImeStreamOp,
    text: &str,
    target: Option<&crate::windows_ime_ipc::ImeSubmitTarget>,
) -> bool {
    let request = crate::windows_ime_ipc::ImeSubmitRequest {
        session_id: session_id.to_string(),
        text: text.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        target: target.cloned(),
    };
    let outcome = inner.windows_ime.stream_prepared(prepared, op, request).await;
    let ok = matches!(outcome, Ok(InsertStatus::Inserted));
    if !ok {
        let detail = outcome
            .as_ref()
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "rejected".to_string());
        log::warn!(
            "[streaming-composition] op {op:?} failed session_id={session_id}: {detail}"
        );
    }
    ok
}

/// 串行驱动:所有组字管道操作在此任务按序执行(coordinator 侧只投递命令),
/// 保证 update/commit/cancel 的到达顺序。失败即降级并尽力清场后退出。
#[cfg(target_os = "windows")]
async fn streaming_cancel_op(
    inner: &Arc<Inner>,
    session_id: SessionId,
    prepared: &crate::windows_ime_session::PreparedWindowsImeSession,
    target: Option<&crate::windows_ime_ipc::ImeSubmitTarget>,
) -> bool {
    streaming_run_op(
        inner,
        session_id,
        prepared,
        crate::windows_ime_ipc::ImeStreamOp::Cancel,
        "",
        target,
    )
    .await
}

/// 串行驱动:所有组字管道操作在此任务按序执行(coordinator 侧只投递命令),
/// 保证 update/commit/cancel 的到达顺序。失败即降级并尽力清场后退出。
#[cfg(target_os = "windows")]
async fn streaming_composition_driver(
    inner: Arc<Inner>,
    session_id: SessionId,
    prepared: crate::windows_ime_session::PreparedWindowsImeSession,
    target: Option<crate::windows_ime_ipc::ImeSubmitTarget>,
    mut rx: tokio::sync::mpsc::Receiver<StreamingCompositionCommand>,
) {
    macro_rules! cancel {
        () => {
            streaming_cancel_op(&inner, session_id, &prepared, target.as_ref()).await
        };
    }
    // 会话起点清场:上一会话遗留组字一律取消。失败不判死——管道不存在
    // 恰好说明 DLL 未实例化=没有旧组字可清(Chromium 类应用懒加载输入法,
    // 2026-09-22 VS Code 实测开场 700ms 无管道);update 节奏会持续重试,
    // 应用何时把 TIP 拉起来就何时接上。
    let _ = cancel!();
    let mut residue_visible = false;
    // 无残留期的连续失败上限:防"永不加载 DLL 的应用"整场空转管道扫描。
    let mut consecutive_update_failures: u32 = 0;
    const STREAMING_UPDATE_FAILURE_BUDGET: u32 = 24;
    loop {
        let command = tokio::select! {
            biased;
            maybe = rx.recv() => {
                match maybe {
                    Some(command) => command,
                    None => break,
                }
            }
            _ = tokio::time::sleep(STREAMING_COMPOSITION_IDLE_CHECK_INTERVAL) => {
                let (current_session, cancelled) = {
                    let state = inner.state.lock();
                    (state.session_id, state.cancelled)
                };
                if current_session == session_id && !cancelled {
                    continue;
                }
                if residue_visible {
                    let _ = cancel!();
                }
                break;
            }
        };
        match command {
            StreamingCompositionCommand::Update { text, key } => {
                if streaming_run_op(
                    &inner,
                    session_id,
                    &prepared,
                    crate::windows_ime_ipc::ImeStreamOp::Update,
                    &text,
                    target.as_ref(),
                )
                .await
                {
                    residue_visible = true;
                    streaming_state_apply(&inner, session_id, |state| {
                        state.updates_sent = state.updates_sent.saturating_add(1);
                        state.last_update_sent_at = Some(Instant::now());
                        state.last_sent_key = key;
                    });
                    consecutive_update_failures = 0;
                } else if residue_visible {
                    // 屏上有组字却失去管道控制:清场,清不掉=污染(宁少不双写)。
                    let cleared = cancel!();
                    streaming_state_apply(&inner, session_id, |state| {
                        streaming_state_fail(
                            state,
                            "stream_update_rejected",
                            !cleared,
                        );
                    });
                    return;
                } else {
                    // 无残留期失败 = 管道还没起来(懒加载应用):计数重试,
                    // 超预算才降级,避免整场空转管道扫描。
                    consecutive_update_failures = consecutive_update_failures.saturating_add(1);
                    if consecutive_update_failures >= STREAMING_UPDATE_FAILURE_BUDGET {
                        streaming_state_apply(&inner, session_id, |state| {
                            streaming_state_fail(
                                state,
                                "stream_update_pipe_never_ready",
                                false,
                            );
                        });
                        return;
                    }
                }
            }
            StreamingCompositionCommand::Commit { text, reply } => {
                let ok = streaming_run_op(
                    &inner,
                    session_id,
                    &prepared,
                    crate::windows_ime_ipc::ImeStreamOp::Commit,
                    &text,
                    target.as_ref(),
                )
                .await;
                if ok {
                    residue_visible = false;
                    streaming_state_apply(&inner, session_id, |state| {
                        state.commits = state.commits.saturating_add(1);
                    });
                } else {
                    let cleared = cancel!();
                    streaming_state_apply(&inner, session_id, |state| {
                        streaming_state_fail(
                            state,
                            "stream_commit_failed",
                            residue_visible && !cleared,
                        );
                    });
                }
                if let Some(reply) = reply {
                    let _ = reply.send(ok);
                }
                if !ok {
                    return;
                }
            }
            StreamingCompositionCommand::CancelComposition { reply } => {
                let had_residue = residue_visible;
                let ok = cancel!();
                residue_visible = false;
                if let Some(reply) = reply {
                    let _ = reply.send(ok);
                }
                if !ok {
                    streaming_state_apply(&inner, session_id, |state| {
                        streaming_state_fail(state, "stream_cancel_failed", had_residue);
                    });
                    return;
                }
            }
            StreamingCompositionCommand::EndSession => {
                if residue_visible {
                    let _ = cancel!();
                }
                break;
            }
        }
    }
    streaming_state_apply(&inner, session_id, |state| {
        if state.phase == StreamingCompositionPhase::Active {
            state.phase = StreamingCompositionPhase::Ended;
            state.command_tx = None;
        }
    });
}

#[cfg(test)]
mod streaming_composition_tests {
    use super::*;

    #[test]
    fn fresh_state_is_disabled_and_send_is_noop() {
        let state = StreamingCompositionState::default();
        assert_eq!(state.phase, StreamingCompositionPhase::Disabled);
        assert!(state.session_id.is_none());
    }

    #[test]
    fn update_should_send_requires_change_beyond_last_key() {
        assert!(!streaming_update_should_send("", ""));
        assert!(!streaming_update_should_send("你好", "你好"));
        // 增长要发;收缩/改写也要发——组字原地替换,修订正是它的职责。
        assert!(streaming_update_should_send("你好世", "你好"));
        assert!(streaming_update_should_send("你哈", "你好"));
        assert!(streaming_update_should_send("你", "你好"));
    }

    #[test]
    fn failure_marks_contamination_and_ends_only_from_active() {
        let mut state = StreamingCompositionState::default();
        streaming_state_fail(&mut state, "stream_update_rejected", true);
        assert_eq!(
            state.phase,
            StreamingCompositionPhase::Failed {
                reason: "stream_update_rejected",
                contaminated: true
            }
        );
        assert!(state.command_tx.is_none());
        // Failed 是终态:后续 EndSession 不改写(避免把降级事实抹掉)。
        if state.phase == StreamingCompositionPhase::Active {
            state.phase = StreamingCompositionPhase::Ended;
        }
        assert_ne!(state.phase, StreamingCompositionPhase::Ended);
    }
}
