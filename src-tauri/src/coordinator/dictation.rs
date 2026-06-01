use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::coordinator_state::request_stop_during_starting_state;
use crate::correction::apply_correction_rules;
use crate::types::HotkeyMode;

use super::qa::handle_qa_option_edge;
use super::resources::*;
use super::*;

/// 同一个 hotkey 边沿之间的最小间隔。低于此阈值的连按整体作为误触丢弃 ——
/// 避免微动开关回弹 / 用户手抖双击造成的空转写报错和 ASR session 抢资源。
const HOTKEY_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
const EMBEDDED_AUDIO_FEED_CHUNK_BYTES: usize = 3_200;
const EMBEDDED_AUDIO_ASR_PREROLL_MS: usize = 800;
const EMBEDDED_AUDIO_ASR_PREROLL_BYTES: usize = 16_000 * 2 * EMBEDDED_AUDIO_ASR_PREROLL_MS / 1_000;
const EMBEDDED_AUDIO_TARGET_RMS: f64 = 2_300.0;
const EMBEDDED_AUDIO_MAX_GAIN: f64 = 16.0;
const EMBEDDED_AUDIO_MIN_GAIN: f64 = 1.05;

fn clear_embedded_audio_stats(inner: &Arc<Inner>) {
    *inner.embedded_audio_stats.lock() = None;
}

fn clear_embedded_audio_partial_preview(inner: &Arc<Inner>) {
    *inner.embedded_audio_partial_preview.lock() = None;
}

fn clear_embedded_audio_stop_feedback(inner: &Arc<Inner>) {
    inner
        .embedded_audio_stop_feedback_latched
        .store(false, Ordering::SeqCst);
}

fn latch_embedded_audio_stop_feedback(inner: &Arc<Inner>) {
    inner
        .embedded_audio_stop_feedback_latched
        .store(true, Ordering::SeqCst);
}

fn embedded_audio_stop_feedback_latched(inner: &Arc<Inner>) -> bool {
    inner
        .embedded_audio_stop_feedback_latched
        .load(Ordering::SeqCst)
}

fn register_embedded_ble_cancel_flag(inner: &Arc<Inner>, flag: &Arc<AtomicBool>) {
    *inner.embedded_ble_cancel_flag.lock() = Some(Arc::clone(flag));
}

fn clear_embedded_ble_cancel_flag(inner: &Arc<Inner>, flag: &Arc<AtomicBool>) {
    let mut slot = inner.embedded_ble_cancel_flag.lock();
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, flag))
    {
        *slot = None;
    }
}

fn request_embedded_ble_capture_cancel(inner: &Arc<Inner>) -> bool {
    let flag = inner.embedded_ble_cancel_flag.lock().clone();
    if let Some(flag) = flag {
        flag.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

fn current_embedded_audio_partial_preview(inner: &Arc<Inner>) -> Option<String> {
    inner.embedded_audio_partial_preview.lock().clone()
}

fn update_embedded_audio_partial_preview(inner: &Arc<Inner>, session_id: SessionId, text: String) {
    let preview = text.trim().to_string();
    if preview.is_empty() {
        return;
    }
    {
        let mut slot = inner.embedded_audio_partial_preview.lock();
        if slot.as_deref() == Some(preview.as_str()) {
            return;
        }
        *slot = Some(preview.clone());
    }

    let (should_emit, capsule_state, elapsed) = {
        let state = inner.state.lock();
        let active_session = state.session_id == session_id;
        let capsule_state = if matches!(
            state.phase,
            SessionPhase::Starting | SessionPhase::Listening
        ) && embedded_audio_stop_feedback_latched(inner)
        {
            CapsuleState::Transcribing
        } else {
            match state.phase {
                SessionPhase::Starting | SessionPhase::Listening => CapsuleState::Recording,
                SessionPhase::Processing | SessionPhase::Inserting => CapsuleState::Transcribing,
                _ => CapsuleState::Idle,
            }
        };
        (
            active_session && capsule_state != CapsuleState::Idle,
            capsule_state,
            state.started_at.elapsed().as_millis() as u64,
        )
    };
    if should_emit {
        emit_capsule(inner, capsule_state, 0.0, elapsed, Some(preview), None);
    }
}

fn store_embedded_audio_stats(inner: &Arc<Inner>, stats: crate::embedded_audio::SessionStats) {
    *inner.embedded_audio_stats.lock() = Some(stats);
}

fn take_embedded_audio_stats(inner: &Arc<Inner>) -> Option<crate::embedded_audio::SessionStats> {
    inner.embedded_audio_stats.lock().take()
}

struct EmbeddedAudioDictationSession {
    session_id: SessionId,
    active_asr: String,
    consumer: Arc<dyn crate::recorder::AudioConsumer>,
    archive_pcm: Option<Vec<u8>>,
    streamed_pcm_bytes: usize,
    normalized_pcm_bytes: usize,
    boosted_chunk_count: usize,
    max_gain: f64,
    clipped_samples: usize,
    asr_preroll_sent: bool,
}

impl EmbeddedAudioDictationSession {
    fn consume_streaming_pcm(&mut self, inner: &Arc<Inner>, pcm: &[u8]) -> Result<(), String> {
        if pcm.is_empty() {
            return Ok(());
        }
        if pcm.len() % 2 != 0 {
            return Err("嵌入式音频 PCM chunk 长度不是 16-bit 对齐".to_string());
        }

        if let Some(archive_pcm) = self.archive_pcm.as_mut() {
            archive_pcm.extend_from_slice(pcm);
        }

        let (asr_pcm, gain_stats) = prepare_embedded_streaming_pcm_for_asr(&self.active_asr, pcm);
        self.streamed_pcm_bytes += pcm.len();
        self.normalized_pcm_bytes += asr_pcm.len();
        if gain_stats.gain > 1.0 {
            self.boosted_chunk_count += 1;
            self.max_gain = self.max_gain.max(gain_stats.gain);
            self.clipped_samples += gain_stats.clipped_samples;
        }

        let elapsed = inner.state.lock().started_at.elapsed().as_millis() as u64;
        let capsule_state = if embedded_audio_stop_feedback_latched(inner) {
            CapsuleState::Transcribing
        } else {
            CapsuleState::Recording
        };
        emit_capsule(
            inner,
            capsule_state,
            embedded_pcm_peak_level(&asr_pcm),
            elapsed,
            current_embedded_audio_partial_preview(inner),
            None,
        );
        feed_embedded_asr_preroll_if_needed(self);
        for chunk in asr_pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            self.consumer.consume_pcm_chunk(chunk);
        }
        Ok(())
    }
}

fn embedded_audio_asr_preroll_enabled(active_asr: &str) -> bool {
    active_asr == "volcengine"
}

fn feed_embedded_asr_preroll_if_needed(session: &mut EmbeddedAudioDictationSession) {
    if session.asr_preroll_sent {
        return;
    }
    session.asr_preroll_sent = true;
    if !embedded_audio_asr_preroll_enabled(&session.active_asr) {
        return;
    }

    let silence = vec![0u8; EMBEDDED_AUDIO_ASR_PREROLL_BYTES];
    for chunk in silence.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
        session.consumer.consume_pcm_chunk(chunk);
    }
    log::info!(
        "[coord] embedded audio ASR preroll inserted (asr={}, ms={}, bytes={})",
        session.active_asr,
        EMBEDDED_AUDIO_ASR_PREROLL_MS,
        EMBEDDED_AUDIO_ASR_PREROLL_BYTES
    );
}

#[derive(Default)]
struct EmbeddedStreamingDictation {
    collector: crate::embedded_audio::StreamingSessionCollector,
    session: Option<EmbeddedAudioDictationSession>,
    embedded_session_id: Option<u32>,
    pending_stop_expected_packet_count: Option<u16>,
    terminal_received: bool,
}

/// 跑流式润色路径（opt-in，跨平台）。
///
/// 平台差异：
/// - **macOS**：`switch_to_ascii` 切到 ABC 输入源（规避 CJK / 日文 IME 拦截 Unicode 事件），
///   session 结束 `restore_input_source` 切回。`type_unicode_chunk` 走 CGEvent FFI。
/// - **Windows**：`switch_to_ascii` 是 no-op（SendInput Unicode 绕过 TSF）；
///   `type_unicode_chunk` 走 `SendInput(KEYEVENTF_UNICODE)`。
/// - **Linux（实验）**：`switch_to_ascii` 是 no-op；`type_unicode_chunk` 走 enigo
///   `Keyboard::text`。X11 / XTest 稳定，Wayland 看 compositor 给不给 libei 权限。
///
/// 通用流程：
/// 1. `switch_to_ascii`（macOS）/ no-op（其他）；失败则降级回一次性 `polish_or_passthrough`。
/// 2. 起一个 `spawn_blocking` 后台任务，从 mpsc 收 SSE delta，逐 delta 调
///    `type_unicode_chunk` 模拟键盘事件落到光标处。串行有序，无竞态。
/// 3. 调 `polish_or_passthrough_streaming`，`on_delta` 把 chunk 塞进 mpsc。
/// 4. 流结束 / 失败 / 取消 → drop mpsc 发送端 → typer 任务 drain 完剩余 delta 退出 →
///    `restore_input_source` 恢复用户原输入源（macOS 才有意义，其他平台 no-op）。
/// 5. 返回 `(polished, polish_error, already_streamed)`：
///    - 成功：`(text, None, true)` — 字符已经在屏幕上，调用方应当跳过 `inserter.insert`
///    - 失败：`(raw_text, Some(reason), false)` — 流式过程出错，调用方走 raw 一次性兜底
///    - 不支持：`run_streaming_polish` 内部直接调 `polish_or_passthrough` 透明降级
///
/// **不在流式路径里做**：`apply_chinese_script_preference` / `apply_correction_rules`
/// 这两步在 v1 跳过 —— 字符已经一边流一边落出去了，不好回退。需要的话只能关 toggle 走
/// 一次性路径。
#[allow(clippy::too_many_arguments)]
async fn run_streaming_polish(
    inner: &Arc<Inner>,
    raw: &RawTranscript,
    mode: PolishMode,
    hotwords: &[String],
    style_system_prompt: &str,
    working_languages: &[String],
    chinese_script_preference: crate::types::ChineseScriptPreference,
    output_language_preference: crate::types::OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
    prior_turns: &[(String, String)],
) -> (String, Option<String>, bool) {
    log::info!(
        "[coord] streaming_insert path ENTER (raw_chars={})",
        raw.text.chars().count()
    );

    let app = inner.app.lock().clone();
    let Some(app) = app else {
        log::warn!("[coord] streaming_insert: no AppHandle in Inner; fall back to one-shot");
        let (p, e) = polish_or_passthrough(
            raw,
            mode,
            hotwords,
            style_system_prompt,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app,
            prior_turns,
        )
        .await;
        return (p, e, false);
    };

    // 1. 切到 ABC 输入源。失败则降级 —— 流式路径上 CJK IME 拦截不是可恢复错误。
    log::info!("[coord] streaming_insert: switching input source to ABC");
    let prev_ime = match crate::unicode_keystroke::switch_to_ascii(&app).await {
        Ok(prev) => {
            log::info!(
                "[coord] streaming_insert: switched to ABC (had_previous={})",
                prev.is_some()
            );
            prev
        }
        Err(e) => {
            log::warn!(
                "[coord] streaming_insert: switch_to_ascii failed: {e}; fall back to one-shot"
            );
            let (p, err) = polish_or_passthrough(
                raw,
                mode,
                hotwords,
                style_system_prompt,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                llm_thinking_enabled,
                front_app,
                prior_turns,
            )
            .await;
            return (p, err, false);
        }
    };

    // 2. 起 typer 后台任务：从 mpsc 收 delta，串行调 type_unicode_chunk。
    // 同时累积 typed_text：屏幕上真正落字的内容，用于（a）SSE 中途失败时让 history
    // 与用户实际看到的内容一致；（b）pr-agent #412 反馈 \"saved output diverges
    // from what the user actually sees\"。
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let typer_handle = tokio::task::spawn_blocking(move || {
        let mut rx = rx;
        let mut typed_text = String::new();
        let mut first_failure: Option<String> = None;
        while let Some(delta) = rx.blocking_recv() {
            if first_failure.is_some() {
                // 一旦类型链路出错（如 Secure Input 启用），后续 delta 全部丢弃，但仍
                // 把 mpsc drain 完，避免发送端阻塞。
                continue;
            }
            let delta_chars = delta.chars().count();
            match crate::unicode_keystroke::type_unicode_chunk(&delta) {
                Ok(typed_chars) => {
                    let appended = append_typed_prefix(&mut typed_text, &delta, typed_chars);
                    if appended < delta_chars {
                        let reason = format!(
                            "type_unicode_chunk typed only {appended}/{delta_chars} chars without error"
                        );
                        log::error!(
                            "[coord] streaming_insert: {reason} at typed={} chars; \
                             dropping remaining deltas",
                            typed_text.chars().count()
                        );
                        first_failure = Some(reason);
                    }
                }
                Err(e) => {
                    append_typed_prefix(&mut typed_text, &delta, e.typed_chars());
                    log::error!(
                        "[coord] streaming_insert: type_unicode_chunk failed at typed={} chars: {e}; \
                         dropping remaining deltas",
                        typed_text.chars().count()
                    );
                    first_failure = Some(e.to_string());
                }
            }
        }
        (typed_text, first_failure)
    });

    // 3. 调流式润色，on_delta 塞 mpsc；should_cancel 检查 dictation 取消旗。
    let inner_for_cancel = Arc::clone(inner);
    let should_cancel = move || inner_for_cancel.state.lock().cancelled;
    let outcome = super::polish_or_passthrough_streaming(
        raw,
        mode,
        hotwords,
        style_system_prompt,
        working_languages,
        chinese_script_preference,
        output_language_preference,
        llm_thinking_enabled,
        front_app,
        prior_turns,
        move |delta: &str| {
            let _ = tx.send(delta.to_string());
        },
        should_cancel,
    )
    .await;
    // tx 已经被 move 进 on_delta 闭包；闭包随 polish_or_passthrough_streaming 返回
    // 而 drop，typer 那侧 blocking_recv 拿到 None 自然退出。

    // 4. 等 typer 把缓冲 drain 完，拿到实际落字的全文 + 第一条失败原因。
    let (typed_text, typer_failure) = typer_handle.await.unwrap_or_else(|e| {
        log::error!("[coord] streaming_insert: typer task join failed: {e}");
        (String::new(), Some(format!("typer join: {e}")))
    });
    let typed_chars = typed_text.chars().count();
    log::info!("[coord] streaming_insert: typer drained, typed {typed_chars} chars");

    // 5. 无论流是否成功，都恢复用户原输入源。
    log::info!("[coord] streaming_insert: restoring input source");
    if let Err(e) = crate::unicode_keystroke::restore_input_source(&app, prev_ime).await {
        log::warn!("[coord] streaming_insert: restore_input_source failed: {e}");
    } else {
        log::info!("[coord] streaming_insert: input source restored");
    }

    // 6. 把 outcome 翻译成 (polished, polish_error, already_streamed)。
    match outcome {
        super::StreamingPolishOutcome::Streamed(text) => {
            log::info!(
                "[coord] streaming_insert SUCCESS: polished_chars={} typed_chars={} typer_err={:?}",
                text.chars().count(),
                typed_chars,
                typer_failure
            );
            // 边界 case：polish 成功但 typer 在第一字就失败（最常见：session 开始时
            // 已处于 Secure Input；或 SendInput / enigo 拒绝）。屏幕上一字未见，
            // already_streamed=true 会让上层跳过 inserter，最终用户看不到任何内容。
            // 这里显式回退到一次性兜底，让正常 inserter 路径写出 polish 结果。
            // pr-agent #412 反馈 \"Missing fallback\"。
            if typed_chars == 0 {
                if let Some(reason) = typer_failure {
                    log::warn!(
                        "[coord] streaming_insert: zero chars typed despite polish success ({reason}); falling back to one-shot inserter"
                    );
                    return (text, Some(reason), false);
                }
            }
            // 先确定 final_text —— typer 中途失败时屏幕只有 typed_text 这一段，
            // history 记完整 polish 反而会让用户复盘困惑。让 history / clipboard /
            // 后续逻辑统统用 final_text，三处保持一致。
            // pr-agent #412 反馈 \"Clipboard Mismatch\"：之前先写 text 到剪贴板再
            // 决定 typer 是否中途失败，导致 Cmd+V 粘出用户屏幕上没见过的内容。
            let (final_text, polish_err) = match typer_failure {
                Some(e) => (typed_text, Some(format!("typing partially failed: {e}"))),
                None => (text, None),
            };
            // 把 final_text 写回剪贴板（默认 on，可关）。一次性路径天然走剪贴板，
            // 开关默认对齐一次性行为，让 Cmd+V 重复粘贴可用。
            if inner.prefs.get().streaming_insert_save_clipboard {
                match arboard::Clipboard::new() {
                    Ok(mut cb) => match cb.set_text(final_text.clone()) {
                        Ok(()) => log::info!(
                            "[coord] streaming_insert: final text written to clipboard ({} chars)",
                            final_text.chars().count()
                        ),
                        Err(e) => {
                            log::warn!("[coord] streaming_insert: clipboard set_text failed: {e}")
                        }
                    },
                    Err(e) => {
                        log::warn!("[coord] streaming_insert: clipboard handle init failed: {e}")
                    }
                }
            } else {
                log::info!("[coord] streaming_insert: clipboard save skipped (pref off)");
            }
            (final_text, polish_err, true)
        }
        super::StreamingPolishOutcome::UnsupportedFallback => {
            log::info!(
                "[coord] streaming_insert: dispatch reported unsupported, fall back to one-shot"
            );
            let (p, e) = polish_or_passthrough(
                raw,
                mode,
                hotwords,
                style_system_prompt,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                llm_thinking_enabled,
                front_app,
                prior_turns,
            )
            .await;
            (p, e, false)
        }
        super::StreamingPolishOutcome::Failed(reason) => {
            log::warn!(
                "[coord] streaming_insert FAILED: {reason}; typed {typed_chars} chars before failure"
            );
            // 流式失败但已经流了一部分 chars：用户屏幕上有半截 polish。history 应当
            // 跟屏幕一致 —— 记 typed_text 而不是 raw.text，否则保存内容跟用户看见的
            // 内容会分叉（pr-agent #412 \"Wrong final text\" 反馈）。
            // 一字都没流时 typed_text 是空串，回到 raw 一次性兜底。
            if typed_chars > 0 {
                (
                    typed_text,
                    Some(format!(
                        "streaming polish failed mid-stream after {typed_chars} chars: {reason}"
                    )),
                    true,
                )
            } else {
                (raw.text.clone(), Some(reason), false)
            }
        }
    }
}

