// Wake candidate, speaker gate, streaming polish helpers.
// Included into `coordinator::dictation` via `include!`.

impl EmbeddedAudioDictationSession {
    fn consume_streaming_pcm(&mut self, inner: &Arc<Inner>, pcm: &[u8]) -> Result<(), String> {
        if pcm.is_empty() {
            return Ok(());
        }
        if pcm.len() % 2 != 0 {
            return Err("嵌入式音频 PCM chunk 长度不是 16-bit 对齐".to_string());
        }

        if embedded_audio_stop_feedback_latched(inner) {
            if !embedded_audio_streaming_session_accepts_pcm(inner, self.session_id) {
                log::debug!(
                    "[coord] embedded audio streaming PCM ignored for inactive dictation session ({})",
                    self.session_id
                );
                return Ok(());
            }
        } else {
            if !emit_embedded_audio_pcm_capsule_if_active(
                inner,
                self.session_id,
                CapsuleState::Recording,
                embedded_pcm_visual_level(pcm),
            ) {
                log::debug!(
                    "[coord] embedded audio streaming PCM ignored for inactive dictation session ({})",
                    self.session_id
                );
                return Ok(());
            }
        }

        if let Some(archive_pcm) = self.archive_pcm.as_mut() {
            archive_pcm.extend_from_slice(pcm);
        }

        self.streamed_pcm_bytes += pcm.len();
        self.streaming_pcm_buffer.extend_from_slice(pcm);
        self.consume_ready_streaming_pcm_blocks();
        Ok(())
    }

    fn flush_streaming_pcm(&mut self) {
        if self.streaming_pcm_buffer.is_empty() {
            return;
        }

        let trailing_pcm = std::mem::take(&mut self.streaming_pcm_buffer);
        self.consume_prepared_streaming_pcm(&trailing_pcm);
    }

    fn consume_ready_streaming_pcm_blocks(&mut self) {
        let ready_bytes = self.streaming_pcm_buffer.len() / EMBEDDED_AUDIO_FEED_CHUNK_BYTES
            * EMBEDDED_AUDIO_FEED_CHUNK_BYTES;
        if ready_bytes == 0 {
            return;
        }

        let trailing_pcm = self.streaming_pcm_buffer.split_off(ready_bytes);
        let ready_pcm = std::mem::replace(&mut self.streaming_pcm_buffer, trailing_pcm);
        for pcm_block in ready_pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            self.consume_prepared_streaming_pcm(pcm_block);
        }
    }

    fn consume_prepared_streaming_pcm(&mut self, pcm: &[u8]) {
        let source_pcm_offset_ms = (self.normalized_pcm_bytes as u64) / 32;
        let (asr_pcm, gain_stats) = self.prepare_streaming_pcm_for_asr(pcm);
        if self.active_asr == "volcengine"
            && embedded_streaming_chunk_has_speech_energy(
                gain_stats.rms_before,
                gain_stats.peak_before,
            )
        {
            self.streaming_agc
                .first_voiced_pcm_ms
                .get_or_insert(source_pcm_offset_ms);
        }

        // 改A: track sustained trailing silence AFTER the body has started so the
        // caller can request a host-initiated device stop early. Leading silence
        // (before the user speaks the dictation body) and post-stop tails never
        // count toward the threshold.
        let chunk_ms = (pcm.len() / 32) as u64;
        let (signal_rms, signal_peak) = embedded_pcm_streaming_agc_signal_level(pcm);
        if embedded_streaming_chunk_has_speech_energy(signal_rms, signal_peak) {
            self.proactive_stop_body_started = true;
            self.proactive_stop_silence_ms = 0;
        } else if self.proactive_stop_body_started {
            self.proactive_stop_silence_ms = self
                .proactive_stop_silence_ms
                .saturating_add(chunk_ms);
        }

        self.normalized_pcm_bytes += asr_pcm.len();
        self.consumer.consume_pcm_chunk(&asr_pcm);
    }

    fn prepare_streaming_pcm_for_asr(&mut self, pcm: &[u8]) -> (Vec<u8>, EmbeddedPcmGainStats) {
        if self.active_asr == "volcengine" {
            // The buffer is bounded to one established provider feed interval.
            return normalize_embedded_streaming_pcm_for_asr(pcm, &mut self.streaming_agc);
        }

        normalize_embedded_pcm_for_asr(pcm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferedSpeakerCandidateKind {
    Enrollment,
    Verification,
    Rejected,
}

const HIDDEN_AUTOMATIC_CANDIDATE_NONE: u8 = 0;
const HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE: u8 = 1;
const HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED: u8 = 2;
static HIDDEN_AUTOMATIC_CANDIDATE_STATE: AtomicU8 = AtomicU8::new(HIDDEN_AUTOMATIC_CANDIDATE_NONE);
static WAKE_DIAGNOSTIC_CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(0);

fn save_bounded_wake_diagnostic(embedded_session_id: u32, outcome: &'static str, pcm: &[u8]) {
    // Prefer explicit env; otherwise always keep a small rolling ring under LocalAppData
    // so owner wake misses can be inspected without re-running with special flags.
    let directory = std::env::var(WAKE_DIAGNOSTIC_DIR_ENV).unwrap_or_else(|_| {
        let base = std::env::var("LOCALAPPDATA")
            .or_else(|_| std::env::var("APPDATA"))
            .unwrap_or_else(|_| ".".to_string());
        std::path::Path::new(&base)
            .join("Listener Type")
            .join("Logs")
            .join("wake-diag-live")
            .to_string_lossy()
            .into_owned()
    });
    if directory.trim().is_empty() {
        return;
    }
    let Ok(index) =
        WAKE_DIAGNOSTIC_CAPTURE_COUNT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            (current < WAKE_DIAGNOSTIC_MAX_CANDIDATES).then_some(current + 1)
        })
    else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    if let Err(err) = fs::create_dir_all(&directory) {
        log::warn!("[wake-phrase] diagnostic directory unavailable: {err}");
        return;
    }
    let pcm_len = pcm.len().min(WAKE_DIAGNOSTIC_MAX_PCM_BYTES) & !1usize;
    let pcm = &pcm[..pcm_len];
    let mut wav = Vec::with_capacity(44 + pcm.len());
    let data_size = pcm.len() as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32.saturating_add(data_size)).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    let path = directory.join(format!(
        "wake-candidate-{index:02}-session-{embedded_session_id}-{outcome}.wav"
    ));
    match fs::write(&path, wav) {
        Ok(()) => log::info!(
            "[wake-phrase] bounded local diagnostic saved index={} embedded_session_id={} outcome={} pcm_ms={}",
            index,
            embedded_session_id,
            outcome,
            pcm.len() / 32
        ),
        Err(err) => log::warn!("[wake-phrase] diagnostic WAV write failed: {err}"),
    }
}