fn finalize_polished_text(
    polished: String,
    translation_active: bool,
    _raw_uses_llm: bool,
    mode: PolishMode,
    polish_error: &Option<String>,
    chinese_script_preference: crate::types::ChineseScriptPreference,
    correction_rules: &[crate::types::CorrectionRule],
    already_streamed: bool,
) -> String {
    if already_streamed {
        return polished;
    }
    let should_force_script = if translation_active {
        polish_error.is_some()
    } else {
        mode == PolishMode::Raw || polish_error.is_some()
    };
    let polished = if should_force_script {
        apply_chinese_script_preference(&polished, chinese_script_preference)
    } else {
        polished
    };
    if correction_rules.is_empty() {
        polished
    } else {
        let corrected = apply_correction_rules(&polished, correction_rules);
        if corrected != polished {
            log::info!(
                "[coord] correction rules adjusted final text ({} → {} chars)",
                polished.chars().count(),
                corrected.chars().count()
            );
        }
        corrected
    }
}

fn streaming_insert_eligible(
    streaming_insert_enabled: bool,
    translation_active: bool,
    mode: PolishMode,
    raw_uses_llm: bool,
    wayland_session: bool,
) -> bool {
    streaming_insert_enabled
        && !translation_active
        && (mode != PolishMode::Raw || raw_uses_llm)
        && !wayland_session
}

fn wayland_done_message(status: InsertStatus, polish_failed: bool) -> Option<String> {
    match status {
        InsertStatus::Inserted | InsertStatus::PasteSent => None,
        InsertStatus::CopiedFallback => Some(if polish_failed {
            "Wayland 未启用自动输入，已复制原文到剪贴板，请手动粘贴".to_string()
        } else {
            "Wayland 未启用自动输入，已复制到剪贴板，请手动粘贴".to_string()
        }),
        InsertStatus::Failed => Some("Wayland 未启用自动输入，剪贴板写入失败".to_string()),
    }
}

fn default_done_message(status: InsertStatus, polish_failed: bool) -> Option<String> {
    if polish_failed {
        // polish 失败优先告知用户，即使 insert 成功也要让用户知道这版是原文
        Some("润色失败，已插入原文".to_string())
    } else {
        match status {
            InsertStatus::Inserted => None,
            InsertStatus::PasteSent => Some("已尝试粘贴".to_string()),
            InsertStatus::CopiedFallback => Some(if cfg!(target_os = "windows") {
                "已复制，请 Ctrl+V".to_string()
            } else {
                "已复制，请粘贴".to_string()
            }),
            InsertStatus::Failed => Some("插入失败".to_string()),
        }
    }
}

pub(super) async fn handle_pressed_edge(inner: &Arc<Inner>) {
    let was_held = inner.hotkey_trigger_held.swap(true, Ordering::SeqCst);
    if !was_held {
        // 防抖：相邻 < HOTKEY_DEBOUNCE 的边沿直接丢弃，记到 log 方便排查。
        // 与 `hotkey_trigger_held` 互补：held 防 press-without-release，本检查防
        // press-release-press 三连过快。每个有效边沿都会更新时间戳。
        let now = std::time::Instant::now();
        let too_soon = {
            let mut last = inner.last_hotkey_dispatch_at.lock();
            let drop = matches!(*last, Some(t) if now.duration_since(t) < HOTKEY_DEBOUNCE);
            if !drop {
                *last = Some(now);
            }
            drop
        };
        if too_soon {
            log::info!(
                "[coord] hotkey pressed edge debounced (< {} ms since last dispatch)",
                HOTKEY_DEBOUNCE.as_millis()
            );
            return;
        }

        // 路由：QA 浮窗可见时，rightOption 边沿走 QA；否则走主听写。详见 issue #118 v2。
        // 例外：dictation session 已经在跑（Starting / Listening / Processing / Inserting），
        // 即使 QA 浮窗被打开了，这条边沿也必须先走 dictation。否则 begin_qa_session 会
        // 第二次抢同一个麦克风 device —— 在 Linux/PipeWire 上甚至会成功打开两路捕获，
        // dictation 的 recorder 没人停；在 macOS/Windows 上 cpal 会拒绝第二次 build_input_stream
        // 但 dictation session 仍在跑、用户找不到从 QA 面板停掉它的入口。审计 3.3.1。
        let dictation_active = !matches!(inner.state.lock().phase, SessionPhase::Idle);
        let panel_visible = inner.qa_state.lock().panel_visible;
        if panel_visible && !dictation_active {
            handle_qa_option_edge(inner).await;
        } else {
            handle_pressed(inner).await;
        }
    }
}

pub(super) async fn handle_pressed(inner: &Arc<Inner>) {
    let prefs = inner.prefs.get();
    let mode = prefs.hotkey.mode;
    if prefs.dictation_input_source == DictationInputSource::EmbeddedBle {
        log::info!("[coord] hotkey press ignored; Listener BLE input is driven by hardware key");
        return;
    }
    let phase = inner.state.lock().phase;
    log::info!("[coord] hotkey pressed (mode={mode:?}, phase={phase:?})");
    match (mode, phase) {
        (HotkeyMode::Toggle, SessionPhase::Idle) => {
            let _ = begin_session(inner).await;
        }
        (HotkeyMode::Toggle, SessionPhase::Listening) => {
            let _ = end_session(inner).await;
        }
        (HotkeyMode::Hold, SessionPhase::Idle) => {
            let _ = begin_session(inner).await;
        }
        // Toggle 模式 Starting 阶段第二次按 → 用户想停。
        // 不能直接 end_session（ASR session 还没建好），存边沿，握手完成后立即触发。
        (HotkeyMode::Toggle, SessionPhase::Starting) => {
            request_stop_during_starting(inner, "toggle stop edge");
        }
        _ => {}
    }
}

pub(super) async fn handle_released_edge(inner: &Arc<Inner>) {
    let was_held = inner.hotkey_trigger_held.swap(false, Ordering::SeqCst);
    if was_held {
        // QA 浮窗可见时，Option 行为是 press-toggle（不分 hold/release），release 边沿忽略。
        // 与 handle_pressed_edge 的路由对称：dictation session 在跑时 Pressed 已经被路由到
        // dictation，那 Released 必须也路由到 dictation —— 否则 Hold 模式松开热键时
        // end_session 不会触发，dictation 永远停不下来。审计 3.3.1。
        let dictation_active = !matches!(inner.state.lock().phase, SessionPhase::Idle);
        let panel_visible = inner.qa_state.lock().panel_visible;
        if panel_visible && !dictation_active {
            return;
        }
        handle_released(inner).await;
    }
}

pub(super) async fn handle_released(inner: &Arc<Inner>) {
    let prefs = inner.prefs.get();
    let mode = prefs.hotkey.mode;
    if prefs.dictation_input_source == DictationInputSource::EmbeddedBle {
        log::info!("[coord] hotkey release ignored; Listener BLE input is driven by hardware key");
        return;
    }
    let phase = inner.state.lock().phase;
    log::info!("[coord] hotkey released (mode={mode:?}, phase={phase:?})");
    if mode == HotkeyMode::Hold {
        match phase {
            SessionPhase::Listening => {
                let _ = end_session(inner).await;
            }
            // Hold 模式 Starting 阶段松开 → 用户想停。同上：握手完成后再 end。
            SessionPhase::Starting => {
                request_stop_during_starting(inner, "hold release edge");
            }
            _ => {}
        }
    }
}

pub(super) fn request_stop_during_starting(inner: &Arc<Inner>, reason: &str) {
    {
        let mut state = inner.state.lock();
        if !request_stop_during_starting_state(&mut state) {
            return;
        }
    }
    log::info!("[coord] {reason} during Starting — queued");
    stop_recorder_if_pending_start_stop(inner);
}

pub(super) async fn begin_session(inner: &Arc<Inner>) -> Result<(), String> {
    let current_session_id = {
        let mut state = inner.state.lock();
        let Some(session_id) =
            begin_session_state(&mut state, capture_focus_target(), capture_frontmost_app())
        else {
            return Ok(());
        };
        if let Some(label) = state.front_app.as_deref() {
            log::info!("[coord] front_app captured: {label}");
        }
        session_id
    };
    clear_embedded_audio_stats(inner);
    clear_embedded_audio_partial_preview(inner);
    #[cfg(target_os = "windows")]
    {
        let prepared = inner.windows_ime.prepare_session();
        let mut slots = inner.prepared_windows_ime_session.lock();
        store_prepared_windows_ime_session(&mut slots, current_session_id, prepared);
    }
    // 翻译模式标志重置；hotkey 监听器在 Shift down 时再 set true。
    inner
        .translation_modifier_seen
        .store(false, Ordering::SeqCst);

    #[cfg(any(debug_assertions, test))]
    if hotkey_injection_dry_run_enabled() {
        emit_capsule(inner, CapsuleState::Recording, 0.0, 0, None, None);
        inner.state.lock().phase = SessionPhase::Listening;
        log::info!("[coord] session started (hotkey-injection dry-run)");
        return Ok(());
    }

    if let Err(message) = ensure_asr_credentials() {
        log::warn!("[coord] ASR credential gate failed: {message}");
        emit_capsule(
            inner,
            CapsuleState::Error,
            0.0,
            0,
            Some(message.clone()),
            None,
        );
        restore_prepared_windows_ime_session(inner, current_session_id);
        inner.state.lock().phase = SessionPhase::Idle;
        return Err(message);
    }

    let active_asr = CredentialsVault::get_active_asr();

    if let Err(message) = ensure_microphone_permission(inner) {
        log::warn!("[coord] microphone permission gate failed: {message}");
        emit_capsule(
            inner,
            CapsuleState::Error,
            0.0,
            0,
            Some(message.clone()),
            None,
        );
        restore_prepared_windows_ime_session(inner, current_session_id);
        inner.state.lock().phase = SessionPhase::Idle;
        schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
        return Err(message);
    }

    // 不在这里 emit Recording capsule —— 让 start_recorder_for_starting 在
    // Recorder::start 成功后再发，确保「用户看到录音条」时 mic 已经在 capture。
    // 之前在这一行就 emit 会让用户看到录音条后立刻开口，但 mic 还在 cpal init
    // 窗口（50-200ms）内 → 开头几个字物理上录不到。详见 issue 备注。
    #[cfg(target_os = "windows")]
    if foundry::is_foundry_local_whisper(&active_asr) {
        let prefs = inner.prefs.get();
        let model_alias = if foundry::model_alias_is_known(&prefs.foundry_local_asr_model) {
            prefs.foundry_local_asr_model.clone()
        } else {
            foundry::DEFAULT_MODEL_ALIAS.to_string()
        };
        let language_hint = prefs.foundry_local_asr_language_hint.trim().to_string();
        let language_hint = if language_hint.is_empty() {
            None
        } else {
            Some(language_hint)
        };
        let local = Arc::new(FoundryLocalWhisperAsr::new(
            Arc::clone(&inner.foundry_local_runtime),
            model_alias,
            prefs.foundry_local_runtime_source.clone(),
            language_hint,
        ));
        store_asr_for_session(
            inner,
            current_session_id,
            ActiveAsr::FoundryLocalWhisper(Arc::clone(&local)),
        );
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = local;
        start_recorder_and_enter_listening(inner, current_session_id, &active_asr, consumer)
            .await?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    if crate::asr::local::is_local_qwen3(&active_asr) {
        let local = match build_local_qwen3(inner).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("[coord] 本地 Qwen3-ASR 初始化失败: {e:#}");
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some(format!("本地模型初始化失败: {e}")),
                    None,
                );
                restore_prepared_windows_ime_session(inner, current_session_id);
                inner.state.lock().phase = SessionPhase::Idle;
                schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                return Err(format!("local ASR init failed: {e}"));
            }
        };
        store_asr_for_session(
            inner,
            current_session_id,
            ActiveAsr::Local(Arc::clone(&local)),
        );
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = local;
        start_recorder_and_enter_listening(inner, current_session_id, &active_asr, consumer)
            .await?;
        return Ok(());
    }

    if is_bailian_provider(&active_asr) {
        let asr = Arc::new(BailianRealtimeASR::new(read_bailian_credentials()));
        let bridge = Arc::new(DeferredAsrBridge::new());
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge.clone();
        store_asr_for_session(
            inner,
            current_session_id,
            ActiveAsr::Bailian(Arc::clone(&asr)),
        );
        start_recorder_for_starting(inner, current_session_id, &active_asr, consumer).await?;

        if let Err(e) = asr.open_session().await {
            log::error!("[coord] open Bailian ASR session failed: {e}");
            match startup_race_status_for_starting(inner, current_session_id) {
                StartupRaceStatus::StaleContinuation => {
                    log::info!(
                        "[coord] stale Bailian ASR open_session error from session {current_session_id} — ignoring"
                    );
                    asr.cancel();
                    discard_startup_resources_for_session(inner, current_session_id);
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    return Ok(());
                }
                StartupRaceStatus::CancelRaced => {
                    asr.cancel();
                    discard_startup_resources_for_session(inner, current_session_id);
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    set_phase_idle_if_session_matches(inner, current_session_id);
                    return Ok(());
                }
                StartupRaceStatus::ActiveStarting => {
                    asr.cancel();
                }
            }
            discard_startup_resources_for_session(inner, current_session_id);
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                0,
                Some(format!("ASR 连接失败: {e}")),
                None,
            );
            restore_prepared_windows_ime_session(inner, current_session_id);
            set_phase_idle_if_session_matches(inner, current_session_id);
            schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
            return Err(e.to_string());
        }
        match startup_race_status_for_starting(inner, current_session_id) {
            StartupRaceStatus::ActiveStarting => {}
            StartupRaceStatus::CancelRaced => {
                log::info!("[coord] cancel raced during Bailian ASR open_session — aborting begin");
                asr.cancel();
                discard_startup_resources_for_session(inner, current_session_id);
                restore_prepared_windows_ime_session(inner, current_session_id);
                set_phase_idle_if_session_matches(inner, current_session_id);
                return Ok(());
            }
            StartupRaceStatus::StaleContinuation => {
                log::info!(
                    "[coord] stale Bailian ASR open_session continuation from session {current_session_id} — ignoring"
                );
                asr.cancel();
                discard_startup_resources_for_session(inner, current_session_id);
                restore_prepared_windows_ime_session(inner, current_session_id);
                return Ok(());
            }
        }
        let target: Arc<dyn crate::asr::AudioConsumer> = asr;
        let flushed_bytes = bridge.attach(target);
        log::info!("[coord] Bailian ASR connected; flushed {flushed_bytes} deferred audio bytes");
        finish_starting_session(inner, current_session_id).await;
    } else if is_whisper_compatible_provider(&active_asr) {
        let (api_key, base_url, model, proxy_config) =
            read_whisper_credentials().map_err(|e| e.to_string())?;
        // 用户辞書の有効フレーズを Whisper の `prompt` に流し込む。固有名詞や
        // 専門用語の同音・近形誤認識を ASR 段階で抑える。Polish LLM 側には
        // 既に system prompt として注入済みだが、Whisper 出力が大きく崩れる
        // と Polish でも救えない（特に CJK で顕著）。Volcengine ASR は元々
        // hotword を受け取っており、UI 説明文も「ASR ホットワードと後処理
        // モデルのコンテキスト両方に渡される」と明示しているので、Whisper
        // 互換プロバイダにも揃えるのが筋。
        let whisper_prompt =
            crate::asr::whisper::build_prompt_from_phrases(&enabled_phrases(inner));
        let client = http_client_builder_with_proxy(&base_url, 30, &proxy_config)
            .build()
            .map_err(|e| format!("build Whisper HTTP client failed: {e}"))?;
        let whisper = Arc::new(WhisperBatchASR::new_with_client(
            api_key,
            base_url,
            model,
            whisper_prompt,
            client,
        ));
        store_asr_for_session(
            inner,
            current_session_id,
            ActiveAsr::Whisper(Arc::clone(&whisper)),
        );
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = whisper;
        start_recorder_and_enter_listening(inner, current_session_id, &active_asr, consumer)
            .await?;
    } else {
        let hotwords = enabled_hotwords(inner);
        let creds = read_volc_credentials();
        let asr = Arc::new(VolcengineStreamingASR::new(creds, hotwords));
        let bridge = Arc::new(DeferredAsrBridge::new());
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge.clone();
        store_asr_for_session(
            inner,
            current_session_id,
            ActiveAsr::Volcengine(Arc::clone(&asr)),
        );
        start_recorder_for_starting(inner, current_session_id, &active_asr, consumer).await?;

        if let Err(e) = asr.open_session().await {
            log::error!("[coord] open ASR session failed: {e}");
            match startup_race_status_for_starting(inner, current_session_id) {
                StartupRaceStatus::StaleContinuation => {
                    log::info!(
                        "[coord] stale ASR open_session error from session {current_session_id} — ignoring"
                    );
                    asr.cancel();
                    discard_startup_resources_for_session(inner, current_session_id);
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    return Ok(());
                }
                StartupRaceStatus::CancelRaced => {
                    asr.cancel();
                    discard_startup_resources_for_session(inner, current_session_id);
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    set_phase_idle_if_session_matches(inner, current_session_id);
                    return Ok(());
                }
                StartupRaceStatus::ActiveStarting => {}
            }
            discard_startup_resources_for_session(inner, current_session_id);
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                0,
                Some(format!("ASR 连接失败: {e}")),
                None,
            );
            restore_prepared_windows_ime_session(inner, current_session_id);
            set_phase_idle_if_session_matches(inner, current_session_id);
            schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
            return Err(e.to_string());
        }
        // open_session.await 期间用户可能按了 Esc / 改变心意。如果 cancel_session
        // 已触发（cancelled=true 或 phase 被改回 Idle），别再装 ASR，直接善后。
        // audit HIGH #1。
        match startup_race_status_for_starting(inner, current_session_id) {
            StartupRaceStatus::ActiveStarting => {}
            StartupRaceStatus::CancelRaced => {
                log::info!("[coord] cancel raced during ASR open_session — aborting begin");
                asr.cancel();
                discard_startup_resources_for_session(inner, current_session_id);
                restore_prepared_windows_ime_session(inner, current_session_id);
                set_phase_idle_if_session_matches(inner, current_session_id);
                return Ok(());
            }
            StartupRaceStatus::StaleContinuation => {
                log::info!(
                    "[coord] stale ASR open_session continuation from session {current_session_id} — ignoring"
                );
                asr.cancel();
                discard_startup_resources_for_session(inner, current_session_id);
                restore_prepared_windows_ime_session(inner, current_session_id);
                return Ok(());
            }
        }
        let target: Arc<dyn crate::asr::AudioConsumer> = asr;
        let flushed_bytes = bridge.attach(target);
        log::info!("[coord] ASR connected; flushed {flushed_bytes} deferred audio bytes");
        finish_starting_session(inner, current_session_id).await;
    }

    Ok(())
}

pub(super) async fn start_recorder_for_starting(
    inner: &Arc<Inner>,
    session_id: SessionId,
    active_asr: &str,
    consumer: Arc<dyn crate::recorder::AudioConsumer>,
) -> Result<(), String> {
    let inner_for_level = Arc::clone(inner);
    // 节流：电平回调本身约 185 Hz（cpal 默认音频块），全部转发到前端会让 CSS
    // transition 互相覆盖、视觉上"被平均"成静止。限制为 ~30 Hz（33ms 最少间隔），
    // 配合 CSS 短 transition 让每次 emit 完整可见。
    let last_emit_at = Arc::new(Mutex::new(None::<Instant>));
    const LEVEL_EMIT_MIN_INTERVAL_MS: u64 = 33;
    let level_handler: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |level| {
        let phase = inner_for_level.state.lock().phase;
        if phase != SessionPhase::Listening && phase != SessionPhase::Starting {
            return;
        }
        let now = Instant::now();
        {
            let mut last = last_emit_at.lock();
            if let Some(prev) = *last {
                if now.duration_since(prev).as_millis() < LEVEL_EMIT_MIN_INTERVAL_MS as u128 {
                    return;
                }
            }
            *last = Some(now);
        }
        let elapsed = inner_for_level
            .state
            .lock()
            .started_at
            .elapsed()
            .as_millis() as u64;
        emit_capsule(
            &inner_for_level,
            CapsuleState::Recording,
            level,
            elapsed,
            None,
            None,
        );
    });

    let microphone_device_name = selected_microphone_device_name(inner);
    stop_microphone_preview_monitor(inner, "dictation recorder");
    acquire_recording_mute(inner, "dictation").await;
    let audio_archive_path = if inner.prefs.get().record_audio_for_debug {
        // 用 coordinator 的 SessionId 作为文件名，跟 history 那条记录 id 对齐（见
        // 下游 polish 收尾时 `history_session_id = current_session_id.to_string()`）。
        // 顺手把超龄 / 超量录音清理一下，避免 debug 开关常开时磁盘膨胀。
        let prefs = inner.prefs.get();
        let _ = crate::persistence::prune_recordings(
            prefs.history_retention_days,
            prefs.audio_recording_max_entries,
        );
        crate::persistence::recording_path_for_session(&session_id.to_string()).ok()
    } else {
        None
    };
    match Recorder::start(
        microphone_device_name,
        consumer,
        level_handler,
        audio_archive_path,
    ) {
        Ok((rec, runtime_errors, archive_active)) => {
            // 把 archive 实际创建状态存到 Inner，让 history 写入路径（含 empty-transcript
            // 失败分支）读真实情况，而不是 prefs 开关。修 pr_agent "Wrong Flag" 反馈。
            inner
                .audio_archive_active
                .store(archive_active, std::sync::atomic::Ordering::Relaxed);
            store_recorder_for_session(inner, session_id, rec);
            spawn_recorder_error_monitor(inner, runtime_errors);
            // 不在这里 emit Recording capsule。
            // Recorder::start Ok 仅代表 cpal Stream::play 完成，不代表 audio
            // 线程已经在向 consumer 推 PCM —— macOS CoreAudio AudioUnit 启动到
            // 第一帧 process_callback 中间有 50–200 ms 间隙（Windows 类似）。
            // 之前在这里立即 emit Recording 会让用户「看到录音条」就开口，但前几个
            // 字落在 cpal init 窗口里被吞，反映为短录音漏首字（用户报告）。
            //
            // 现改为：level_handler 第一次被触发时才 emit Recording capsule。
            // recorder.rs::process_callback 的顺序是 consume_pcm_chunk → level_handler，
            // 所以 level_handler 第一次执行 == PCM 已经真实流到 consumer。从这一刻
            // 起用户说什么都被录到。capsule 自然就晚 50–200 ms 出现，但出现 ==
            // mic 真的在录，匹配「麦先录、UI 再弹」的预期。
            //
            // 原本的竞态保护交还给两条已有路径：
            //   - stop_recorder_if_pending_start_stop：短按时把 capsule 切到
            //     Transcribing；recorder 已 stop，level_handler 不会再发火。
            //   - level_handler 内部 phase 检查：cancel / 错误使 phase 不在
            //     {Starting, Listening} 时直接 return，不会在错误状态上盖
            //     Recording。
            stop_recorder_if_pending_start_stop(inner);
            log::info!("[coord] recorder started (asr={active_asr}, phase=Starting)");
        }
        Err(e) => {
            log::error!("[coord] recorder start failed: {e}");
            cancel_asr_for_session(inner, session_id);
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                0,
                Some(format!("录音启动失败: {e}")),
                None,
            );
            restore_prepared_windows_ime_session(inner, session_id);
            release_recording_mute(inner, "dictation");
            inner.state.lock().phase = SessionPhase::Idle;
            schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
            return Err(e.to_string());
        }
    }

    Ok(())
}