fn mark_hidden_automatic_candidate_active() {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE.store(HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE, Ordering::SeqCst);
}

fn clear_hidden_automatic_candidate() {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE.store(HIDDEN_AUTOMATIC_CANDIDATE_NONE, Ordering::SeqCst);
}

pub(super) fn hidden_automatic_candidate_active() -> bool {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE.load(Ordering::SeqCst) == HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE
}

pub(super) fn request_hidden_automatic_candidate_promotion() -> bool {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE
        .compare_exchange(
            HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE,
            HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn take_hidden_automatic_candidate_promotion() -> bool {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE
        .compare_exchange(
            HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED,
            HIDDEN_AUTOMATIC_CANDIDATE_NONE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn discard_pre_press_candidate_pcm(pcm: &mut Vec<u8>) -> usize {
    let discarded_pcm_bytes = pcm.len();
    pcm.clear();
    discarded_pcm_bytes
}

fn buffered_speaker_candidate_kind(
    start_origin: crate::embedded_audio::SessionStartOrigin,
    enrollment_armed: bool,
    enrolled: bool,
) -> Option<BufferedSpeakerCandidateKind> {
    if enrollment_armed {
        return Some(BufferedSpeakerCandidateKind::Enrollment);
    }
    match start_origin {
        crate::embedded_audio::SessionStartOrigin::User => None,
        // Automatic wake always enters the Verification gate path.
        // `speaker_verification::verify` already open-gates when no voiceprint is
        // enrolled (phrase hit alone accepts). Rejecting here when !enrolled made
        // "delete voiceprint" permanently disable wake — opposite of product intent.
        crate::embedded_audio::SessionStartOrigin::VoiceActivation => {
            let _ = enrolled;
            Some(BufferedSpeakerCandidateKind::Verification)
        }
        crate::embedded_audio::SessionStartOrigin::Unknown(_) => {
            Some(BufferedSpeakerCandidateKind::Rejected)
        }
    }
}

struct BufferedSpeakerCandidate {
    kind: BufferedSpeakerCandidateKind,
    pcm: Vec<u8>,
    wake_detector: Option<crate::wake_phrase::StreamingDetector>,
    pending_phrase_match: Option<PendingAutomaticPhraseMatch>,
    #[cfg(target_os = "windows")]
    local_confirmation_task:
        Option<tauri::async_runtime::JoinHandle<Result<LocalWakeConfirmation, String>>>,
    local_confirmation_attempts: usize,
    local_confirmation_last_snapshot_bytes: usize,
    kws_fed_bytes: usize,
    kws_total_ms: u64,
    started_at: Instant,
}

struct PendingAutomaticPhraseMatch {
    wake_match: crate::wake_phrase::Match,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    local_confirmation_ms: u64,
    owner_verification_start_ms: usize,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct LocalWakeConfirmation {
    matched: bool,
    phrase_relation: crate::wake_phrase::LocalPhraseRelation,
    transcript_chars: usize,
    inference_ms: u64,
    snapshot_pcm_ms: usize,
}

const MAX_BUFFERED_SPEAKER_CANDIDATE_BYTES: usize = 2_100_000;
const STREAMING_KWS_FEED_BATCH_BYTES: usize = 1_600;
/// Proactive trailing-silence stop (改A) — DISABLED. A fixed energy-silence
/// threshold cannot distinguish a mid-sentence pause from a real
/// end-of-utterance, so any value that beats the firmware `auto_stop_silence`
/// timeout (observed 3-14s) truncates speech. User acceptance 2026-07-26:
/// "话还没说完就结束了". Set well above the firmware's max auto_stop so the
/// device's own endpointer always wins and this path stays dormant. The proper
/// fix is a content-aware endpoint (ASR sentence boundary) and/or device-side
/// wake word detection — see the 治本 plan. Do not lower this again until one of
/// those gates the dispatch.
const EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS: u64 = 30_000;
const OWNER_VERIFICATION_START_MS: usize = 1_100;
const OWNER_VERIFICATION_START_BYTES: usize = OWNER_VERIFICATION_START_MS * 32;
const OWNER_VERIFICATION_SNAPSHOT_MS: [usize; 3] = [OWNER_VERIFICATION_START_MS, 1_800, 2_400];
const LOCAL_CONFIRMATION_START_MS: usize =
    denzic_voice_activation_v1_core::DEFAULT_LOCAL_CONFIRMATION_START_MS as usize;
const LOCAL_CONFIRMATION_START_BYTES: usize = LOCAL_CONFIRMATION_START_MS * 32;
const LOCAL_CONFIRMATION_SNAPSHOT_MS: [usize; 6] = [
    LOCAL_CONFIRMATION_START_MS,
    2_400,
    3_000,
    5_000,
    8_000,
    12_000,
];

fn owner_verification_window_ready(pcm_bytes: usize) -> bool {
    // No enrolled voiceprint → phrase hit alone is enough; do not stall for the
    // 1.1s owner speech window (that delay only exists for embedding quality).
    if !crate::speaker_verification::is_enrolled() {
        return true;
    }
    pcm_bytes >= OWNER_VERIFICATION_START_BYTES
}

fn next_owner_verification_retry_ms(pcm_ms: usize) -> Option<usize> {
    OWNER_VERIFICATION_SNAPSHOT_MS
        .iter()
        .copied()
        .find(|snapshot_ms| *snapshot_ms > pcm_ms)
}

fn next_local_confirmation_snapshot_bytes(attempts: usize) -> Option<usize> {
    LOCAL_CONFIRMATION_SNAPSHOT_MS
        .get(attempts)
        .map(|milliseconds| milliseconds * 32)
}

#[cfg(target_os = "windows")]
fn spawn_local_wake_confirmation(
    _inner: &Arc<Inner>,
    pcm: Vec<u8>,
    phrase: String,
) -> tauri::async_runtime::JoinHandle<Result<LocalWakeConfirmation, String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let snapshot_pcm_ms = pcm.len() / 32;
        let result = crate::asr::local::wake_helper::confirm(&pcm, &phrase, Duration::from_secs(4))
            .map_err(|err| format!("local wake confirmation failed: {err}"))?;
        Ok(LocalWakeConfirmation {
            matched: result.matched,
            phrase_relation: result.phrase_relation,
            transcript_chars: result.transcript_chars,
            inference_ms: result
                .inference_ms
                .max(started.elapsed().as_millis() as u64),
            snapshot_pcm_ms,
        })
    })
}

#[derive(Default)]
struct EmbeddedStreamingDictation {
    collector: crate::embedded_audio::StreamingSessionCollector,
    session: Option<EmbeddedAudioDictationSession>,
    speaker_candidate: Option<BufferedSpeakerCandidate>,
    embedded_session_id: Option<u32>,
    transcript: Option<crate::embedded_audio::EmbeddedAudioTranscriptResult>,
    pending_stop_expected_packet_count: Option<u16>,
    terminal_received: bool,
    keep_listening_after_pipeline_errors: bool,
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
        // polish 失败仍写 history error_code，但录音原文已成功落地时不要把整个
        // dictation 呈现成失败；否则无效 LLM key 会让成功录音看起来像回退。
        match status {
            InsertStatus::Inserted => None,
            InsertStatus::PasteSent => Some("已尝试粘贴原文".to_string()),
            InsertStatus::CopiedFallback => Some(if cfg!(target_os = "windows") {
                "已复制原文，请 Ctrl+V".to_string()
            } else {
                "已复制原文，请粘贴".to_string()
            }),
            InsertStatus::Failed => Some("润色不可用，插入失败".to_string()),
        }
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

fn device_processing_final_succeeded(status: InsertStatus, error_code: Option<&str>) -> bool {
    if status == InsertStatus::Failed {
        return false;
    }
    matches!(error_code, None | Some("polishFailed"))
}