pub(super) fn spawn_recorder_error_monitor(inner: &Arc<Inner>, rx: mpsc::Receiver<RecorderError>) {
    // 捕获当前 session_id：err 来时若 id 已经不一致说明是上一 session 的迟到事件，
    // 不能去 abort 当前 active 的新 session（它录得好好的）。
    let captured_session_id = inner.state.lock().session_id;
    let inner = Arc::clone(inner);
    std::thread::Builder::new()
        .name("listener-type-recorder-error-monitor".into())
        .spawn(move || {
            if let Ok(err) = rx.recv() {
                let current_session_id = inner.state.lock().session_id;
                if captured_session_id != current_session_id {
                    log::warn!(
                        "[coord] recorder error from stale session {} dropped (current={}, err={})",
                        captured_session_id,
                        current_session_id,
                        err
                    );
                    return;
                }
                log::error!("[coord] recorder runtime error: {err}");
                abort_recording_with_error(&inner, format!("录音中断: {err}"));
            }
        })
        .ok();
}

pub(super) fn abort_recording_with_error(inner: &Arc<Inner>, message: String) {
    let Some(abort) = ({
        let mut state = inner.state.lock();
        begin_recording_abort_before_restore(&mut state)
    }) else {
        return;
    };

    discard_startup_resources_for_session(inner, abort.session_id);
    restore_prepared_windows_ime_session(inner, abort.session_id);
    {
        let mut state = inner.state.lock();
        publish_abort_idle_after_restore(&mut state, abort.session_id);
    }

    emit_capsule(
        inner,
        CapsuleState::Error,
        0.0,
        abort.elapsed,
        Some(message),
        None,
    );
    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
}

pub(super) async fn start_recorder_and_enter_listening(
    inner: &Arc<Inner>,
    session_id: SessionId,
    active_asr: &str,
    consumer: Arc<dyn crate::recorder::AudioConsumer>,
) -> Result<(), String> {
    start_recorder_for_starting(inner, session_id, active_asr, consumer).await?;
    finish_starting_session(inner, session_id).await;
    Ok(())
}

pub(super) async fn finish_starting_session(inner: &Arc<Inner>, session_id: SessionId) {
    // audit HIGH #1：转 Listening 之前在同一 lock 内检查 cancel race。
    // 之前是无条件 phase=Listening，会把 cancel_session 在 await 期间设的 Idle
    // 反向覆盖回 Listening → 用户的 cancel 边沿被吞掉。
    let outcome = {
        let mut state = inner.state.lock();
        finish_starting_session_state(&mut state, session_id)
    };
    match outcome {
        BeginOutcome::StaleContinuation => {
            log::info!(
                "[coord] stale recorder/ASR startup continuation from session {session_id} — ignoring"
            );
            discard_startup_resources_for_session(inner, session_id);
            restore_prepared_windows_ime_session(inner, session_id);
        }
        BeginOutcome::CancelRaced => {
            log::info!("[coord] cancel raced during recorder/ASR startup — aborting begin");
            discard_startup_resources_for_session(inner, session_id);
            restore_prepared_windows_ime_session(inner, session_id);
            set_phase_idle_if_session_matches(inner, session_id);
        }
        BeginOutcome::Started | BeginOutcome::PendingStop => {
            log::info!("[coord] session started");
            if matches!(outcome, BeginOutcome::PendingStop) {
                log::info!("[coord] applying pending_stop edge → end_session immediately");
                let _ = end_session(inner).await;
            }
        }
    }
}

pub(super) async fn submit_embedded_audio_notifications(
    inner: &Arc<Inner>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let collector =
        crate::embedded_audio::collect_notifications(notifications.iter().map(Vec::as_slice))
            .map_err(|err| format!("嵌入式音频包解析失败: {err}"))?;
    let stats = collector.stats();
    if !stats.terminal_received {
        return Err("嵌入式音频会话尚未收到结束包".to_string());
    }
    if stats.end_reason != Some(crate::embedded_audio::SessionEndReason::Stop) {
        return Err(format!("嵌入式音频会话未正常结束: {:?}", stats.end_reason));
    }

    let pcm = collector.reconstructed_asr_boundary_pcm();
    if pcm.is_empty() {
        return Err("嵌入式音频会话没有可识别的 PCM 数据".to_string());
    }
    if pcm.len() % 2 != 0 {
        return Err("嵌入式音频 PCM 长度不是 16-bit 对齐".to_string());
    }

    let reconstructed_pcm_bytes = stats.reconstructed_pcm_bytes;
    if stats.post_stop_packet_count > 0 {
        log::info!(
            "[coord] embedded audio batch excluded post-stop tail from ASR (tail_packets={}, tail_pcm_bytes={}, asr_pcm_bytes={}, reconstructed_pcm_bytes={})",
            stats.post_stop_packet_count,
            stats.post_stop_pcm_bytes,
            stats.asr_boundary_pcm_bytes,
            stats.reconstructed_pcm_bytes
        );
    }
    submit_embedded_pcm_for_dictation_with_stats(inner, &pcm, Some(stats.clone())).await?;
    Ok(crate::embedded_audio::EmbeddedAudioSubmissionResult {
        stats,
        reconstructed_pcm_bytes,
    })
}

pub(super) async fn submit_embedded_audio_streaming_notifications(
    inner: &Arc<Inner>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let mut streaming = EmbeddedStreamingDictation::default();
    for notification in notifications {
        match streaming.handle_notification(inner, &notification).await {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => {
                streaming.abort_active_session(inner, &err);
                return Err(err);
            }
        }
    }
    if !streaming.terminal_received {
        streaming.abort_active_session(inner, "嵌入式音频流式会话尚未收到结束包");
    }
    streaming.into_submission_result()
}

pub(super) async fn submit_embedded_audio_file(
    inner: &Arc<Inner>,
    path: std::path::PathBuf,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let pcm = crate::embedded_audio::read_input_pcm(&path, format)
        .map_err(|err| format!("读取嵌入式音频文件失败 ({}): {err}", path.display()))?;
    let notifications = crate::embedded_audio::build_session_replay_notifications(
        crate::embedded_audio::ReplayConfig {
            session_id: embedded_audio_file_session_id(),
            payload_pcm_bytes: crate::embedded_audio::DEFAULT_REPLAY_PAYLOAD_PCM_BYTES,
        },
        &pcm,
    )
    .map_err(|err| format!("构造嵌入式音频回放包失败: {err}"))?;
    submit_embedded_audio_notifications(inner, notifications).await
}

pub(super) async fn submit_embedded_audio_streaming_file(
    inner: &Arc<Inner>,
    path: std::path::PathBuf,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let pcm = crate::embedded_audio::read_input_pcm(&path, format)
        .map_err(|err| format!("读取嵌入式音频文件失败 ({}): {err}", path.display()))?;
    let notifications = crate::embedded_audio::build_session_replay_notifications(
        crate::embedded_audio::ReplayConfig {
            session_id: embedded_audio_file_session_id(),
            payload_pcm_bytes: crate::embedded_audio::DEFAULT_REPLAY_PAYLOAD_PCM_BYTES,
        },
        &pcm,
    )
    .map_err(|err| format!("构造嵌入式音频流式回放包失败: {err}"))?;
    let mut streaming = EmbeddedStreamingDictation::default();
    for notification in notifications {
        let packet_duration = crate::embedded_audio::parse_packet(&notification)
            .ok()
            .filter(|packet| {
                packet.header.packet_type == crate::embedded_audio::PacketType::AudioData
            })
            .map(|packet| {
                Duration::from_secs_f64(
                    packet.header.packet_pcm_bytes as f64
                        / crate::embedded_audio::PCM_BYTES_PER_SECOND as f64,
                )
            });
        match streaming.handle_notification(inner, &notification).await {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => {
                streaming.abort_active_session(inner, &err);
                return Err(err);
            }
        }
        if let Some(duration) = packet_duration {
            tokio::time::sleep(duration).await;
        }
    }
    if !streaming.terminal_received {
        streaming.abort_active_session(inner, "嵌入式音频流式文件回放尚未收到结束包");
    }
    streaming.into_submission_result()
}

pub(super) async fn submit_embedded_audio_ble_once(
    inner: &Arc<Inner>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(120_000).max(1_000));
    let notifications = tauri::async_runtime::spawn_blocking(move || {
        crate::embedded_ble::capture_notifications_once(timeout)
    })
    .await
    .map_err(|err| format!("嵌入式 BLE 抓音任务失败: {err}"))??;
    submit_embedded_audio_notifications(inner, notifications).await
}

pub(super) async fn submit_embedded_audio_ble_stream(
    inner: &Arc<Inner>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    submit_embedded_audio_ble_stream_impl(inner, timeout_ms, true, Arc::new(AtomicBool::new(false)))
        .await
}

pub(super) async fn submit_embedded_audio_ble_stream_background(
    inner: &Arc<Inner>,
    cancel_capture: Arc<AtomicBool>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    submit_embedded_audio_ble_stream_impl(inner, None, false, cancel_capture).await
}

fn embedded_ble_stream_idle_timeout(
    timeout: Duration,
    emit_idle_capture_errors: bool,
) -> Option<Duration> {
    emit_idle_capture_errors.then_some(timeout)
}

async fn submit_embedded_audio_ble_stream_impl(
    inner: &Arc<Inner>,
    timeout_ms: Option<u64>,
    emit_idle_capture_errors: bool,
    cancel_capture: Arc<AtomicBool>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(120_000).max(1_000));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    register_embedded_ble_cancel_flag(inner, &cancel_capture);
    let cancel_capture_for_task = Arc::clone(&cancel_capture);
    let ready_inner = (!emit_idle_capture_errors).then(|| Arc::clone(inner));
    let ready_cancel = Arc::clone(&cancel_capture);
    let capture_task = tauri::async_runtime::spawn_blocking(move || {
        let mut on_ready = || {
            if let Some(inner) = ready_inner.as_ref() {
                mark_embedded_ble_listener_ready(inner, &ready_cancel);
            }
            Ok(())
        };
        crate::embedded_ble::capture_notification_events_until_cancelled(
            embedded_ble_stream_idle_timeout(timeout, emit_idle_capture_errors),
            cancel_capture_for_task,
            &mut on_ready,
            &mut |event| {
                tx.send(event.notification)
                    .map_err(|_| "嵌入式音频流式处理已结束".to_string())
            },
        )
    });

    let mut streaming = EmbeddedStreamingDictation::default();
    while let Some(notification) = rx.recv().await {
        match streaming.handle_notification(inner, &notification).await {
            Ok(true) => {
                cancel_capture.store(true, Ordering::SeqCst);
                break;
            }
            Ok(false) => {}
            Err(err) => {
                streaming.abort_active_session(inner, &err);
                cancel_capture.store(true, Ordering::SeqCst);
                clear_embedded_ble_cancel_flag(inner, &cancel_capture);
                return Err(err);
            }
        }
    }
    let capture_cancel_requested = cancel_capture.load(Ordering::SeqCst);
    cancel_capture.store(true, Ordering::SeqCst);

    let capture_result = capture_task
        .await
        .map_err(|err| format!("嵌入式 BLE 流式抓音任务失败: {err}"))
        .and_then(|result| result);
    clear_embedded_ble_cancel_flag(inner, &cancel_capture);
    let cancelled_by_caller = !streaming.terminal_received
        && (streaming.session.is_some() || streaming.embedded_session_id.is_some())
        && inner.state.lock().cancelled;
    if capture_result.is_ok() && cancelled_by_caller {
        log::info!("[embedded-ble] streaming capture stopped after dictation cancel");
        return Ok(streaming.into_cancelled_submission_result());
    }
    if capture_result.is_ok()
        && !emit_idle_capture_errors
        && capture_cancel_requested
        && !streaming.terminal_received
        && streaming.session.is_none()
        && streaming.embedded_session_id.is_none()
    {
        return Err("嵌入式 BLE 后台监听已取消，尚未开始录音会话".to_string());
    }
    if let Err(err) = capture_result {
        if !streaming.terminal_received {
            record_embedded_ble_recovery_failure(inner, &err);
            let guidance = embedded_ble_wake_guidance_for_error(&err);
            let message = if emit_idle_capture_errors {
                format!("嵌入式 BLE 流式抓音中断: {guidance}")
            } else {
                format!("嵌入式 BLE 流式抓音中断: {guidance}; cause={err}")
            };
            if emit_idle_capture_errors || streaming.session.is_some() {
                streaming.abort_active_session(inner, &message);
            }
            return Err(message);
        }
    }
    if !streaming.terminal_received {
        if streaming.collector.inner().has_stopped_with_audio() {
            streaming.finish_pending_stop_after_capture(inner).await?;
        } else {
            if emit_idle_capture_errors || streaming.session.is_some() {
                streaming.abort_active_session(inner, "嵌入式 BLE 流式会话尚未收到结束包");
            }
        }
    }
    streaming.into_submission_result()
}

fn embedded_audio_file_session_id() -> u32 {
    (chrono::Utc::now().timestamp_millis() as u64 & u32::MAX as u64) as u32
}

impl EmbeddedStreamingDictation {
    async fn handle_notification(
        &mut self,
        inner: &Arc<Inner>,
        notification: &[u8],
    ) -> Result<bool, String> {
        let event = self
            .collector
            .handle_notification(notification)
            .map_err(|err| format!("嵌入式音频流式包解析失败: {err}"))?;
        self.handle_event(inner, event).await
    }

    async fn handle_event(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
    ) -> Result<bool, String> {
        match event {
            crate::embedded_audio::StreamingSessionEvent::Started { session_id } => {
                self.begin_session_if_needed(inner, session_id).await?;
                Ok(false)
            }
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => {
                let chunk_session_id = chunk.session_id;
                if embedded_streaming_chunk_is_asr_input(&chunk) {
                    self.begin_session_if_needed(inner, chunk.session_id)
                        .await?;
                    let session = self
                        .session
                        .as_mut()
                        .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
                    session.consume_streaming_pcm(inner, &chunk.pcm)?;
                } else {
                    log::info!(
                        "[coord] embedded audio streaming tail packet excluded from ASR after STOP (session_id={}, packet_sequence={}, pcm_bytes={})",
                        chunk.session_id,
                        chunk.packet_sequence,
                        chunk.pcm.len()
                    );
                    self.show_transcribing_after_stop(inner);
                }
                if let Some(expected_packet_count) = self.pending_stop_expected_packet_count {
                    if self.collector.inner().has_successful_complete_session() {
                        self.finish_streaming_session(
                            inner,
                            chunk_session_id,
                            expected_packet_count,
                        )
                        .await?;
                        self.terminal_received = true;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            crate::embedded_audio::StreamingSessionEvent::Stopped {
                session_id,
                expected_packet_count,
            } => {
                self.pending_stop_expected_packet_count = Some(expected_packet_count);
                self.show_transcribing_after_stop(inner);
                if self.collector.inner().has_successful_complete_session() {
                    self.finish_streaming_session(inner, session_id, expected_packet_count)
                        .await?;
                    self.terminal_received = true;
                    Ok(true)
                } else {
                    let stats = self.collector.inner().stats();
                    log::info!(
                        "[coord] embedded audio streaming stop received; waiting for tail packets (expected={}, received={}, missing={})",
                        expected_packet_count,
                        stats.received_packet_count,
                        stats.missing_packet_count
                    );
                    Ok(false)
                }
            }
            crate::embedded_audio::StreamingSessionEvent::Cancelled { session_id, .. } => {
                self.abort_streaming_session(inner, session_id, "嵌入式音频会话已取消");
                Err("嵌入式音频会话已取消".to_string())
            }
            crate::embedded_audio::StreamingSessionEvent::Error {
                session_id,
                error_code,
                ..
            } => {
                let message = format!("嵌入式音频会话错误: {error_code:?}");
                self.abort_streaming_session(inner, session_id, &message);
                Err(message)
            }
            crate::embedded_audio::StreamingSessionEvent::Ignored(reason) => {
                log::debug!("[coord] embedded audio streaming ignored packet: {reason:?}");
                Ok(false)
            }
        }
    }

    async fn begin_session_if_needed(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<(), String> {
        if self.session.is_some() {
            if self.embedded_session_id != Some(embedded_session_id) {
                return Err(format!(
                    "嵌入式音频流式 session 不一致: current={:?}, incoming={embedded_session_id}",
                    self.embedded_session_id
                ));
            }
            return Ok(());
        }

        self.embedded_session_id = Some(embedded_session_id);
        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        log::info!(
            "[coord] embedded audio streaming dictation started (embedded_session_id={embedded_session_id}, coordinator_session_id={}, asr={})",
            session.session_id,
            session.active_asr
        );
        self.session = Some(session);
        Ok(())
    }

    async fn finish_streaming_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        expected_packet_count: u16,
    ) -> Result<(), String> {
        if self.embedded_session_id != Some(embedded_session_id) {
            return Err(format!(
                "嵌入式音频停止包 session 不一致: current={:?}, incoming={embedded_session_id}",
                self.embedded_session_id
            ));
        }
        let stats = self.collector.inner().stats();
        if stats.received_pcm_bytes == 0 {
            self.abort_streaming_session(
                inner,
                embedded_session_id,
                "嵌入式音频会话没有可识别的 PCM 数据",
            );
            return Err("嵌入式音频会话没有可识别的 PCM 数据".to_string());
        }
        if stats.missing_packet_count > 0 {
            log::warn!(
                "[coord] embedded audio streaming stop with missing packets (expected={}, missing={:?})",
                expected_packet_count,
                stats.missing_packet_indices
            );
        }
        if stats.post_stop_packet_count > 0 {
            log::info!(
                "[coord] embedded audio streaming collected post-stop tail for diagnostics (tail_packets={}, tail_pcm_bytes={}, tail_duration={:.3}s, asr_pcm_bytes={})",
                stats.post_stop_packet_count,
                stats.post_stop_pcm_bytes,
                stats.post_stop_duration_seconds,
                stats.asr_boundary_pcm_bytes
            );
        }
        store_embedded_audio_stats(inner, stats.clone());
        self.show_transcribing_after_stop(inner);

        let session = self
            .session
            .take()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        let archive_active = session
            .archive_pcm
            .as_deref()
            .map(|pcm| archive_embedded_audio_if_enabled(inner, session.session_id, pcm))
            .unwrap_or(false);
        inner
            .audio_archive_active
            .store(archive_active, std::sync::atomic::Ordering::Relaxed);
        if session.boosted_chunk_count > 0 {
            log::info!(
                "[coord] embedded audio streaming normalized for ASR (boosted_chunks={}, max_gain={:.2}, clipped_samples={})",
                session.boosted_chunk_count,
                session.max_gain,
                session.clipped_samples
            );
        }
        log::info!(
            "[coord] embedded audio streaming submitted to dictation pipeline (asr={}, pcm_bytes={}, asr_pcm_bytes={}, archive={})",
            session.active_asr,
            session.streamed_pcm_bytes,
            session.normalized_pcm_bytes,
            archive_active
        );
        end_session(inner).await
    }

    async fn finish_pending_stop_after_capture(
        &mut self,
        inner: &Arc<Inner>,
    ) -> Result<(), String> {
        let embedded_session_id = self
            .embedded_session_id
            .ok_or_else(|| "嵌入式 BLE 流式会话尚未收到开始包".to_string())?;
        let expected_packet_count = self
            .pending_stop_expected_packet_count
            .or_else(|| {
                self.collector
                    .inner()
                    .stats()
                    .expected_packet_count
                    .and_then(|count| u16::try_from(count).ok())
            })
            .ok_or_else(|| "嵌入式 BLE 流式会话尚未收到结束包".to_string())?;
        self.finish_streaming_session(inner, embedded_session_id, expected_packet_count)
            .await?;
        self.terminal_received = true;
        Ok(())
    }

    fn abort_streaming_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        message: &str,
    ) {
        if self.embedded_session_id != Some(embedded_session_id) {
            return;
        }
        self.abort_active_session(inner, message);
    }

    fn abort_active_session(&mut self, inner: &Arc<Inner>, message: &str) {
        if let Some(session) = self.session.take() {
            cancel_asr_for_session(inner, session.session_id);
            restore_prepared_windows_ime_session(inner, session.session_id);
            set_phase_idle_if_session_matches(inner, session.session_id);
        }
        let elapsed = inner.state.lock().started_at.elapsed().as_millis() as u64;
        emit_capsule(
            inner,
            CapsuleState::Error,
            0.0,
            elapsed,
            Some(message.to_string()),
            None,
        );
        schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
        self.terminal_received = true;
    }

    fn show_transcribing_after_stop(&self, inner: &Arc<Inner>) {
        if self.session.is_some() {
            latch_embedded_audio_stop_feedback(inner);
            let elapsed = inner.state.lock().started_at.elapsed().as_millis() as u64;
            emit_capsule(
                inner,
                CapsuleState::Transcribing,
                0.0,
                elapsed,
                current_embedded_audio_partial_preview(inner),
                None,
            );
        }
    }

    fn into_submission_result(
        self,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        let collector = self.collector.into_inner();
        let stats = collector.stats();
        if !self.terminal_received || !stats.terminal_received {
            return Err("嵌入式音频流式会话尚未收到结束包".to_string());
        }
        if stats.end_reason != Some(crate::embedded_audio::SessionEndReason::Stop) {
            return Err(format!(
                "嵌入式音频流式会话未正常结束: {:?}",
                stats.end_reason
            ));
        }
        Ok(crate::embedded_audio::EmbeddedAudioSubmissionResult {
            reconstructed_pcm_bytes: stats.reconstructed_pcm_bytes,
            stats,
        })
    }

    fn into_cancelled_submission_result(
        self,
    ) -> crate::embedded_audio::EmbeddedAudioSubmissionResult {
        let collector = self.collector.into_inner();
        let mut stats = collector.stats();
        stats.end_reason = Some(crate::embedded_audio::SessionEndReason::Cancel);
        crate::embedded_audio::EmbeddedAudioSubmissionResult {
            reconstructed_pcm_bytes: stats.reconstructed_pcm_bytes,
            stats,
        }
    }
}

async fn begin_embedded_audio_dictation_session(
    inner: &Arc<Inner>,
) -> Result<EmbeddedAudioDictationSession, String> {
    let current_session_id = {
        let mut state = inner.state.lock();
        begin_session_state(&mut state, capture_focus_target(), capture_frontmost_app())
            .ok_or_else(|| "当前已有听写会话在运行，暂不能提交嵌入式音频".to_string())?
    };
    clear_embedded_audio_stats(inner);
    clear_embedded_audio_partial_preview(inner);
    clear_embedded_audio_stop_feedback(inner);
    #[cfg(target_os = "windows")]
    {
        let prepared = inner.windows_ime.prepare_session();
        let mut slots = inner.prepared_windows_ime_session.lock();
        store_prepared_windows_ime_session(&mut slots, current_session_id, prepared);
    }
    inner
        .translation_modifier_seen
        .store(false, Ordering::SeqCst);
    inner
        .audio_archive_active
        .store(false, std::sync::atomic::Ordering::Relaxed);
    emit_capsule(inner, CapsuleState::Recording, 0.0, 0, None, None);

    if let Err(message) = ensure_asr_credentials() {
        log::warn!("[coord] embedded audio ASR credential gate failed: {message}");
        emit_capsule(
            inner,
            CapsuleState::Error,
            0.0,
            0,
            Some(message.clone()),
            None,
        );
        restore_prepared_windows_ime_session(inner, current_session_id);
        inner.state.lock().phase = SessionPhase::Idle;
        schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
        return Err(message);
    }

    let active_asr = CredentialsVault::get_active_asr();
    let consumer =
        match build_embedded_audio_asr_consumer(inner, current_session_id, &active_asr).await {
            Ok(consumer) => consumer,
            Err(message) => {
                log::warn!("[coord] embedded audio ASR setup failed: {message}");
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some(message.clone()),
                    None,
                );
                restore_prepared_windows_ime_session(inner, current_session_id);
                cancel_asr_for_session(inner, current_session_id);
                inner.state.lock().phase = SessionPhase::Idle;
                schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                return Err(message);
            }
        };

    let archive_pcm = record_embedded_audio_for_debug_enabled(inner).then(Vec::new);
    Ok(EmbeddedAudioDictationSession {
        session_id: current_session_id,
        active_asr,
        consumer,
        archive_pcm,
        streamed_pcm_bytes: 0,
        normalized_pcm_bytes: 0,
        boosted_chunk_count: 0,
        max_gain: 1.0,
        clipped_samples: 0,
        asr_preroll_sent: false,
    })
}

fn activate_embedded_audio_dictation_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
    initial_level: f32,
) -> bool {
    {
        let mut state = inner.state.lock();
        if state.session_id != session_id || state.phase != SessionPhase::Starting {
            cancel_asr_for_session(inner, session_id);
            restore_prepared_windows_ime_session(inner, session_id);
            return false;
        }
        state.phase = SessionPhase::Listening;
    }

    emit_capsule(inner, CapsuleState::Recording, initial_level, 0, None, None);
    true
}

async fn submit_embedded_pcm_for_dictation_with_stats(
    inner: &Arc<Inner>,
    pcm: &[u8],
    stats: Option<crate::embedded_audio::SessionStats>,
) -> Result<(), String> {
    let mut session = begin_embedded_audio_dictation_session(inner).await?;
    let current_session_id = session.session_id;
    let active_asr = session.active_asr.clone();
    let consumer = Arc::clone(&session.consumer);

    let archive_active = archive_embedded_audio_if_enabled(inner, current_session_id, pcm);
    inner
        .audio_archive_active
        .store(archive_active, std::sync::atomic::Ordering::Relaxed);
    let (asr_pcm, gain_stats) = normalize_embedded_pcm_for_asr(pcm);
    if gain_stats.gain > 1.0 {
        log::info!(
            "[coord] embedded audio normalized for ASR (rms_before={:.1}, peak_before={}, gain={:.2}, clipped_samples={})",
            gain_stats.rms_before,
            gain_stats.peak_before,
            gain_stats.gain,
            gain_stats.clipped_samples
        );
    }

    if !activate_embedded_audio_dictation_session(
        inner,
        current_session_id,
        embedded_pcm_peak_level(&asr_pcm),
    ) {
        return Ok(());
    }
    feed_embedded_asr_preroll_if_needed(&mut session);
    for chunk in asr_pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
        consumer.consume_pcm_chunk(chunk);
    }
    log::info!(
        "[coord] embedded audio submitted to dictation pipeline (asr={active_asr}, pcm_bytes={}, asr_pcm_bytes={}, gain={:.2})",
        pcm.len(),
        asr_pcm.len(),
        gain_stats.gain
    );
    if let Some(stats) = stats {
        store_embedded_audio_stats(inner, stats);
    }

    end_session(inner).await
}

fn embedded_streaming_chunk_is_asr_input(chunk: &crate::embedded_audio::StreamingPcmChunk) -> bool {
    !chunk.after_stop_boundary
}

async fn build_embedded_audio_asr_consumer(
    inner: &Arc<Inner>,
    session_id: SessionId,
    active_asr: &str,
) -> Result<Arc<dyn crate::recorder::AudioConsumer>, String> {
    #[cfg(target_os = "windows")]
    if foundry::is_foundry_local_whisper(active_asr) {
        let prefs = inner.prefs.get();
        let model_alias = if foundry::model_alias_is_known(&prefs.foundry_local_asr_model) {
            prefs.foundry_local_asr_model.clone()
        } else {
            foundry::DEFAULT_MODEL_ALIAS.to_string()
        };
        let language_hint = prefs.foundry_local_asr_language_hint.trim().to_string();
        let language_hint = if language_hint.is_empty() {
            None
        } else {
            Some(language_hint)
        };
        let local = Arc::new(FoundryLocalWhisperAsr::new(
            Arc::clone(&inner.foundry_local_runtime),
            model_alias,
            prefs.foundry_local_runtime_source.clone(),
            language_hint,
        ));
        store_asr_for_session(
            inner,
            session_id,
            ActiveAsr::FoundryLocalWhisper(Arc::clone(&local)),
        );
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = local;
        return Ok(consumer);
    }

    if is_whisper_compatible_provider(active_asr) {
        let (api_key, base_url, model, proxy_config) =
            read_whisper_credentials().map_err(|err| err.to_string())?;
        let whisper_prompt =
            crate::asr::whisper::build_prompt_from_phrases(&enabled_phrases(inner));
        let client = http_client_builder_with_proxy(&base_url, 30, &proxy_config)
            .build()
            .map_err(|err| format!("build Whisper HTTP client failed: {err}"))?;
        let whisper = Arc::new(WhisperBatchASR::new_with_client(
            api_key,
            base_url,
            model,
            whisper_prompt,
            client,
        ));
        store_asr_for_session(inner, session_id, ActiveAsr::Whisper(Arc::clone(&whisper)));
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = whisper;
        return Ok(consumer);
    }

    if is_bailian_provider(active_asr) {
        let asr = Arc::new(BailianRealtimeASR::new(read_bailian_credentials()));
        asr.open_session()
            .await
            .map_err(|err| format!("打开 Bailian ASR 连接失败: {err}"))?;
        store_asr_for_session(inner, session_id, ActiveAsr::Bailian(Arc::clone(&asr)));
        let bridge = Arc::new(DeferredAsrBridge::new());
        let target: Arc<dyn crate::asr::AudioConsumer> = asr;
        bridge.attach(target);
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge;
        return Ok(consumer);
    }

    let asr = Arc::new(VolcengineStreamingASR::new(
        read_volc_credentials(),
        enabled_hotwords(inner),
    ));
    let inner_for_partial = Arc::clone(inner);
    asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
        update_embedded_audio_partial_preview(&inner_for_partial, session_id, text);
    })));
    asr.open_session()
        .await
        .map_err(|err| format!("打开火山 ASR 连接失败: {err}"))?;
    store_asr_for_session(inner, session_id, ActiveAsr::Volcengine(Arc::clone(&asr)));
    let bridge = Arc::new(DeferredAsrBridge::new());
    let target: Arc<dyn crate::asr::AudioConsumer> = asr;
    bridge.attach(target);
    let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge;
    Ok(consumer)
}

fn archive_embedded_audio_if_enabled(
    inner: &Arc<Inner>,
    session_id: SessionId,
    pcm: &[u8],
) -> bool {
    if !record_embedded_audio_for_debug_enabled(inner) {
        return false;
    }

    let prefs = inner.prefs.get();
    let _ = crate::persistence::prune_recordings(
        prefs.history_retention_days,
        prefs.audio_recording_max_entries,
    );
    let path = match crate::persistence::recording_path_for_session(&session_id.to_string()) {
        Ok(path) => path,
        Err(err) => {
            log::warn!("[coord] embedded audio archive path failed: {err}");
            return false;
        }
    };
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    let wav = crate::asr::wav::encode_wav_16k_mono(&samples);
    match fs::write(&path, wav) {
        Ok(()) => true,
        Err(err) => {
            log::warn!(
                "[coord] embedded audio archive write failed at {}: {err}",
                path.display()
            );
            false
        }
    }
}

fn record_embedded_audio_for_debug_enabled(inner: &Arc<Inner>) -> bool {
    inner.prefs.get().record_audio_for_debug
        || std::env::var("LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG")
            .map(|value| value == "1")
            .unwrap_or(false)
}

fn embedded_pcm_peak_level(pcm: &[u8]) -> f32 {
    let peak = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]).unsigned_abs())
        .max()
        .unwrap_or(0);
    (peak as f32 / i16::MAX as f32).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy)]
struct EmbeddedPcmGainStats {
    rms_before: f64,
    peak_before: u16,
    gain: f64,
    clipped_samples: usize,
}

fn normalize_embedded_pcm_for_asr(pcm: &[u8]) -> (Vec<u8>, EmbeddedPcmGainStats) {
    let (rms_before, peak_before) = embedded_pcm_rms_and_peak(pcm);
    let mut stats = EmbeddedPcmGainStats {
        rms_before,
        peak_before,
        gain: 1.0,
        clipped_samples: 0,
    };

    if rms_before <= 0.0 || rms_before >= EMBEDDED_AUDIO_TARGET_RMS {
        return (pcm.to_vec(), stats);
    }

    let gain = (EMBEDDED_AUDIO_TARGET_RMS / rms_before).min(EMBEDDED_AUDIO_MAX_GAIN);
    if gain < EMBEDDED_AUDIO_MIN_GAIN {
        return (pcm.to_vec(), stats);
    }

    let mut normalized = Vec::with_capacity(pcm.len());
    let mut clipped_samples = 0usize;
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        let scaled = (sample as f64 * gain).round();
        let clamped = scaled.clamp(i16::MIN as f64, i16::MAX as f64);
        if (scaled - clamped).abs() > f64::EPSILON {
            clipped_samples += 1;
        }
        normalized.extend_from_slice(&(clamped as i16).to_le_bytes());
    }

    stats.gain = gain;
    stats.clipped_samples = clipped_samples;
    (normalized, stats)
}

fn embedded_pcm_rms_and_peak(pcm: &[u8]) -> (f64, u16) {
    let mut sum_squares = 0.0f64;
    let mut sample_count = 0usize;
    let mut peak = 0u16;
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        let abs = sample.unsigned_abs();
        peak = peak.max(abs);
        sum_squares += (sample as f64) * (sample as f64);
        sample_count += 1;
    }
    if sample_count == 0 {
        (0.0, peak)
    } else {
        ((sum_squares / sample_count as f64).sqrt(), peak)
    }
}

pub(super) async fn end_session(inner: &Arc<Inner>) -> Result<(), String> {
    let current_session_id = {
        let mut state = inner.state.lock();
        let Some(session_id) = start_processing_if_listening(&mut state) else {
            return Ok(());
        };
        session_id
    };

    let elapsed = inner.state.lock().started_at.elapsed().as_millis() as u64;
    emit_capsule(
        inner,
        CapsuleState::Transcribing,
        0.0,
        elapsed,
        current_embedded_audio_partial_preview(inner),
        None,
    );

    if let Some(rec) = take_recorder_for_session(inner, current_session_id) {
        rec.stop();
        release_recording_mute(inner, "dictation");
    }

    let asr_opt = take_asr_for_session(inner, current_session_id);
    let asr = match asr_opt {
        Some(a) => a,
        None => {
            restore_prepared_windows_ime_session(inner, current_session_id);
            clear_embedded_audio_stats(inner);
            inner.state.lock().phase = SessionPhase::Idle;
            return Ok(());
        }
    };

    let uses_global_timeout = asr_transcribe_uses_global_timeout(&asr);
    let raw = match asr {
        ActiveAsr::Volcengine(asr) => {
            debug_assert!(uses_global_timeout);
            if let Err(e) = asr.send_last_frame().await {
                log::error!("[coord] send last frame failed: {e}");
            }
            // 添加全局超时保护：防止 await_final_result() 永远挂起
            let timeout_duration = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
            match tokio::time::timeout(timeout_duration, asr.await_final_result()).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::error!("[coord] await final failed: {e}");
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some(format!("识别失败: {e}")),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err(e.to_string());
                }
                Err(_) => {
                    // 全局超时：最后的防线
                    log::error!(
                        "[coord] 全局超时 {} 秒 - 强制恢复",
                        COORDINATOR_GLOBAL_TIMEOUT_SECS
                    );
                    // 清理 ASR session，避免资源泄漏
                    asr.cancel();
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some("识别超时".to_string()),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err("global timeout".to_string());
                }
            }
        }
        ActiveAsr::Whisper(w) => {
            debug_assert!(uses_global_timeout);
            // Whisper 也添加类似的超时保护
            let timeout_duration = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
            match tokio::time::timeout(timeout_duration, w.transcribe()).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::error!("[coord] whisper transcribe failed: {e}");
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some(format!("识别失败: {e}")),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err(e.to_string());
                }
                Err(_) => {
                    log::error!(
                        "[coord] whisper 全局超时 {} 秒",
                        COORDINATOR_GLOBAL_TIMEOUT_SECS
                    );
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some("识别超时".to_string()),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err("whisper global timeout".to_string());
                }
            }
        }
        ActiveAsr::Bailian(asr) => {
            debug_assert!(uses_global_timeout);
            if let Err(e) = asr.send_last_frame().await {
                log::error!("[coord] Bailian send last frame failed: {e}");
            }
            let timeout_duration = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
            match tokio::time::timeout(timeout_duration, asr.await_final_result()).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::error!("[coord] Bailian await final failed: {e}");
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some(format!("识别失败: {e}")),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err(e.to_string());
                }
                Err(_) => {
                    log::error!(
                        "[coord] Bailian 全局超时 {} 秒",
                        COORDINATOR_GLOBAL_TIMEOUT_SECS
                    );
                    asr.cancel();
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some("识别超时".to_string()),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err("bailian global timeout".to_string());
                }
            }
        }
        #[cfg(target_os = "windows")]
        ActiveAsr::FoundryLocalWhisper(local) => {
            debug_assert!(!uses_global_timeout);
            match local
                .transcribe(foundry_audio_transcribe_timeout_duration())
                .await
            {
                Ok(r) => {
                    schedule_foundry_local_asr_release(inner, current_session_id);
                    r
                }
                Err(e) => {
                    if inner.state.lock().cancelled {
                        log::info!(
                            "[coord] Foundry Local Whisper transcribe cancelled — discarding transcript"
                        );
                        schedule_foundry_local_asr_release(inner, current_session_id);
                        restore_prepared_windows_ime_session(inner, current_session_id);
                        set_phase_idle_if_session_matches(inner, current_session_id);
                        return Ok(());
                    }
                    log::error!("[coord] Foundry Local Whisper transcribe failed: {e:#}");
                    schedule_foundry_local_asr_release(inner, current_session_id);
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some(format!("本地识别失败: {e}")),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err(e.to_string());
                }
            }
        }
        #[cfg(target_os = "macos")]
        ActiveAsr::Local(local) => {
            debug_assert!(uses_global_timeout);
            // 缓存命中时 transcribe 不含 load 时间；冷启动 load 已在 build_local_qwen3
            // 提前完成。但 transcribe 本身受音频长度影响：用户实测 RTF ≈ 0.3，慢机
            // 可达 0.5；15s 固定超时在 ≥ 30s 录音上会把整段结果丢掉。改用动态
            // 超时 max(15, ceil(audio_s × 0.6) + 10)，公式与单测见
            // `local_qwen_transcribe_timeout`。
            let audio_secs = (local.buffer_duration_ms() as f64) / 1000.0;
            let timeout_duration = local_qwen_transcribe_timeout(audio_secs);
            log::info!(
                "[coord] local Qwen3-ASR transcribe: audio={:.2}s timeout={}s",
                audio_secs,
                timeout_duration.as_secs()
            );
            let result = tokio::time::timeout(timeout_duration, local.transcribe()).await;
            inner.local_asr_cache.touch();
            schedule_local_asr_release(inner);
            match result {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::error!("[coord] local Qwen3-ASR transcribe failed: {e:#}");
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some(format!("本地识别失败: {e}")),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err(e.to_string());
                }
                Err(_) => {
                    log::error!(
                        "[coord] local Qwen3-ASR 动态超时 {}s（音频 {:.2}s）",
                        timeout_duration.as_secs(),
                        audio_secs
                    );
                    emit_capsule(
                        inner,
                        CapsuleState::Error,
                        0.0,
                        elapsed,
                        Some("识别超时".to_string()),
                        None,
                    );
                    restore_prepared_windows_ime_session(inner, current_session_id);
                    inner.state.lock().phase = SessionPhase::Idle;
                    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
                    return Err("local global timeout".to_string());
                }
            }
        }
    };

    // ASR 完成后 cancel 检查：用户在 transcribe 进行中按 Esc 时，这里就会命中。
    // 优先级高于 empty 检查 — 用户取消 → 静默丢弃，不写失败历史也不弹错误胶囊。
    if inner.state.lock().cancelled {
        log::info!("[coord] cancel detected after ASR — discarding transcript");
        restore_prepared_windows_ime_session(inner, current_session_id);
        clear_embedded_audio_stats(inner);
        // PR #387 的「cancel 后清 focus_target」契约要在 Processing 路径上也成立。
        // cancel_session 在 Processing 阶段故意跳过 finish_cancel_session_state（让
        // 这里收尾），但此前的 end_session 没把 focus_target 清掉。logic-review
        // 2026-05-10 P3 (🚩) 把这条补完。
        {
            let mut state = inner.state.lock();
            state.phase = SessionPhase::Idle;
            state.focus_target = None;
        }
        return Ok(());
    }

    // ASR 返回空转写护栏（来自 PR #66）：写一条 emptyTranscript 失败历史 + 错误胶囊，
    // 与 main 上其它 error 路径保持一致（带 schedule_capsule_idle 让胶囊自动消失）。
    let mut raw = raw;

    #[cfg(any(debug_assertions, test))]
    if raw.text.trim().is_empty() {
        if let Some(debug_text) = debug_transcript_override_text() {
            log::info!(
                "[coord] using debug transcript override (chars={})",
                debug_text.chars().count()
            );
            raw.text = debug_text;
        }
    }

    if raw.text.trim().is_empty() {
        let session = DictationSession {
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339(),
            raw_transcript: raw.text.clone(),
            final_text: String::new(),
            mode: inner.prefs.get().default_mode,
            app_bundle_id: None,
            app_name: None,
            insert_status: InsertStatus::Failed,
            error_code: Some("emptyTranscript".to_string()),
            duration_ms: Some(raw.duration_ms),
            dictionary_entry_count: Some(enabled_phrases(inner).len() as u32),
            // empty-transcript（ASR 没识别到任何文字）也保留 wav 标记——这是用户最想
            // 通过原始录音定位"是不是麦克风太小声 / ASR 模型问题"的场景。修 pr_agent
            // "Missing Audio" 反馈。
            has_audio_recording: Some(inner.audio_archive_active.load(Ordering::Relaxed)),
            embedded_audio_stats: take_embedded_audio_stats(inner),
        };
        let prefs_snapshot = inner.prefs.get();
        if let Err(e) = inner.history.append_with_retention(
            session,
            prefs_snapshot.history_retention_days,
            prefs_snapshot.history_max_entries,
        ) {
            log::error!("[coord] history append failed: {e}");
        }
        emit_capsule(
            inner,
            CapsuleState::Error,
            0.0,
            elapsed,
            Some("没有识别到语音".to_string()),
            None,
        );
        restore_prepared_windows_ime_session(inner, current_session_id);
        inner.state.lock().phase = SessionPhase::Idle;
        schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
        return Err("ASR returned empty transcript".to_string());
    }

    let correction_rules = match inner.correction_rules.list() {
        Ok(rules) => rules,
        Err(e) => {
            log::warn!("[coord] load correction rules failed: {e}; continue without correction");
            Vec::new()
        }
    };
    let front_app = inner.state.lock().front_app.clone();
    if !correction_rules.is_empty() {
        let corrected = apply_correction_rules(&raw.text, &correction_rules);
        if corrected != raw.text {
            log::info!(
                "[coord] correction rules adjusted raw transcript ({} → {} chars)",
                raw.text.chars().count(),
                corrected.chars().count()
            );
            raw.text = corrected;
        }
    }
    emit_capsule(
        inner,
        CapsuleState::Polishing,
        0.0,
        elapsed,
        Some(raw.text.clone()),
        None,
    );

    let prefs = inner.prefs.get();
    let force_raw_output = std::env::var("LISTENER_TYPE_FORCE_RAW_OUTPUT")
        .map(|value| value == "1")
        .unwrap_or(false);
    let pack = if force_raw_output {
        log::info!("[coord] force raw output enabled by LISTENER_TYPE_FORCE_RAW_OUTPUT");
        crate::types::builtin_style_pack_for_mode(PolishMode::Raw)
    } else {
        match inner
            .style_packs
            .get_or_default_active(&prefs.active_style_pack_id)
        {
            Ok(pack) => pack,
            Err(error) => {
                log::warn!(
                    "[coord] active style pack unavailable, falling back to builtin light: {error}"
                );
                crate::types::builtin_style_pack_for_mode(PolishMode::Light)
            }
        }
    };
    let mode = pack.base_mode;
    let hotword_strs = enabled_phrases(inner);
    let working_languages = prefs.working_languages.clone();
    let chinese_script_preference = prefs.chinese_script_preference;
    let output_language_preference = prefs.output_language_preference;
    let llm_thinking_enabled = prefs.llm_thinking_enabled;
    let style_system_prompt = pack.prompt.clone();
    let raw_uses_llm = mode == PolishMode::Raw && super::raw_style_pack_uses_llm(&pack);
    let translation_target = prefs.translation_target_language.trim().to_string();
    let translation_active =
        inner.translation_modifier_seen.load(Ordering::SeqCst) && !translation_target.is_empty();
    log::info!(
        "[style-pack] runtime dispatch session_id={} active_pack={} kind={:?} mode={:?} raw_chars={} prompt_chars={} raw_uses_llm={} translation_active={} hotwords={} working_languages={:?}",
        current_session_id,
        pack.id,
        pack.kind,
        mode,
        raw.text.chars().count(),
        style_system_prompt.chars().count(),
        raw_uses_llm,
        translation_active,
        hotword_strs.len(),
        working_languages
    );
    // 对话感知 polish：拉最近 N 分钟的会话作为 LLM 上下文。仅在非翻译路径且非 Raw mode
    // 才有意义（Raw 不走 LLM、翻译走单轮独立 prompt）。窗口=0 时 prior_turns 是空 Vec，
    // polish 路径自动退化成单轮单消息——跟历史行为一致。
    let polish_context_window_minutes = prefs.polish_context_window_minutes;
    let prior_turns: Vec<(String, String)> = if !translation_active
        && (mode != PolishMode::Raw || raw_uses_llm)
        && polish_context_window_minutes > 0
    {
        match inner
            .history
            .recent_within_minutes(polish_context_window_minutes)
        {
            Ok(sessions) => sessions
                .into_iter()
                // 只取实际成功润色过的会话作为上下文：失败的会话 final_text 是 raw 兜底，
                // 喂回 LLM 会让模型以为"上一轮我什么都没做"——没意义且占 token。
                .filter(|s| s.error_code.is_none() && !s.final_text.trim().is_empty())
                .map(|s| (s.raw_transcript, s.final_text))
                .collect(),
            Err(e) => {
                log::warn!("[coord] fetch polish context failed: {e}; fall back to single-turn");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    // 流式插入 opt-in 路径：开关打开 + 非翻译 + 非 Raw 模式 → 进入流式分支。
    // 任何不满足都走原一次性 polish_or_passthrough 路径，行为跟历史完全一致。
    let wayland_session = crate::hotkey::is_wayland_session();
    let streaming_eligible = streaming_insert_eligible(
        prefs.streaming_insert,
        translation_active,
        mode,
        raw_uses_llm,
        wayland_session,
    );
    log::info!(
        "[coord] polish dispatch: translation={translation_active} mode={mode:?} wayland_session={wayland_session} streaming_eligible={streaming_eligible}"
    );

    let (polished, polish_error, already_streamed) = if translation_active {
        log::info!(
            "[coord] translation mode → target=\u{300C}{}\u{300D} working={:?} front_app={:?}",
            translation_target,
            working_languages,
            front_app
        );
        let (p, e) = translate_or_passthrough(
            &raw,
            &translation_target,
            &working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app.as_deref(),
        )
        .await;
        (p, e, false)
    } else if streaming_eligible {
        run_streaming_polish(
            inner,
            &raw,
            mode,
            &hotword_strs,
            &style_system_prompt,
            &working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app.as_deref(),
            &prior_turns,
        )
        .await
    } else {
        let (p, e) = polish_or_passthrough(
            &raw,
            mode,
            &hotword_strs,
            &style_system_prompt,
            &working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app.as_deref(),
            &prior_turns,
        )
        .await;
        (p, e, false)
    };

    let polished = finalize_polished_text(
        polished,
        translation_active,
        raw_uses_llm,
        mode,
        &polish_error,
        chinese_script_preference,
        &correction_rules,
        already_streamed,
    );
    emit_capsule(
        inner,
        CapsuleState::Polishing,
        0.0,
        elapsed,
        Some(polished.clone()),
        None,
    );
    // 原子化最后一次 cancel 检查 + 转 Inserting：
    // 在同一 lock 内决定「丢弃」还是「进入 Inserting」。一旦设到 Inserting，
    // cancel_session 就拒绝介入（Cmd+V 已发出，撤销不掉）。这是 audit HIGH #2 的修复，
    // 之前 check 与 inserter.insert 之间有窗口期。
    //
    // 流式路径例外：`already_streamed = true` 表示字符已经一边流一边落到光标了，
    // 撤销不掉。即使 cancel 旗在中途被立起来，也只能尊重「已经发生」的事实，进入
    // Inserting 状态完成 history / vocab 等收尾工作。
    let proceed_to_insert = {
        let mut state = inner.state.lock();
        if state.cancelled && !already_streamed {
            state.phase = SessionPhase::Idle;
            false
        } else {
            state.phase = SessionPhase::Inserting;
            true
        }
    };
    if !proceed_to_insert {
        log::info!(
            "[coord] cancel detected before insert — discarding output (chars={})",
            polished.chars().count()
        );
        restore_prepared_windows_ime_session(inner, current_session_id);
        return Ok(());
    }

    let focus_target = inner.state.lock().focus_target;
    let focus_ready_for_paste = restore_focus_target_if_possible(focus_target);
    let prefs = inner.prefs.get();
    let restore_clipboard = prefs.restore_clipboard_after_paste;
    let allow_non_tsf_insertion_fallback = prefs.allow_non_tsf_insertion_fallback;
    let allow_foreground_insert_fallback =
        std::env::var("LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK")
            .map(|value| value == "1")
            .unwrap_or(false);
    let paste_shortcut = prefs.paste_shortcut;
    // 流式路径下，字符已经通过 Unicode keystroke 落到光标处，跳过 inserter.insert。
    let status = if already_streamed {
        log::info!(
            "[coord] insertion skipped: {} chars already streamed via unicode_keystroke (polish_error={:?})",
            polished.chars().count(),
            polish_error
        );
        InsertStatus::Inserted
    } else if wayland_session {
        log::info!(
            "[coord] Wayland session detected; skipping synthetic paste and attempting copy-only fallback ({} chars)",
            polished.chars().count()
        );
        let status = inner.inserter.copy_fallback(&polished);
        match status {
            InsertStatus::CopiedFallback => {
                log::info!("[coord] Wayland copy-only fallback succeeded")
            }
            InsertStatus::Failed => {
                log::error!("[coord] Wayland copy-only fallback failed: clipboard write failed")
            }
            other => log::warn!(
                "[coord] Wayland copy-only fallback returned unexpected status: {other:?}"
            ),
        }
        status
    } else if focus_ready_for_paste {
        #[cfg(target_os = "windows")]
        {
            let ime_target = capture_ime_submit_target();
            insert_with_windows_ime_first(
                inner,
                current_session_id,
                &polished,
                restore_clipboard,
                allow_non_tsf_insertion_fallback,
                paste_shortcut,
                ime_target,
            )
            .await
        }
        #[cfg(not(target_os = "windows"))]
        {
            inner
                .inserter
                .insert(&polished, restore_clipboard, paste_shortcut)
        }
    } else if allow_foreground_insert_fallback {
        log::warn!(
            "[coord] original insertion target is not foreground; inserting into current foreground by LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK"
        );
        #[cfg(target_os = "windows")]
        {
            let ime_target = capture_ime_submit_target();
            insert_with_windows_ime_first(
                inner,
                current_session_id,
                &polished,
                restore_clipboard,
                allow_non_tsf_insertion_fallback,
                paste_shortcut,
                ime_target,
            )
            .await
        }
        #[cfg(not(target_os = "windows"))]
        {
            inner
                .inserter
                .insert(&polished, restore_clipboard, paste_shortcut)
        }
    } else {
        log::warn!(
            "[coord] original insertion target is not foreground; copied output without paste"
        );
        if allow_non_tsf_insertion_fallback {
            inner.inserter.copy_fallback(&polished)
        } else {
            InsertStatus::Failed
        }
    };
    restore_prepared_windows_ime_session(inner, current_session_id);
    let inserted_chars = polished.chars().count() as u32;

    // 累计每条 enabled 词条在最终文本中的命中次数。
    // 用 polished（最终插入的文本）扫描，与用户实际看到的输出一致。
    let total_hits: u64 = match inner.vocab.record_hits(&polished) {
        Ok(n) => n,
        Err(e) => {
            log::error!("[coord] record_hits failed: {e}");
            0
        }
    };
    // 词汇本页面在打开时通常需要立即看到 hits 增长，否则用户得手动切走再切回来才刷新。
    // 命中数 > 0 时通知前端：Vocab 页面订阅 vocab:updated 即时 listVocab() 重新加载。
    if total_hits > 0 {
        if let Some(app) = inner.app.lock().clone() {
            let _ = app.emit("vocab:updated", total_hits);
        }
    }

    // polish 失败时在 history 里标记 polishFailed，让用户能在历史详情看到为什么这次输出
    // 不是预期的 mode 风格。即使失败也不丢词 — final_text 仍是原文（保留"用户的话不丢"语义）。
    let error_code = dictation_error_code(
        status,
        polish_error.is_some(),
        focus_ready_for_paste,
        allow_non_tsf_insertion_fallback,
        wayland_session,
    )
    .map(str::to_string);
    let tsf_required_insert_failed = error_code.as_deref() == Some("windowsImeTsfRequired");

    // 与 coordinator 内部 SessionId 对齐：方便 recorder 旁路写盘的 `<session_id>.wav`
    // 跟 history 这条 DictationSession.id 同名，前端凭 id 就能找到对应录音文件。
    let history_session_id = current_session_id.to_string();
    let history_created_at = Utc::now().to_rfc3339();
    let prefs_snapshot = inner.prefs.get();
    let session = DictationSession {
        id: history_session_id.clone(),
        created_at: history_created_at.clone(),
        raw_transcript: raw.text.clone(),
        final_text: polished.clone(),
        mode,
        app_bundle_id: None,
        app_name: None,
        insert_status: status,
        error_code,
        duration_ms: Some(raw.duration_ms),
        // 历史详情页的"X 个热词"显示：用本次实际命中次数（每个匹配实例算一次），
        // 比"启用词条总数"更能反映本段口述命中了多少。u64 → u32 截断对单段听写足够。
        dictionary_entry_count: Some(total_hits.min(u32::MAX as u64) as u32),
        // 用 begin_session 时 Recorder::start 返回的实际写盘状态，而不是 prefs 开关——
        // 开关打开但路径创建失败时这里是 false，避免前端渲染播放按钮后端 404。
        has_audio_recording: Some(inner.audio_archive_active.load(Ordering::Relaxed)),
        embedded_audio_stats: take_embedded_audio_stats(inner),
    };
    if let Err(e) = inner.history.append_with_retention(
        session,
        prefs_snapshot.history_retention_days,
        prefs_snapshot.history_max_entries,
    ) {
        log::error!("[coord] history append failed: {e}");
    }
    let done_message = if status == InsertStatus::Inserted
        && !polish_error.is_some()
        && !tsf_required_insert_failed
        && !wayland_session
    {
        Some(polished.clone())
    } else if tsf_required_insert_failed {
        Some("TSF 未上屏，已禁止非 TSF 兜底".to_string())
    } else if wayland_session {
        wayland_done_message(status, polish_error.is_some())
    } else {
        default_done_message(status, polish_error.is_some())
    };

    emit_capsule(
        inner,
        CapsuleState::Done,
        0.0,
        elapsed,
        done_message,
        Some(inserted_chars),
    );

    {
        let mut state = inner.state.lock();
        state.phase = SessionPhase::Idle;
        state.focus_target = None;
    }
    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);

    Ok(())
}

pub(super) fn dictation_error_code(
    status: InsertStatus,
    polish_failed: bool,
    focus_ready_for_paste: bool,
    allow_non_tsf_insertion_fallback: bool,
    wayland_session: bool,
) -> Option<&'static str> {
    if wayland_session && status == InsertStatus::Failed {
        Some("waylandClipboardWriteFailed")
    } else if !focus_ready_for_paste && status == InsertStatus::Failed {
        Some("focusRestoreFailed")
    } else if cfg!(target_os = "windows")
        && focus_ready_for_paste
        && !allow_non_tsf_insertion_fallback
        && status == InsertStatus::Failed
    {
        Some("windowsImeTsfRequired")
    } else if polish_failed {
        Some("polishFailed")
    } else {
        None
    }
}

pub(super) fn cancel_session(inner: &Arc<Inner>) {
    let Some(decision) = ({
        let mut state = inner.state.lock();
        let phase = state.phase;
        let decision = begin_cancel_session_state(&mut state);
        if phase == SessionPhase::Inserting {
            log::info!("[coord] cancel ignored — already in Inserting phase, can't undo paste");
        }
        decision
    }) else {
        return;
    };

    stop_recorder_for_session(inner, decision.session_id);
    cancel_asr_for_session(inner, decision.session_id);
    restore_prepared_windows_ime_session(inner, decision.session_id);
    if request_embedded_ble_capture_cancel(inner) {
        log::info!("[coord] embedded BLE capture cancel requested");
    }
    // Processing 阶段保持 phase=Processing 让 end_session 自己走完检查 + 收尾；
    // 其他阶段直接转 Idle。
    if decision.phase != SessionPhase::Processing {
        let mut state = inner.state.lock();
        finish_cancel_session_state(&mut state, decision);
    }
    emit_capsule(inner, CapsuleState::Cancelled, 0.0, 0, None, None);
    log::info!("[coord] session cancelled (was {:?})", decision.phase);
    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS);
}

fn append_typed_prefix(target: &mut String, delta: &str, typed_chars: usize) -> usize {
    let mut end = 0;
    let mut appended = 0;
    for (idx, ch) in delta.char_indices().take(typed_chars) {
        end = idx + ch.len_utf8();
        appended += 1;
    }
    target.push_str(&delta[..end]);
    appended
}

#[cfg(test)]
mod tests {
    use super::{
        append_typed_prefix, cancel_session, clear_embedded_ble_cancel_flag, default_done_message,
        dictation_error_code, embedded_ble_stream_idle_timeout, embedded_pcm_rms_and_peak,
        embedded_streaming_chunk_is_asr_input, finalize_polished_text,
        normalize_embedded_pcm_for_asr, prepare_embedded_streaming_pcm_for_asr,
        register_embedded_ble_cancel_flag, streaming_insert_eligible, wayland_done_message,
        EmbeddedStreamingDictation, EMBEDDED_AUDIO_ASR_PREROLL_BYTES,
        EMBEDDED_AUDIO_ASR_PREROLL_MS, EMBEDDED_AUDIO_FEED_CHUNK_BYTES,
    };
    use crate::coordinator::Coordinator;
    use crate::coordinator_state::SessionPhase;
    use crate::embedded_audio::StreamingPcmChunk;
    use crate::types::{ChineseScriptPreference, CorrectionRule, InsertStatus, PolishMode};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn correction_rule(pattern: &str, replacement: &str) -> CorrectionRule {
        CorrectionRule {
            id: "test".into(),
            pattern: pattern.into(),
            replacement: replacement.into(),
            enabled: true,
            created_at: String::new(),
        }
    }

    fn pcm_from_samples(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn cancel_session_requests_registered_embedded_ble_capture_cancel() {
        let coordinator = Coordinator::new();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
        {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        cancel_session(&coordinator.inner);

        assert!(cancel_flag.load(Ordering::SeqCst));
    }

    #[test]
    fn embedded_ble_cancel_registration_only_clears_matching_flag() {
        let coordinator = Coordinator::new();
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));

        register_embedded_ble_cancel_flag(&coordinator.inner, &first);
        register_embedded_ble_cancel_flag(&coordinator.inner, &second);
        clear_embedded_ble_cancel_flag(&coordinator.inner, &first);
        assert!(Arc::ptr_eq(
            coordinator
                .inner
                .embedded_ble_cancel_flag
                .lock()
                .as_ref()
                .expect("second flag remains registered"),
            &second
        ));

        clear_embedded_ble_cancel_flag(&coordinator.inner, &second);
        assert!(coordinator.inner.embedded_ble_cancel_flag.lock().is_none());
    }

    #[test]
    fn caller_cancelled_embedded_ble_stream_returns_cancel_result() {
        let result = EmbeddedStreamingDictation::default().into_cancelled_submission_result();

        assert_eq!(
            result.stats.end_reason,
            Some(crate::embedded_audio::SessionEndReason::Cancel)
        );
        assert_eq!(result.reconstructed_pcm_bytes, 0);
    }

    #[test]
    fn embedded_ble_background_stream_has_no_idle_timeout() {
        let timeout = std::time::Duration::from_secs(120);

        assert_eq!(
            embedded_ble_stream_idle_timeout(timeout, true),
            Some(timeout)
        );
        assert_eq!(embedded_ble_stream_idle_timeout(timeout, false), None);
    }

    #[test]
    fn embedded_streaming_tail_chunk_is_not_asr_input() {
        let before_stop = StreamingPcmChunk {
            session_id: 1,
            packet_sequence: 0,
            pcm: vec![1, 2],
            after_stop_boundary: false,
        };
        let after_stop = StreamingPcmChunk {
            session_id: 1,
            packet_sequence: 1,
            pcm: vec![3, 4],
            after_stop_boundary: true,
        };

        assert!(embedded_streaming_chunk_is_asr_input(&before_stop));
        assert!(!embedded_streaming_chunk_is_asr_input(&after_stop));
    }

    #[test]
    fn streamed_output_skips_postprocessing_mutations() {
        let rules = vec![correction_rule("Open AI", "OpenAI")];

        let result = finalize_polished_text(
            "Open AI".into(),
            false,
            false,
            PolishMode::Raw,
            &None,
            ChineseScriptPreference::Auto,
            &rules,
            true,
        );

        assert_eq!(result, "Open AI");
    }

    #[test]
    fn raw_llm_output_still_applies_script_preference() {
        let result = finalize_polished_text(
            "繁體".into(),
            false,
            true,
            PolishMode::Raw,
            &None,
            ChineseScriptPreference::Simplified,
            &[],
            false,
        );

        assert_eq!(result, "繁体");
    }

    #[test]
    fn non_streamed_output_still_applies_correction_rules() {
        let rules = vec![correction_rule("Open AI", "OpenAI")];

        let result = finalize_polished_text(
            "Open AI".into(),
            false,
            false,
            PolishMode::Raw,
            &None,
            ChineseScriptPreference::Auto,
            &rules,
            false,
        );

        assert_eq!(result, "OpenAI");
    }

    #[test]
    fn append_typed_prefix_keeps_unicode_char_boundaries() {
        let mut typed = String::from("前");

        let appended = append_typed_prefix(&mut typed, "a你🙂b", 3);

        assert_eq!(appended, 3);
        assert_eq!(typed, "前a你🙂");
    }

    #[test]
    fn append_typed_prefix_caps_at_delta_length() {
        let mut typed = String::new();

        let appended = append_typed_prefix(&mut typed, "好", 10);

        assert_eq!(appended, 1);
        assert_eq!(typed, "好");
    }

    #[test]
    fn wayland_disables_streaming_insert_even_when_pref_enabled() {
        assert!(!streaming_insert_eligible(
            true,
            false,
            PolishMode::Light,
            false,
            true
        ));
    }

    #[test]
    fn x11_linux_can_still_use_streaming_insert_when_other_gates_pass() {
        assert!(streaming_insert_eligible(
            true,
            false,
            PolishMode::Light,
            false,
            false
        ));
    }

    #[test]
    fn wayland_done_message_tells_user_manual_paste_is_required() {
        assert_eq!(
            wayland_done_message(InsertStatus::CopiedFallback, false),
            Some("Wayland 未启用自动输入，已复制到剪贴板，请手动粘贴".to_string())
        );
        assert_eq!(
            wayland_done_message(InsertStatus::CopiedFallback, true),
            Some("Wayland 未启用自动输入，已复制原文到剪贴板，请手动粘贴".to_string())
        );
        assert_eq!(
            wayland_done_message(InsertStatus::Failed, false),
            Some("Wayland 未启用自动输入，剪贴板写入失败".to_string())
        );
    }

    #[test]
    fn default_done_message_keeps_existing_non_wayland_behavior() {
        assert_eq!(
            default_done_message(InsertStatus::PasteSent, false),
            Some("已尝试粘贴".to_string())
        );
        assert_eq!(
            default_done_message(InsertStatus::Inserted, true),
            Some("润色失败，已插入原文".to_string())
        );
    }

    #[test]
    fn wayland_clipboard_failure_uses_specific_error_code() {
        assert_eq!(
            dictation_error_code(InsertStatus::Failed, false, false, true, true),
            Some("waylandClipboardWriteFailed")
        );
    }

    #[test]
    fn embedded_pcm_normalization_boosts_low_rms_despite_single_peak() {
        let mut samples = vec![500i16; 999];
        samples.push(i16::MAX);
        let pcm = pcm_from_samples(&samples);

        let (normalized, stats) = normalize_embedded_pcm_for_asr(&pcm);
        let (rms_after, _) = embedded_pcm_rms_and_peak(&normalized);

        assert_eq!(normalized.len(), pcm.len());
        assert!(stats.gain > 1.5, "gain={}", stats.gain);
        assert!(stats.clipped_samples > 0);
        assert!(rms_after > stats.rms_before);
    }

    #[test]
    fn embedded_pcm_normalization_leaves_loud_audio_unchanged() {
        let pcm = pcm_from_samples(&vec![3_000i16; 256]);

        let (normalized, stats) = normalize_embedded_pcm_for_asr(&pcm);

        assert_eq!(stats.gain, 1.0);
        assert_eq!(normalized, pcm);
    }

    #[test]
    fn volcengine_streaming_keeps_ble_chunks_unmodified() {
        let pcm = pcm_from_samples(&[100, -100, 80, -80]);

        let (prepared, stats) = prepare_embedded_streaming_pcm_for_asr("volcengine", &pcm);

        assert_eq!(prepared, pcm);
        assert_eq!(stats.gain, 1.0);
    }

    #[test]
    fn embedded_asr_preroll_is_frame_aligned() {
        assert_eq!(EMBEDDED_AUDIO_ASR_PREROLL_MS, 800);
        assert_eq!(EMBEDDED_AUDIO_ASR_PREROLL_BYTES, 25_600);
        assert_eq!(
            EMBEDDED_AUDIO_ASR_PREROLL_BYTES % EMBEDDED_AUDIO_FEED_CHUNK_BYTES,
            0
        );
    }
}

fn prepare_embedded_streaming_pcm_for_asr(
    active_asr: &str,
    pcm: &[u8],
) -> (Vec<u8>, EmbeddedPcmGainStats) {
    let (rms_before, peak_before) = embedded_pcm_rms_and_peak(pcm);
    let stats = EmbeddedPcmGainStats {
        rms_before,
        peak_before,
        gain: 1.0,
        clipped_samples: 0,
    };

    if active_asr == "volcengine" {
        return (pcm.to_vec(), stats);
    }

    normalize_embedded_pcm_for_asr(pcm)
}
