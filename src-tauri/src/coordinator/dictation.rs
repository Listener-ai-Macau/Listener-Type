use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::coordinator_state::{
    apply_dictation_event, request_stop_during_starting_state, DictationEvent, DictationTransition,
    DictationUiState,
};
use crate::correction::apply_correction_rules;
use crate::types::{
    ChineseScriptPreference, HotkeyMode, InsertStatus, OutputLanguagePreference, PostDictationKey,
    ShortcutBinding, UserPreferences,
};

use super::qa::handle_qa_option_edge;
use super::recording_gate::{self, RecordIntent};
use super::resources::*;
use super::*;

/// 同一个 hotkey 边沿之间的最小间隔。低于此阈值的连按整体作为误触丢弃 ——
/// 避免微动开关回弹 / 用户手抖双击造成的空转写报错和 ASR session 抢资源。
pub(super) const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(250);
const EMBEDDED_AUDIO_FEED_CHUNK_BYTES: usize = 3_200;
const EMBEDDED_AUDIO_HOST_LIMITER_PEAK: f64 = i16::MAX as f64 * 0.707_945_784;
const EMBEDDED_AUDIO_VISUAL_RMS_REFERENCE: f64 = 700.0;
// Firmware AFE owns adaptive gain. Type keeps speech-energy telemetry but may
// only attenuate blocks that exceed the -3 dBFS host safety ceiling.
const EMBEDDED_AUDIO_STREAMING_SPEECH_RMS: f64 = 120.0;
const EMBEDDED_AUDIO_STREAMING_QUIET_SPEECH_RMS: f64 = 45.0;
const EMBEDDED_AUDIO_STREAMING_QUIET_SPEECH_PEAK: u16 = 256;
const EMBEDDED_AUDIO_STREAMING_AGC_SIGNAL_PERCENTILE_NUMERATOR: usize = 99;
const EMBEDDED_AUDIO_STREAMING_AGC_SIGNAL_PERCENTILE_DENOMINATOR: usize = 100;
const EMBEDDED_BLE_PCM_EVENT_TRACE_PACKET_INTERVAL: u16 = 50;
const EMBEDDED_BLE_READY_CAPSULE_MESSAGE: &str = "Listener BLE 已连接，等待设备开始录音。";
const DEVICE_AI_PROCESSING_MIN_VISIBLE_MS: u64 = 750;
const DEVICE_AI_PROCESSING_MAX_VISIBLE_MS: u64 = 5_000;
const EMBEDDED_BLE_STATS_ONLY_ENV: &str = "LISTENER_TYPE_EMBEDDED_BLE_STATS_ONLY";
const EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV: &str =
    "LISTENER_TYPE_DISABLE_EMBEDDED_BLE_PROCESSING_SYNC";
const EMBEDDED_BLE_CONTROL_START_SIGNAL_ENV: &str =
    "LISTENER_TYPE_EMBEDDED_BLE_CONTROL_START_SIGNAL";
const EMBEDDED_BLE_CONTROL_STOP_SIGNAL_ENV: &str = "LISTENER_TYPE_EMBEDDED_BLE_CONTROL_STOP_SIGNAL";
const WAKE_DIAGNOSTIC_DIR_ENV: &str = "LISTENER_WAKE_DIAGNOSTIC_DIR";
const WAKE_DIAGNOSTIC_MAX_CANDIDATES: usize = 128;
const WAKE_DIAGNOSTIC_MAX_PCM_BYTES: usize = 12 * 16_000 * 2;
const WAKE_DIAGNOSTIC_RETENTION_MAX_FILES: usize = 128;
const WAKE_DIAGNOSTIC_RETENTION_MAX_BYTES: u64 = 32 * 1024 * 1024;
const WAKE_DIAGNOSTIC_RETENTION_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const POST_DICTATION_KEY_DELAY: Duration = Duration::from_millis(60);
const EMBEDDED_ASR_SPEECH_ACTIVITY_TIMEOUT: Duration = Duration::from_millis(300);
// Owner dictation endpoint. Once body speech has started, every preview shape
// uses the established 1.0s inactivity contract. Do not make completion depend
// on optimistic punctuation, body length, or an uncertain voiceprint vote: the
// installed 1.0.5 ladder (1.5/2.0/2.5s) made the same spoken ending complete at
// different speeds. Only the wake/target speaker's latest speech refreshes this
// clock, so other people talking still cannot lengthen auto-end.
const EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS: u64 = 1_000;
// A settled-text timer shares the async runtime with BLE and ASR callbacks.
// Arm it slightly before the public one-second endpoint so ordinary Windows
// scheduling jitter still dispatches at about 1.0 s (live 577 measured
// 1,141 ms from a nominal 1,000 ms timer; live 578 lost the race to firmware).
// The stop reason and provider/audio-clock policy remain the one-second
// contract; only this wall-clock wake-up receives the scheduling allowance.
const EMBEDDED_SETTLED_TARGET_WALL_CLOCK_MS: u64 = 900;
// Installed session 72519330: wake capsule → ~1.2s host auto-end on the wake
// clock with empty body → "没有识别到语音". Initial body wait is only 700ms, so
// 1.0s snappy endpoint after that treats "thinking after wake" as done. Keep
// 1.0s once body text exists; before body, require a longer abandon silence.
const EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS: u64 = 3_000;
const EMBEDDED_PROVIDER_STALL_FALLBACK_LAG_MS: u64 = 500;
// 500ms confirmed stalls still mid-cut live speech when the cloud clock freezes
// for one network blip. Require a full second of no provider coverage growth
// before local-clock fallback may end the session.
const EMBEDDED_PROVIDER_STALL_CONFIRM_MS: u64 = 1_000;
// A provider may stabilize an old utterance several seconds after the owner
// stopped. Treating that late bookkeeping update as live speech refreshes the
// firmware's two-second safety timer and makes completion feel randomly slow.
// Real preview growth refreshes the local owner clock at the current capture
// edge, so a 600 ms alignment window retains live keepalives without allowing
// a stale two-pass boundary to extend recording.
const EMBEDDED_LIVE_OWNER_ACTIVITY_ALIGNMENT_MS: u64 = 600;

include!("dictation_endpoint_clock.rs");

fn preview_ends_with_sentence_terminal(preview: Option<&str>) -> bool {
    let Some(text) = preview.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    text.chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
        .is_some_and(|ch| matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | '…'))
}

fn target_speaker_inactive_stop_reason(timeout_ms: u64) -> &'static str {
    if timeout_ms >= EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS {
        "target_speaker_inactive_no_body_3000ms"
    } else {
        "target_speaker_inactive_1000ms"
    }
}
// The wake phrase is a complete activation command: after the capsule becomes
// visible, give the owner a full three seconds to begin the body. The first
// non-empty body preview ends this wait immediately, after which the exact
// 1000 ms owner-inactivity endpoint remains unchanged.
const EMBEDDED_AUTOMATIC_BODY_INITIAL_WAIT_MS: u64 = 3_000;
const EMBEDDED_TERMINAL_WAKE_CONTINUATION_TTL: Duration = Duration::from_secs(6);
const EMBEDDED_LOCAL_SPEECH_ALIGNMENT_SLACK_MS: u64 = 200;
// The local speaker verifier runs on overlapping windows and reports roughly
// every 400 ms. Its classified audio edge therefore legitimately trails the
// newest VAD speech edge by one cadence. Installed multi-speaker session 1831
// had a confirmed NonTarget edge at 13.9 s while the live speech clock was
// already near 14.3 s; the former 100 ms allowance treated that sustained room
// speaker as unclassified owner speech and disabled provider-stall auto-end.
// Keep this below two verifier cadences so stale evidence still expires.
const EMBEDDED_LOCAL_SPEAKER_CLASSIFICATION_SLACK_MS: u64 = 600;
// F4（2026-08-09 12:47:04）：旁人连续说话时未归属本地语音不断前进，会把
// 自动结束无限挂起。挂起以最后一次归属语音 +6s 封顶；本人正常说话的分类
// 滞后远小于 6s，不受影响。
const EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS: u64 = 2_000;
static EMBEDDED_ASR_SPEECH_ACTIVITY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn should_restore_clipboard_after_dictation(
    prefs: &UserPreferences,
    final_retention_applies: bool,
) -> bool {
    prefs.restore_clipboard_after_paste && !final_retention_applies
}

fn should_send_post_dictation_key(
    enabled: bool,
    key: PostDictationKey,
    status: InsertStatus,
    has_nonempty_final_text: bool,
    original_target_restored: bool,
    clipboard_retention_satisfied: bool,
    translation_active: bool,
) -> Option<ShortcutBinding> {
    if !enabled
        || !has_nonempty_final_text
        || !original_target_restored
        || !clipboard_retention_satisfied
        || translation_active
        || status != InsertStatus::Inserted
    {
        return None;
    }

    Some(match key {
        PostDictationKey::Enter => ShortcutBinding {
            primary: "Enter".to_string(),
            modifiers: Vec::new(),
        },
        PostDictationKey::CtrlEnter => ShortcutBinding {
            primary: "Enter".to_string(),
            modifiers: vec!["ctrl".to_string()],
        },
    })
}

fn claim_post_dictation_key(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let mut state = inner.state.lock();
    if state.session_id != session_id || state.post_dictation_key_claimed {
        return false;
    }
    state.post_dictation_key_claimed = true;
    true
}

struct FoundryLanguageHintSelection {
    hint: Option<String>,
    source: &'static str,
}

fn foundry_language_hint_from_preferences(prefs: &UserPreferences) -> FoundryLanguageHintSelection {
    let explicit = prefs.foundry_local_asr_language_hint.trim();
    if !explicit.is_empty() {
        return FoundryLanguageHintSelection {
            hint: Some(explicit.to_string()),
            source: "explicit",
        };
    }

    if let Some(hint) = foundry_language_hint_for_output_language(prefs.output_language_preference)
    {
        return FoundryLanguageHintSelection {
            hint: Some(hint.to_string()),
            source: "output_language_preference",
        };
    }

    if matches!(
        prefs.chinese_script_preference,
        ChineseScriptPreference::Simplified | ChineseScriptPreference::Traditional
    ) {
        return FoundryLanguageHintSelection {
            hint: Some("zh".to_string()),
            source: "chinese_script_preference",
        };
    }

    for language in &prefs.working_languages {
        if let Some(hint) = foundry_language_hint_for_working_language(language) {
            return FoundryLanguageHintSelection {
                hint: Some(hint.to_string()),
                source: "working_languages",
            };
        }
    }

    FoundryLanguageHintSelection {
        hint: None,
        source: "auto",
    }
}

fn foundry_language_hint_for_output_language(
    preference: OutputLanguagePreference,
) -> Option<&'static str> {
    match preference {
        OutputLanguagePreference::ZhCn | OutputLanguagePreference::ZhTw => Some("zh"),
        OutputLanguagePreference::En => Some("en"),
        OutputLanguagePreference::Ja => Some("ja"),
        OutputLanguagePreference::Ko => Some("ko"),
        OutputLanguagePreference::Auto => None,
    }
}

fn foundry_language_hint_for_working_language(language: &str) -> Option<&'static str> {
    let normalized = language.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    if language.contains("中文")
        || language.contains("汉语")
        || language.contains("漢語")
        || language.contains("简体")
        || language.contains("簡體")
        || language.contains("繁体")
        || language.contains("繁體")
        || normalized == "zh"
        || normalized.starts_with("zh-")
        || normalized.contains("chinese")
    {
        return Some("zh");
    }

    if normalized == "en" || normalized.starts_with("en-") || normalized.contains("english") {
        return Some("en");
    }

    if language.contains("日本")
        || language.contains("日语")
        || language.contains("日語")
        || normalized == "ja"
        || normalized.starts_with("ja-")
        || normalized.contains("japanese")
    {
        return Some("ja");
    }

    if language.contains("한국")
        || language.contains("韩语")
        || language.contains("韓語")
        || normalized == "ko"
        || normalized.starts_with("ko-")
        || normalized.contains("korean")
    {
        return Some("ko");
    }

    None
}

fn dictation_asr_engine_backend_id(active_asr: &str) -> &'static str {
    if crate::asr::local::is_local_qwen3(active_asr) {
        "local-qwen3"
    } else if foundry::is_foundry_local_whisper(active_asr) {
        "foundry-local-whisper"
    } else if is_bailian_provider(active_asr) {
        "bailian"
    } else if is_whisper_compatible_provider(active_asr) {
        "whisper-compatible"
    } else {
        "volcengine"
    }
}

fn dictation_asr_engine_label(active_asr: &str) -> String {
    match dictation_asr_engine_backend_id(active_asr) {
        "volcengine" => "Volcengine".to_string(),
        "local-qwen3" => "Local Qwen3-ASR".to_string(),
        "foundry-local-whisper" => "Foundry Local Whisper".to_string(),
        "bailian" => "Bailian".to_string(),
        "whisper-compatible" => format!("Whisper-compatible ({})", active_asr.trim()),
        _ => active_asr.trim().to_string(),
    }
}

fn dictation_asr_uses_core_accurate_engine(active_asr: &str) -> bool {
    dictation_asr_engine_backend_id(active_asr) == "volcengine"
}

fn dictation_asr_quality_warning(active_asr: &str) -> Option<String> {
    if dictation_asr_uses_core_accurate_engine(active_asr) {
        return None;
    }
    Some(format!(
        "当前识别引擎为{}，不是核心 Volcengine 准确引擎，识别可能不准。",
        dictation_asr_engine_label(active_asr)
    ))
}

fn log_dictation_asr_engine_selection(session_id: SessionId, active_asr: &str) {
    let backend = dictation_asr_engine_backend_id(active_asr);
    let label = dictation_asr_engine_label(active_asr);
    let core_accurate = dictation_asr_uses_core_accurate_engine(active_asr);
    log::info!(
        "[coord] dictation ASR engine selected: session_id={session_id} provider={} backend={backend} label=\"{label}\" core_accurate={core_accurate}",
        active_asr.trim()
    );
    if !core_accurate {
        log::warn!(
            "[coord] dictation ASR non-core accuracy warning: session_id={session_id} provider={} backend={backend}",
            active_asr.trim()
        );
    }
}

fn note_embedded_asr_speech_activity(inner: &Arc<Inner>, session_id: SessionId) {
    if !device_ai_processing_io_allowed()
        || !embedded_ble_host_recording_control_context_active(inner)
        || embedded_audio_stop_feedback_latched(inner)
    {
        return;
    }
    let active = {
        let state = inner.state.lock();
        state.session_id == session_id
            && matches!(
                state.phase,
                SessionPhase::Starting | SessionPhase::Listening
            )
    };
    if !active
        || EMBEDDED_ASR_SPEECH_ACTIVITY_IN_FLIGHT
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }

    async_runtime::spawn_blocking(move || {
        let result = crate::embedded_ble::send_recording_control_speech_activity(
            EMBEDDED_ASR_SPEECH_ACTIVITY_TIMEOUT,
        );
        EMBEDDED_ASR_SPEECH_ACTIVITY_IN_FLIGHT.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => log::debug!(
                "[embedded-ble] recognized speech refreshed auto-stop timeout session_id={session_id}"
            ),
            Err(err) => log::warn!(
                "[embedded-ble] recognized speech refresh failed session_id={session_id}: {err}"
            ),
        }
    });
}

/// endpoint 触发时的预热润色：用当前预览文本提前发起 LLM 流式润色，与 ASR
/// 终稿等待并行。被采用时首字提前 ~0.4-0.6s；终稿与预热输入不一致则取消
/// 丢弃、走正常路径（delta 只进缓冲，绝不上屏，丢弃对外不可见）。
fn maybe_start_polish_prefetch(inner: &Arc<Inner>, session_id: SessionId) {
    let prefs = inner.prefs.get();
    if !prefs.streaming_insert || inner.translation_modifier_seen.load(Ordering::SeqCst) {
        return;
    }
    if std::env::var("LISTENER_TYPE_FORCE_RAW_OUTPUT")
        .map(|value| value == "1")
        .unwrap_or(false)
    {
        return;
    }
    let pack = match inner
        .style_packs
        .get_or_default_active(&prefs.active_style_pack_id)
    {
        Ok(pack) => pack,
        Err(_) => return,
    };
    let mode = pack.base_mode;
    let raw_uses_llm = mode == PolishMode::Raw && super::raw_style_pack_uses_llm(&pack);
    if mode == PolishMode::Raw && !raw_uses_llm {
        return;
    }
    let auth_blocked = current_llm_auth_fingerprint()
        .ok()
        .is_some_and(current_llm_auth_is_rejected);
    if auth_blocked || llm_stall_circuit_open() {
        return;
    }
    let Some(preview) = current_embedded_audio_partial_preview(inner) else {
        return;
    };
    let preview = preview.trim().to_string();
    if preview.is_empty() {
        return;
    }
    // 与完成路径同序的确定性变换，最大化终稿一致率。
    let correction_rules = inner.correction_rules.list().unwrap_or_default();
    let input = apply_correction_rules(&preview, &correction_rules);
    let prior_turns: Vec<(String, String)> = if prefs.polish_context_window_minutes > 0 {
        inner
            .history
            .recent_within_minutes(prefs.polish_context_window_minutes)
            .map(|sessions| {
                sessions
                    .into_iter()
                    .filter(|s| s.error_code.is_none() && !s.final_text.trim().is_empty())
                    .map(|s| (s.raw_transcript, s.final_text))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let hotwords = enabled_phrases(inner);
    let working_languages = prefs.working_languages.clone();
    let chinese_script_preference = prefs.chinese_script_preference;
    let output_language_preference = prefs.output_language_preference;
    let llm_thinking_enabled = prefs.llm_thinking_enabled;
    let front_app = inner.state.lock().front_app.clone();
    let style_system_prompt = pack.prompt.clone();

    let prefetch = PolishPrefetch {
        input: input.clone(),
        buf: Arc::new(Mutex::new(PolishPrefetchBuf::default())),
        notify: Arc::new(tokio::sync::Notify::new()),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let buf = Arc::clone(&prefetch.buf);
    let notify = Arc::clone(&prefetch.notify);
    let cancel = Arc::clone(&prefetch.cancel);
    let result_buf = Arc::clone(&prefetch.buf);
    let result_notify = Arc::clone(&prefetch.notify);
    let should_cancel = {
        let inner = Arc::clone(inner);
        move || cancel.load(Ordering::SeqCst) || inner.state.lock().cancelled
    };
    let raw = RawTranscript {
        text: input.clone(),
        duration_ms: 0,
    };
    async_runtime::spawn(async move {
        let outcome = super::polish_or_passthrough_streaming(
            &raw,
            mode,
            &hotwords,
            &style_system_prompt,
            &working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app.as_deref(),
            &prior_turns,
            move |delta: &str| {
                buf.lock().chunks.push_back(delta.to_string());
                notify.notify_one();
            },
            should_cancel,
        )
        .await;
        let outcome = match outcome {
            super::StreamingPolishOutcome::UnsupportedFallback => {
                super::StreamingPolishOutcome::Failed("prefetch unsupported".to_string())
            }
            other => other,
        };
        result_buf.lock().result = Some(outcome);
        result_notify.notify_one();
    });
    // 同会话只留一份预热；覆盖旧槽前先取消（防泄漏在后台跑满 8s 空转）。
    if let Some((_, old)) = inner.polish_prefetch.lock().replace((session_id, prefetch)) {
        old.cancel.store(true, Ordering::SeqCst);
    }
    log::info!(
        "[coord] polish prefetch started session_id={session_id} input_chars={}",
        input.chars().count()
    );
}

fn take_polish_prefetch(inner: &Arc<Inner>, session_id: SessionId) -> Option<PolishPrefetch> {
    match inner.polish_prefetch.lock().take() {
        Some((id, prefetch)) if id == session_id => Some(prefetch),
        Some((_, prefetch)) => {
            prefetch.cancel.store(true, Ordering::SeqCst);
            None
        }
        None => None,
    }
}

/// 采用条件：终稿与预热输入逐字一致且预热流未失败。
fn polish_prefetch_adoptable(prefetch: &PolishPrefetch, final_text: &str) -> bool {
    prefetch.input == final_text && !prefetch.failed()
}

fn handle_target_speaker_update(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    update: crate::asr::volcengine::TargetSpeakerUpdate,
    settled_wall_clock_due: bool,
) {
    let session_active = {
        let state = inner.state.lock();
        state.session_id == session_id
            && !state.cancelled
            && matches!(
                state.phase,
                SessionPhase::Starting | SessionPhase::Listening
            )
    };
    if !session_active {
        return;
    }
    if update.target_activity_advanced || update.pending_activity_advanced {
        if target_speaker_update_has_live_owner_activity(&update) {
            note_embedded_asr_speech_activity(inner, session_id);
        } else {
            log::info!(
                "[asr] stale attributed activity did not refresh firmware endpoint provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} stable_attributed_end_ms={:?}",
                update.provider_audio_duration_ms,
                update.audio_duration_ms,
                update.target_speech_end_ms,
                update.local_target_speech_end_ms,
                update.stable_attributed_speech_end_ms,
            );
        }
    }
    let preview = current_embedded_audio_partial_preview(inner);
    // Prefer the live filtered preview if present; wake guard body_started is
    // the durable latch once any non-empty body was seen this session.
    let body_started = automatic_wake_body_started(inner, session_id)
        || preview
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty());
    let mode_timeout_ms = target_speaker_end_timeout_ms_for_preview(preview.as_deref());
    let mode_endpoint_timeout_ms = if body_started {
        mode_timeout_ms
    } else if automatic_wake_session_active(inner, session_id) {
        EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS.max(mode_timeout_ms)
    } else {
        mode_timeout_ms
    };
    let fusion_state = target_speaker_fusion_state(&update);
    let endpoint_timeout_ms =
        target_speaker_endpoint_timeout_with_fusion(fusion_state, mode_endpoint_timeout_ms);
    let stop_reason = target_speaker_inactive_stop_reason(endpoint_timeout_ms);
    let initial_body_wait_active =
        automatic_wake_initial_body_wait_active(inner, session_id, update.audio_duration_ms);
    let provider_stall_confirmed =
        provider_progress_stalled(inner, session_id, &update, Instant::now());
    let provider_clock_endpoint_due = target_speaker_endpoint_due_with_provider_stall(
        &update,
        provider_stall_confirmed,
        endpoint_timeout_ms,
    );
    let settled_wall_clock_endpoint_due = body_started && settled_wall_clock_due;
    let endpoint_due = !initial_body_wait_active
        && (provider_clock_endpoint_due || settled_wall_clock_endpoint_due);
    if !endpoint_due
        || stop_dispatched
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }

    let provider_stall_fallback =
        provider_stall_local_endpoint_due(&update, provider_stall_confirmed, endpoint_timeout_ms);
    if settled_wall_clock_endpoint_due && !provider_clock_endpoint_due {
        log::info!(
            "[asr] target endpoint using settled-text wall clock provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} timeout_ms={endpoint_timeout_ms}",
            update.provider_audio_duration_ms,
            update.audio_duration_ms,
            update.target_speech_end_ms,
            update.local_target_speech_end_ms
        );
    }
    if provider_stall_fallback {
        log::info!(
            "[asr] target endpoint using bounded provider-stall fallback provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} timeout_ms={endpoint_timeout_ms}",
            update.provider_audio_duration_ms,
            update.audio_duration_ms,
            update.target_speech_end_ms,
            update.local_target_speech_end_ms
        );
    }

    // 1.0.4 A3: latch Transcribing + keep last preview immediately at the
    // silence threshold so the UI does not hang on Listening while BLE stop
    // and final-frame work are still in flight.
    let stop_feedback_started = Instant::now();
    let feedback_emitted = request_embedded_audio_stop_feedback(inner, stop_reason);
    // 预热润色：与 stop/终稿并行发起，终稿一致则采用，首字提前 ~0.4-0.6s。
    maybe_start_polish_prefetch(inner, session_id);
    if feedback_emitted {
        log::info!(
            "[asr] stop_to_transcribing_ms={} session_id={session_id} reason={stop_reason} timeout_ms={endpoint_timeout_ms} body_started={body_started} sentence_pause={} semantic_continuation={} fusion_state={fusion_state:?}",
            stop_feedback_started.elapsed().as_millis(),
            preview_ends_with_sentence_terminal(preview.as_deref()),
            preview_has_dangling_continuation(preview.as_deref()),
        );
    }

    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let early_final_asr = clone_volcengine_asr_for_session(&inner, session_id);
    async_runtime::spawn(async move {
        let stop_future = request_embedded_ble_recording_stop_from_host(&inner, stop_reason);
        let finalization_future = async move {
            let Some(asr) = early_final_asr else {
                return;
            };
            let started = Instant::now();
            match asr.send_last_frame().await {
                Ok(()) => log::info!(
                    "[asr] proactive endpoint final frame sent session_id={session_id} provider_stall_fallback={provider_stall_fallback} settled_wall_clock_fallback={settled_wall_clock_endpoint_due} elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
                Err(err) => log::warn!(
                    "[asr] proactive endpoint final frame failed session_id={session_id} provider_stall_fallback={provider_stall_fallback} settled_wall_clock_fallback={settled_wall_clock_endpoint_due} elapsed_ms={} error={err}",
                    started.elapsed().as_millis()
                ),
            }
        };
        let (stop_result, ()) = tokio::join!(stop_future, finalization_future);
        match stop_result {
            Ok(true) => log::info!(
                "[embedded-ble] target-speaker auto-stop sent session_id={session_id} reason={stop_reason}"
            ),
            Ok(false) => {
                // A transient Starting/Listening ownership race must not burn
                // the one-shot endpoint latch forever. A later provider/local
                // update may retry while the same session is still active.
                stop_dispatched.store(false, Ordering::SeqCst);
                log::info!(
                    "[embedded-ble] target-speaker auto-stop not dispatched; retry armed session_id={session_id} reason={stop_reason}"
                );
            }
            Err(err) => {
                stop_dispatched.store(false, Ordering::SeqCst);
                log::warn!(
                    "[embedded-ble] target-speaker auto-stop failed; retry armed session_id={session_id} reason={stop_reason}: {err}"
                );
            }
        }
    });
}

fn target_speaker_update_has_live_owner_activity(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> bool {
    if !update.target_activity_advanced && !update.pending_activity_advanced {
        return false;
    }
    let audio_edge_ms = update
        .audio_duration_ms
        .max(update.provider_audio_duration_ms);
    let owner_edge_ms = update
        .target_speech_end_ms
        .max(update.local_target_speech_end_ms)
        .max(update.stable_attributed_speech_end_ms);
    audio_edge_ms
        .zip(owner_edge_ms)
        .is_some_and(|(audio, owner)| {
            audio.saturating_sub(owner) <= EMBEDDED_LIVE_OWNER_ACTIVITY_ALIGNMENT_MS
        })
}
fn target_speaker_endpoint_due(update: &crate::asr::volcengine::TargetSpeakerUpdate) -> bool {
    target_speaker_endpoint_due_with_timeout(update, EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS)
}

fn target_speaker_endpoint_due_with_timeout(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    endpoint_timeout_ms: u64,
) -> bool {
    target_speaker_endpoint_due_with_provider_stall(update, false, endpoint_timeout_ms)
}

fn local_speech_confidently_non_target(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    local_speech_ms: u64,
) -> bool {
    update
        .local_non_target_speech_end_ms
        .is_some_and(|non_target_ms| {
            non_target_ms.saturating_add(EMBEDDED_LOCAL_SPEAKER_CLASSIFICATION_SLACK_MS)
                >= local_speech_ms
        })
}

/// The owner was confirmed earlier, then the newest speech-energy window moved
/// beyond that local Target boundary without becoming a confirmed NonTarget.
/// This is identity uncertainty, not evidence that the owner stopped talking.
/// Keep the hold bounded at two seconds; explicit other-speaker evidence never
/// enters this branch.
fn has_uncertain_owner_identity_tail(update: &crate::asr::volcengine::TargetSpeakerUpdate) -> bool {
    if !update.local_speaker_tracking_enabled {
        return false;
    }
    update
        .local_target_speech_end_ms
        .zip(update.local_speech_end_ms)
        .is_some_and(|(target_ms, speech_ms)| {
            speech_ms > target_ms.saturating_add(EMBEDDED_LOCAL_SPEECH_ALIGNMENT_SLACK_MS)
                && speech_ms
                    <= target_ms.saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
                && !local_speech_confidently_non_target(update, speech_ms)
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetSpeakerFusionState {
    /// Provider or local owner evidence advanced on this update.
    OwnerContinuing,
    /// Speech continued after the last confirmed owner boundary, but neither
    /// local nor cloud evidence identified it as another person.
    UncertainOwnerTail,
    /// Strong local identity evidence says the newest speech is another person.
    ConfirmedOther,
    /// No current continuation evidence; ordinary silence endpoint applies.
    Quiet,
}

fn target_speaker_fusion_state(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> TargetSpeakerFusionState {
    let latest_local_speech_is_other = update
        .local_speech_end_ms
        .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms));
    // Explicit other-speaker evidence wins over provider growth because cloud
    // diarization may collapse two simultaneous speakers into the owner row.
    if latest_local_speech_is_other {
        return TargetSpeakerFusionState::ConfirmedOther;
    }
    if update.target_activity_advanced || update.pending_activity_advanced {
        return TargetSpeakerFusionState::OwnerContinuing;
    }
    if has_uncertain_owner_identity_tail(update) {
        return TargetSpeakerFusionState::UncertainOwnerTail;
    }
    TargetSpeakerFusionState::Quiet
}

fn target_speaker_endpoint_timeout_with_fusion(
    _fusion_state: TargetSpeakerFusionState,
    mode_timeout_ms: u64,
) -> u64 {
    mode_timeout_ms
}

/// Recent local speech energy that is not yet attributed as non-target. Used to
/// hold auto-end while the owner may still be talking even if cloud text froze.
/// The uncertain energy clock is capped from the last *confirmed owner*
/// boundary, not from any attributed speaker. This preserves a natural pause
/// while preventing weak room speech from repeatedly extending the session.
fn has_unresolved_recent_local_speech(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    endpoint_timeout_ms: u64,
) -> bool {
    update
        .audio_duration_ms
        .zip(update.local_speech_end_ms)
        .is_some_and(|(audio_ms, local_speech_ms)| {
            let confirmed_owner_end_ms = update
                .target_speech_end_ms
                .into_iter()
                .chain(update.local_target_speech_end_ms)
                .max()
                .unwrap_or_default();
            confirmed_owner_end_ms > 0
                && local_speech_ms
                    > confirmed_owner_end_ms
                        .saturating_add(EMBEDDED_LOCAL_SPEECH_ALIGNMENT_SLACK_MS)
                && local_speech_ms
                    <= confirmed_owner_end_ms
                        .saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
                && !local_speech_confidently_non_target(update, local_speech_ms)
                && audio_ms.saturating_sub(local_speech_ms) < endpoint_timeout_ms
        })
}

fn target_speaker_endpoint_due_with_provider_stall(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    provider_stall_confirmed: bool,
    endpoint_timeout_ms: u64,
) -> bool {
    let unresolved_recent_local_speech =
        has_unresolved_recent_local_speech(update, endpoint_timeout_ms);
    let local_target_authority =
        update.local_speaker_tracking_enabled && update.local_target_speech_end_ms.is_some();
    let cloud_target_authority = update.speaker_info_present && update.speaker_id.is_some();
    let recent_local_speech_is_non_target = update
        .local_speech_end_ms
        .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms));
    let provider_other_speaker_advanced = update
        .target_speech_end_ms
        .zip(update.stable_attributed_speech_end_ms)
        .is_some_and(|(target_ms, attributed_ms)| attributed_ms > target_ms);
    let uncertain_owner_budget_exhausted = update
        .local_target_speech_end_ms
        .zip(update.local_speech_end_ms)
        .is_some_and(|(target_ms, speech_ms)| {
            speech_ms > target_ms.saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
        });
    // Installed session 19df34c4: body text kept growing only in the provisional
    // channel while stable_attributed stayed on the wake phrase. A local target
    // clock already existed, so the old "pending only blocks without local
    // authority" rule let auto-end fire mid-sentence. Pending unattributed body
    // speech blocks unless repeated strong local evidence and a newer
    // non-target provider attribution both identify another person; room
    // speech must not hold auto-end, while startup calibration stays protected.
    let pending_blocks_endpoint = update.pending_unattributed_speech
        && !(recent_local_speech_is_non_target && provider_other_speaker_advanced);
    let stable_attributed_speech_end_ms = (!recent_local_speech_is_non_target
        && !uncertain_owner_budget_exhausted)
        .then_some(update.stable_attributed_speech_end_ms)
        .flatten();
    let target_speech_end_ms = update
        .target_speech_end_ms
        .into_iter()
        // Provider diarization can briefly split one continuous owner utterance
        // into a new speaker id. Stable attributed speech must still hold the
        // endpoint clock even though target-only text filtering remains strict.
        .chain(stable_attributed_speech_end_ms)
        .chain(update.local_target_speech_end_ms)
        .max();
    // Once the provider has reported any covered audio boundary, measure the
    // endpoint only inside that authoritative coverage. Local capture normally
    // runs ahead; using its newer clock with an older attributed target end can
    // stop a quiet sentence tail milliseconds before the next provider update.
    //
    // Provider-stall fallback may switch the coverage clock to local audio, but
    // must NOT bypass unresolved local speech: mid-sentence cloud freezes with
    // ongoing owner energy were ending on `inactive_1000ms` (2026-08-06 logs).
    let provider_stall_fallback =
        provider_stall_local_endpoint_due(update, provider_stall_confirmed, endpoint_timeout_ms);
    let endpoint_audio_duration_ms = if provider_stall_fallback {
        update.audio_duration_ms
    } else {
        update
            .provider_audio_duration_ms
            .or(update.audio_duration_ms)
    };
    (cloud_target_authority || local_target_authority)
        && !pending_blocks_endpoint
        && !unresolved_recent_local_speech
        && endpoint_audio_duration_ms
            .zip(target_speech_end_ms)
            .is_some_and(|(audio_ms, target_ms)| {
                audio_ms.saturating_sub(target_ms) >= endpoint_timeout_ms
            })
}

fn provider_stall_local_endpoint_due(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    provider_stall_confirmed: bool,
    endpoint_timeout_ms: u64,
) -> bool {
    let Some(provider_audio_ms) = update.provider_audio_duration_ms else {
        return false;
    };
    let Some(local_audio_ms) = update.audio_duration_ms else {
        return false;
    };
    let Some(cloud_target_end_ms) = update.target_speech_end_ms else {
        return false;
    };
    if !provider_stall_confirmed
        || !update.local_speaker_tracking_enabled
        || update.pending_unattributed_speech
        || local_audio_ms.saturating_sub(provider_audio_ms)
            < EMBEDDED_PROVIDER_STALL_FALLBACK_LAG_MS
    {
        return false;
    }

    // Ongoing unclassified local energy means the owner may still be speaking
    // while ASR/provider clocks froze. Confirmed non-target (other people) or
    // energy that has itself been quiet for the full endpoint interval may
    // still use stall fallback so room noise does not hold the session open.
    if has_unresolved_recent_local_speech(update, endpoint_timeout_ms) {
        return false;
    }

    // An enrolled wake can establish the cloud owner while every later local
    // window is too weak to score Target. Requiring a local Target boundary in
    // that state disabled the provider-stall fallback entirely (installed
    // session 1026). Once the newest local speech is explicitly NonTarget, it
    // is safe to retain the cloud owner boundary; the other speaker must not
    // keep the owner's recording open. Unclassified ongoing energy still
    // requires a local Target boundary and therefore remains protected from
    // mid-sentence cuts.
    let local_latest_is_non_target = update
        .local_speech_end_ms
        .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms));
    if update.local_target_speech_end_ms.is_none() && !local_latest_is_non_target {
        return false;
    }

    // A newer locally confirmed target tail is authoritative only after that
    // newer boundary has itself been inactive for the full endpoint interval.
    // This preserves quiet tails without waiting forever for a stalled provider
    // to repeat coverage it has already stopped reporting.
    let newest_target_end_ms = update
        .local_target_speech_end_ms
        .map_or(cloud_target_end_ms, |local_target_end_ms| {
            cloud_target_end_ms.max(local_target_end_ms)
        });
    local_audio_ms.saturating_sub(newest_target_end_ms) >= endpoint_timeout_ms
}

fn provider_progress_stalled(
    inner: &Arc<Inner>,
    session_id: SessionId,
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    now: Instant,
) -> bool {
    let Some(provider_audio_ms) = update.provider_audio_duration_ms else {
        return false;
    };
    let mut slot = inner.embedded_audio_provider_progress_guard.lock();
    let Some(guard) = slot.as_mut().filter(|guard| guard.session_id == session_id) else {
        *slot = Some(ProviderProgressGuard {
            session_id,
            provider_audio_ms,
            last_advanced_at: now,
        });
        return false;
    };
    if provider_audio_ms > guard.provider_audio_ms {
        guard.provider_audio_ms = provider_audio_ms;
        guard.last_advanced_at = now;
        return false;
    }
    provider_audio_ms == guard.provider_audio_ms
        && now.saturating_duration_since(guard.last_advanced_at)
            >= Duration::from_millis(EMBEDDED_PROVIDER_STALL_CONFIRM_MS)
}

fn stage_terminal_wake_continuation(
    inner: &Arc<Inner>,
    wake_pcm: Vec<u8>,
    wake_end_seconds: f32,
    wake_phrase: String,
) -> bool {
    stage_terminal_wake_continuation_at(
        inner,
        wake_pcm,
        wake_end_seconds,
        wake_phrase,
        Instant::now(),
    )
}

fn stage_terminal_wake_continuation_at(
    inner: &Arc<Inner>,
    wake_pcm: Vec<u8>,
    wake_end_seconds: f32,
    wake_phrase: String,
    now: Instant,
) -> bool {
    let mut slot = inner.embedded_audio_terminal_wake_continuation.lock();
    if slot
        .as_ref()
        .is_some_and(|continuation| continuation.expires_at > now)
    {
        return false;
    }
    *slot = Some(TerminalWakeContinuation {
        session_id: None,
        wake_pcm,
        wake_end_seconds,
        wake_phrase,
        expires_at: now + EMBEDDED_TERMINAL_WAKE_CONTINUATION_TTL,
    });
    true
}

pub(super) fn bind_terminal_wake_continuation_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> bool {
    bind_terminal_wake_continuation_session_at(inner, session_id, Instant::now())
}

fn bind_terminal_wake_continuation_session_at(
    inner: &Arc<Inner>,
    session_id: SessionId,
    now: Instant,
) -> bool {
    let phrase = {
        let mut slot = inner.embedded_audio_terminal_wake_continuation.lock();
        let Some(continuation) = slot.as_mut() else {
            return false;
        };
        if continuation.expires_at <= now || continuation.session_id.is_some() {
            *slot = None;
            return false;
        }
        continuation.session_id = Some(session_id);
        continuation.wake_phrase.clone()
    };
    // Bind the visible-capsule body guard before the frontend can acknowledge
    // the Recording event emitted by the host-start path.
    arm_automatic_wake_text_guard(inner, session_id, phrase, 0);
    true
}

fn take_terminal_wake_continuation(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> Option<TerminalWakeContinuation> {
    take_terminal_wake_continuation_at(inner, session_id, Instant::now())
}

fn take_terminal_wake_continuation_at(
    inner: &Arc<Inner>,
    session_id: SessionId,
    now: Instant,
) -> Option<TerminalWakeContinuation> {
    let continuation = inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .take()?;
    if continuation.expires_at > now && continuation.session_id == Some(session_id) {
        Some(continuation)
    } else {
        clear_automatic_wake_text_guard(inner);
        None
    }
}

pub(super) fn clear_terminal_wake_continuation(inner: &Arc<Inner>, session_id: SessionId) {
    let cleared = {
        let mut slot = inner.embedded_audio_terminal_wake_continuation.lock();
        if slot
            .as_ref()
            .is_some_and(|continuation| continuation.session_id == Some(session_id))
        {
            *slot = None;
            true
        } else {
            false
        }
    };
    if cleared {
        clear_automatic_wake_text_guard(inner);
    }
}

fn discard_terminal_wake_continuation(inner: &Arc<Inner>) {
    let had_continuation = inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .take()
        .is_some();
    if had_continuation {
        clear_automatic_wake_text_guard(inner);
    }
}

fn set_volcengine_preview_callbacks(
    asr: &Arc<VolcengineStreamingASR>,
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let stop_dispatched = Arc::new(AtomicBool::new(false));
    let endpoint_clock = Arc::new(Mutex::new(SettledTargetEndpointClock::default()));
    start_settled_target_endpoint_watchdog(inner, session_id, &stop_dispatched, &endpoint_clock);

    let inner_for_stream = Arc::clone(inner);
    let stop_for_stream = Arc::clone(&stop_dispatched);
    let clock_for_stream = Arc::clone(&endpoint_clock);
    asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
        update_embedded_audio_partial_preview(&inner_for_stream, session_id, text);
        arm_settled_target_endpoint_for_visible_body(
            &inner_for_stream,
            session_id,
            &stop_for_stream,
            &clock_for_stream,
        );
    })));

    let inner_for_partial = Arc::clone(inner);
    let stop_for_partial = Arc::clone(&stop_dispatched);
    let clock_for_partial = Arc::clone(&endpoint_clock);
    asr.set_final_intermediate_transcript_callback(Some(Arc::new(move |update| {
        update_embedded_audio_partial_preview_from_final_supplement(
            &inner_for_partial,
            session_id,
            update,
        );
        arm_settled_target_endpoint_for_visible_body(
            &inner_for_partial,
            session_id,
            &stop_for_partial,
            &clock_for_partial,
        );
    })));

    let inner_for_speaker = Arc::clone(inner);
    let clock_for_speaker = Arc::clone(&endpoint_clock);
    asr.set_target_speaker_update_callback(Some(Arc::new(move |update| {
        let body_started = automatic_wake_body_started(&inner_for_speaker, session_id)
            || current_embedded_audio_partial_preview(&inner_for_speaker)
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty());
        let now = Instant::now();
        let (settled_wall_clock_due, generation) = {
            let mut clock = clock_for_speaker.lock();
            let generation = clock.observe(&update, body_started, now);
            let due = clock.is_due(now, EMBEDDED_SETTLED_TARGET_WALL_CLOCK_MS);
            (due, generation)
        };
        handle_target_speaker_update(
            &inner_for_speaker,
            session_id,
            &stop_dispatched,
            update,
            settled_wall_clock_due,
        );
        if let Some(generation) = generation {
            schedule_settled_target_endpoint_timer(
                &inner_for_speaker,
                session_id,
                &stop_dispatched,
                &clock_for_speaker,
                generation,
            );
        }
    })));
}

fn build_volcengine_asr(inner: &Arc<Inner>, session_id: SessionId) -> Arc<VolcengineStreamingASR> {
    let asr = Arc::new(VolcengineStreamingASR::new(
        read_volc_credentials(),
        enabled_hotwords(inner),
    ));
    set_volcengine_preview_callbacks(&asr, inner, session_id);
    asr
}

async fn open_volcengine_asr(
    asr: &Arc<VolcengineStreamingASR>,
) -> Result<(), crate::asr::volcengine::VolcengineASRError> {
    let started = Instant::now();
    asr.open_session_for_deferred_audio().await?;
    log::info!(
        "[asr] authoritative optimized-bidirectional ASR ready; preview and final share one provider session elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(())
}

include!("dictation_device_ai.rs");

pub(super) fn current_embedded_audio_partial_preview(inner: &Arc<Inner>) -> Option<String> {
    inner.embedded_audio_partial_preview.lock().clone()
}

include!("dictation_preview.rs");

pub(super) async fn request_embedded_ble_recording_stop_from_host(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Result<bool, String> {
    if !embedded_ble_host_recording_control_context_active(inner) {
        return Ok(false);
    }
    let (session_id, phase) = {
        let state = inner.state.lock();
        (state.session_id, state.phase)
    };
    if !matches!(phase, SessionPhase::Starting | SessionPhase::Listening) {
        return Ok(false);
    }

    record_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::StopCommand,
        Some(session_id),
        format!("host stop requested reason={reason} phase={phase:?}"),
    );
    match phase {
        SessionPhase::Starting => request_stop_during_starting(inner, reason),
        SessionPhase::Listening => {
            let _ = request_embedded_audio_stop_feedback(inner, reason);
        }
        _ => {}
    }

    #[cfg(test)]
    {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            "firmware_stop_skipped_test",
            format!("session_id={session_id} phase={phase:?} reason={reason}"),
        );
        return Ok(true);
    }

    #[cfg(not(test))]
    {
        let result = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::send_recording_control_stop(
                EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
            )
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|value| value);

        match result {
            Ok(()) => {
                crate::timeline::mark(
                    "backend.embedded_ble_session_actor",
                    "firmware_stop_sent",
                    format!("session_id={session_id} phase={phase:?} reason={reason}"),
                );
                log::info!(
                    "[coord] embedded BLE firmware stop sent session_id={session_id} phase={phase:?} reason={reason}"
                );
                Ok(true)
            }
            Err(err) => {
                set_device_ai_processing_async(inner, false, "host_stop_failed");
                crate::timeline::mark(
                    "backend.embedded_ble_session_actor",
                    "firmware_stop_failed",
                    format!("session_id={session_id} phase={phase:?} reason={reason} error={err}"),
                );
                log::warn!(
                    "[coord] embedded BLE firmware stop failed session_id={session_id} phase={phase:?} reason={reason}: {err}"
                );
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some("Listener 录音停止失败".to_string()),
                    None,
                );
                schedule_capsule_idle(inner, 6000, Some(session_id));
                Err(err)
            }
        }
    }
}

fn store_embedded_audio_stats(inner: &Arc<Inner>, stats: crate::embedded_audio::SessionStats) {
    *inner.embedded_audio_stats.lock() = Some(stats);
}

fn take_embedded_audio_stats(inner: &Arc<Inner>) -> Option<crate::embedded_audio::SessionStats> {
    inner.embedded_audio_stats.lock().take()
}

fn clear_embedded_audio_final_result(inner: &Arc<Inner>) {
    *inner.embedded_audio_final_result.lock() = None;
}

fn store_embedded_audio_final_result(
    inner: &Arc<Inner>,
    result: crate::embedded_audio::EmbeddedAudioTranscriptResult,
) {
    *inner.embedded_audio_final_result.lock() = Some(result);
}

fn take_embedded_audio_final_result(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> Option<crate::embedded_audio::EmbeddedAudioTranscriptResult> {
    let mut slot = inner.embedded_audio_final_result.lock();
    if slot
        .as_ref()
        .is_some_and(|result| result.session_id == session_id.to_string())
    {
        slot.take()
    } else {
        None
    }
}

fn take_latest_embedded_audio_final_result(
    inner: &Arc<Inner>,
) -> Option<crate::embedded_audio::EmbeddedAudioTranscriptResult> {
    inner.embedded_audio_final_result.lock().take()
}

struct EmbeddedAudioDictationSession {
    session_id: SessionId,
    active_asr: String,
    consumer: Arc<dyn crate::recorder::AudioConsumer>,
    volcengine_asr: Option<Arc<VolcengineStreamingASR>>,
    archive_pcm: Option<Vec<u8>>,
    streamed_pcm_bytes: usize,
    normalized_pcm_bytes: usize,
    streaming_pcm_buffer: Vec<u8>,
    streaming_agc: EmbeddedStreamingAgcState,
    local_speaker_tracker: Option<LocalSessionSpeakerTracker>,
    device_ai_processing_started: bool,
    // Proactive trailing-silence stop (改A) state. See
    // EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS and consume_prepared_streaming_pcm.
    proactive_stop_body_started: bool,
    proactive_stop_silence_ms: u64,
    proactive_stop_dispatched: bool,
}

include!("dictation_wake_diagnostics.rs");
include!("dictation_wake_polish.rs");
include!("dictation_wake_fusion.rs");
include!("dictation_wake_prefix_retry.rs");
include!("dictation_wake_owner_gate.rs");

include!("dictation_session.rs");

include!("dictation_embedded_submit.rs");

fn embedded_audio_file_session_id() -> u32 {
    (chrono::Utc::now().timestamp_millis() as u64 & u32::MAX as u64) as u32
}

include!("dictation_embedded_stream_session.rs");
include!("dictation_embedded_stream.rs");
include!("dictation_embedded_stream_completion.rs");

fn submission_result_from_stats(
    terminal_received: bool,
    stats: crate::embedded_audio::SessionStats,
    transcript: Option<crate::embedded_audio::EmbeddedAudioTranscriptResult>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    if !terminal_received || !stats.terminal_received {
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
        transcript,
    })
}

async fn begin_embedded_audio_dictation_session(
    inner: &Arc<Inner>,
) -> Result<EmbeddedAudioDictationSession, String> {
    let current_session_id = begin_embedded_audio_dictation_session_id(inner)?;
    clear_embedded_audio_stats(inner);
    clear_embedded_audio_partial_preview(inner);
    // Host-start paths (terminal_wake_body_continuation, KEY start) arm the
    // automatic wake guard before BLE PCM attaches so empty-body abandon stays
    // at 3.0s. Unconditionally clearing here dropped that latch and made
    // body_started=false sessions fall back to snappy 1.0s ("没有识别到语音" /
    // empty continuation).
    if !automatic_wake_session_active(inner, current_session_id) {
        clear_automatic_wake_text_guard(inner);
    }
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
    let active_asr = active_asr_provider_from_preferences(inner);
    sync_active_asr_provider_to_credentials_for_runtime(&active_asr);
    log_dictation_asr_engine_selection(current_session_id, &active_asr);
    publish_dictation_capsule(
        inner,
        current_session_id,
        DictationUiState::Recording,
        0.0,
        dictation_asr_quality_warning(&active_asr),
        None,
    );

    if let Err(message) = ensure_asr_credentials(&active_asr) {
        log::warn!("[coord] embedded audio ASR credential gate failed: {message}");
        publish_dictation_pipeline_error(inner, current_session_id, message.clone());
        restore_prepared_windows_ime_session(inner, current_session_id);
        schedule_actionable_error_capsule_idle(inner, current_session_id);
        return Err(message);
    }
    let (consumer, volcengine_asr) =
        match build_embedded_audio_asr_consumer(inner, current_session_id, &active_asr).await {
            Ok(consumer) => consumer,
            Err(message) => {
                log::warn!("[coord] embedded audio ASR setup failed: {message}");
                publish_dictation_pipeline_error(inner, current_session_id, message.clone());
                restore_prepared_windows_ime_session(inner, current_session_id);
                cancel_asr_for_session(inner, current_session_id);
                schedule_actionable_error_capsule_idle(inner, current_session_id);
                return Err(message);
            }
        };

    let archive_pcm = record_embedded_audio_for_debug_enabled(inner).then(Vec::new);
    Ok(EmbeddedAudioDictationSession {
        session_id: current_session_id,
        active_asr,
        consumer,
        volcengine_asr,
        archive_pcm,
        streamed_pcm_bytes: 0,
        normalized_pcm_bytes: 0,
        streaming_pcm_buffer: Vec::new(),
        streaming_agc: EmbeddedStreamingAgcState::default(),
        local_speaker_tracker: None,
        device_ai_processing_started: false,
        proactive_stop_body_started: false,
        proactive_stop_silence_ms: 0,
        proactive_stop_dispatched: false,
    })
}

fn begin_embedded_audio_dictation_session_id(inner: &Arc<Inner>) -> Result<SessionId, String> {
    let attach_host_start = inner.prefs.get().dictation_input_source
        == DictationInputSource::EmbeddedBle
        && embedded_ble_actor_context_active(inner);
    let mut state = inner.state.lock();
    if attach_host_start && state.phase == SessionPhase::Starting {
        return Ok(state.session_id);
    }
    // Recover stuck Processing (post-cancel/empty-ASR race, common after OTA churn)
    // so the next EC11 / BLE start is not permanently blocked.
    if state.phase == SessionPhase::Processing {
        log::warn!(
            "[coord] clearing stuck Processing phase before embedded dictation start session_id={} cancelled={}",
            state.session_id,
            state.cancelled
        );
        state.phase = SessionPhase::Idle;
        state.focus_target = None;
        state.cancelled = false;
    }
    begin_session_state(&mut state, capture_focus_target(), capture_frontmost_app())
        .ok_or_else(|| "当前已有听写会话在运行，暂不能提交嵌入式音频".to_string())
}

fn activate_embedded_audio_dictation_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
    initial_level: f32,
) -> bool {
    if embedded_ble_actor_context_active(inner) {
        let applied = apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::BlePacket,
            session_id,
            "ble_start",
            DictationEvent::BleStart { session_id },
            initial_level,
            None,
            None,
        );
        if !applied {
            cancel_asr_for_session(inner, session_id);
            restore_prepared_windows_ime_session(inner, session_id);
        }
        return applied;
    }

    let transition = {
        let mut state = inner.state.lock();
        apply_dictation_event(&mut state, DictationEvent::BleStart { session_id })
    };
    if matches!(transition, DictationTransition::Ignored { .. }) {
        cancel_asr_for_session(inner, session_id);
        restore_prepared_windows_ime_session(inner, session_id);
        return false;
    }

    publish_dictation_transition(inner, transition, initial_level, None, None);
    true
}

async fn submit_embedded_pcm_for_dictation_with_stats(
    inner: &Arc<Inner>,
    pcm: &[u8],
    stats: Option<crate::embedded_audio::SessionStats>,
) -> Result<(), String> {
    let session = begin_embedded_audio_dictation_session(inner).await?;
    let current_session_id = session.session_id;
    let active_asr = session.active_asr.clone();
    let consumer = Arc::clone(&session.consumer);

    let archive_active = archive_embedded_audio_if_enabled(inner, current_session_id, pcm);
    inner
        .audio_archive_active
        .store(archive_active, std::sync::atomic::Ordering::Relaxed);
    let (asr_pcm, gain_stats) = normalize_embedded_pcm_for_asr(pcm);
    if gain_stats.gain < 1.0 || gain_stats.upstream_clipped_samples > 0 {
        log::info!(
            "[coord] embedded audio host limiter (rms_before={:.1}, peak_before={}, rms_after={:.1}, peak_after={}, gain={:.4}, limiter_reduction_db={:.2}, upstream_clipped_samples={}, newly_clipped_samples={})",
            gain_stats.rms_before,
            gain_stats.peak_before,
            gain_stats.rms_after,
            gain_stats.peak_after,
            gain_stats.gain,
            gain_stats.limiter_reduction_db,
            gain_stats.upstream_clipped_samples,
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
    for chunk in asr_pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
        consumer.consume_pcm_chunk(chunk);
    }
    log::info!(
        "[coord] embedded audio submitted to dictation pipeline (asr={active_asr}, pcm_bytes={}, asr_pcm_bytes={}, host_limiter_gain={:.4})",
        pcm.len(),
        asr_pcm.len(),
        gain_stats.gain
    );
    if let Some(stats) = stats {
        store_embedded_audio_stats(inner, stats);
    }

    end_session_with_stop_origin(inner, false).await
}

fn embedded_streaming_chunk_is_asr_input(
    _chunk: &crate::embedded_audio::StreamingPcmChunk,
) -> bool {
    // The collector has already validated session ownership and sequence. STOP ends capture,
    // but firmware can still drain valid PCM from that same capture after its control packet.
    true
}

async fn persist_verified_wake_phrase_calibration(phrase: String) {
    let phrase_for_task = phrase.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        crate::wake_phrase::persist_bootstrap_calibration_if_missing(&phrase_for_task)
    })
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            log::warn!(
                "[wake-phrase] verified runtime calibration was not persisted phrase={phrase}: {err}"
            );
        }
        Err(err) => {
            log::warn!(
                "[wake-phrase] verified runtime calibration task failed phrase={phrase}: {err}"
            );
        }
    }
}

/// Newest hidden VA session Type is currently handling. Used so a late
/// VREC:STOP for reject N does not kill already-started candidate N+1
/// (owner: called twice, second window cut at ~0.9s by previous reject STOP).
static LAST_HIDDEN_VA_SESSION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn note_hidden_va_session(embedded_session_id: u32) {
    LAST_HIDDEN_VA_SESSION.store(embedded_session_id, Ordering::SeqCst);
}

fn reject_hidden_automatic_candidate(reason: &'static str, embedded_session_id: u32) {
    log::info!(
        "[speaker-verification] hidden automatic candidate rejected silently reason={reason} embedded_session_id={embedded_session_id}"
    );
    // Hidden VA candidates never open a Type dictation session (phase stays Idle),
    // so request_embedded_ble_recording_stop_from_host is a no-op. Without an
    // explicit VREC:STOP the device keeps streaming ambient speech until silence
    // or max_session — owner could not re-arm 「开始录音」. Cut the device only
    // when this reject is still the latest candidate (avoid killing N+1).
    #[cfg(not(test))]
    {
        tauri::async_runtime::spawn_blocking(move || {
            // Brief yield: SessionStart for the next candidate often races the
            // terminal reject of the previous one.
            std::thread::sleep(Duration::from_millis(80));
            let current = LAST_HIDDEN_VA_SESSION.load(Ordering::SeqCst);
            if embedded_session_id != 0 && current != embedded_session_id {
                log::info!(
                    "[coord] skip VREC:STOP after reject reason={reason} rejected_session={embedded_session_id} active_session={current}"
                );
                return;
            }
            match crate::embedded_ble::send_recording_control_stop(
                EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
            ) {
                Ok(()) => log::info!(
                    "[coord] VREC:STOP sent after hidden automatic reject reason={reason} embedded_session_id={embedded_session_id}"
                ),
                Err(err) => log::warn!(
                    "[coord] VREC:STOP after hidden reject failed reason={reason} embedded_session_id={embedded_session_id}: {err}"
                ),
            }
        });
    }
}

/// Show Recording capsule early so the user is not waiting on stage-2 alone.
/// Prefer local ExactStart; also allow first KWS hit when no voiceprint is
/// enrolled (open-gate contract — phrase alone accepts). With voiceprint,
/// keep KWS as recall-only until local/owner gates pass (false-start risk).
fn show_early_wake_recording_capsule(inner: &Arc<Inner>, candidate: &mut BufferedSpeakerCandidate) {
    if candidate.early_capsule_session_id.is_some() {
        return;
    }
    let session_id = {
        let mut state = inner.state.lock();
        if matches!(
            state.phase,
            SessionPhase::Starting | SessionPhase::Listening
        ) {
            candidate.early_capsule_session_id = Some(state.session_id);
            return;
        }
        if state.phase != SessionPhase::Idle {
            return;
        }
        match crate::coordinator_state::begin_session_state(
            &mut state,
            capture_focus_target(),
            capture_frontmost_app(),
        ) {
            Some(id) => id,
            None => return,
        }
    };
    candidate.early_capsule_request_ms = Some(candidate.started_at.elapsed().as_millis() as u64);
    publish_dictation_capsule(
        inner,
        session_id,
        DictationUiState::Recording,
        0.0,
        None,
        None,
    );
    candidate.early_capsule_session_id = Some(session_id);
    log::info!(
        "[wake-phrase] early recording capsule shown session_id={session_id} (local full-phrase confirmed)"
    );
}

fn dismiss_early_wake_recording_capsule(inner: &Arc<Inner>, session_id: SessionId) {
    {
        let mut state = inner.state.lock();
        if state.session_id == session_id
            && matches!(
                state.phase,
                SessionPhase::Starting | SessionPhase::Listening
            )
        {
            state.phase = SessionPhase::Idle;
        }
    }
    schedule_capsule_idle(inner, 0, Some(session_id));
    log::info!(
        "[wake-phrase] early recording capsule dismissed session_id={session_id} (wake not confirmed)"
    );
}

fn take_early_capsule_session_id(candidate: &mut BufferedSpeakerCandidate) -> Option<SessionId> {
    candidate.early_capsule_session_id.take()
}

fn complete_voiceprint_enrollment_candidate(reason: &'static str) {
    tauri::async_runtime::spawn_blocking(move || {
        match crate::embedded_ble::send_recording_processing_done(Duration::from_secs(2)) {
            Ok(()) => {
                log::info!("[speaker-verification] device processing completed reason={reason}")
            }
            Err(err) => log::warn!(
                "[speaker-verification] device processing completion failed reason={reason}: {err}"
            ),
        }
    });
}

fn embedded_audio_stop_is_user_initiated(
    origin: Option<crate::embedded_audio::SessionStopOrigin>,
) -> bool {
    origin != Some(crate::embedded_audio::SessionStopOrigin::VoiceActivation)
}

fn speaker_candidate_may_reach_asr(automatic: bool, verified_match: Option<bool>) -> bool {
    !automatic || verified_match == Some(true)
}

fn embedded_ble_session_event_detail(
    event: &crate::embedded_audio::StreamingSessionEvent,
) -> String {
    match event {
        crate::embedded_audio::StreamingSessionEvent::Started { session_id, origin } => {
            format!("event=start embedded_session_id={session_id} origin={origin:?}")
        }
        crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => format!(
            "event=pcm embedded_session_id={} packet_sequence={} pcm_bytes={} raw_input_level_percent={:?} after_stop={}",
            chunk.session_id,
            chunk.packet_sequence,
            chunk.pcm.len(),
            chunk.raw_input_level_percent,
            chunk.after_stop_boundary
        ),
        crate::embedded_audio::StreamingSessionEvent::Stopped {
            session_id,
            expected_packet_count,
            origin,
        } => format!(
            "event=stop embedded_session_id={session_id} expected_packets={expected_packet_count} origin={origin:?}"
        ),
        crate::embedded_audio::StreamingSessionEvent::Cancelled {
            session_id,
            expected_packet_count,
        } => format!(
            "event=cancel embedded_session_id={session_id} expected_packets={expected_packet_count}"
        ),
        crate::embedded_audio::StreamingSessionEvent::Error {
            session_id,
            expected_packet_count,
            error_code,
        } => format!(
            "event=error embedded_session_id={session_id} expected_packets={expected_packet_count} error_code={error_code:?}"
        ),
        crate::embedded_audio::StreamingSessionEvent::Ignored(reason) => {
            format!("event=ignored reason={reason:?}")
        }
    }
}

fn embedded_ble_session_event_should_trace(
    event: &crate::embedded_audio::StreamingSessionEvent,
) -> bool {
    match event {
        crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => {
            chunk.after_stop_boundary
                || chunk.packet_sequence == 0
                || chunk.packet_sequence % EMBEDDED_BLE_PCM_EVENT_TRACE_PACKET_INTERVAL == 0
        }
        _ => true,
    }
}

async fn build_embedded_audio_asr_consumer(
    inner: &Arc<Inner>,
    session_id: SessionId,
    active_asr: &str,
) -> Result<
    (
        Arc<dyn crate::recorder::AudioConsumer>,
        Option<Arc<VolcengineStreamingASR>>,
    ),
    String,
> {
    #[cfg(target_os = "windows")]
    if foundry::is_foundry_local_whisper(active_asr) {
        let prefs = inner.prefs.get();
        let model_alias = if foundry::model_alias_is_known(&prefs.foundry_local_asr_model) {
            prefs.foundry_local_asr_model.clone()
        } else {
            foundry::DEFAULT_MODEL_ALIAS.to_string()
        };
        let language_hint = foundry_language_hint_from_preferences(&prefs);
        log::info!(
            "[foundry-asr] language hint selected source={} hint={}",
            language_hint.source,
            language_hint.hint.as_deref().unwrap_or("auto")
        );
        let local = Arc::new(FoundryLocalWhisperAsr::new(
            Arc::clone(&inner.foundry_local_runtime),
            model_alias,
            prefs.foundry_local_runtime_source.clone(),
            language_hint.hint,
        ));
        store_asr_for_session(
            inner,
            session_id,
            ActiveAsr::FoundryLocalWhisper(Arc::clone(&local)),
        );
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = local;
        return Ok((consumer, None));
    }

    if is_whisper_compatible_provider(active_asr) {
        let (api_key, base_url, model, proxy_config) =
            read_whisper_credentials(active_asr).map_err(|err| err.to_string())?;
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
        return Ok((consumer, None));
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
        return Ok((consumer, None));
    }

    let final_asr = build_volcengine_asr(inner, session_id);
    let bridge = Arc::new(DeferredAsrBridge::new());
    let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge.clone();
    store_asr_for_session(
        inner,
        session_id,
        ActiveAsr::Volcengine(Arc::clone(&final_asr)),
    );
    let inner_for_open = Arc::clone(inner);
    let final_asr_for_open = Arc::clone(&final_asr);
    tauri::async_runtime::spawn(async move {
        match open_volcengine_asr(&final_asr_for_open).await {
            Ok(()) => {
                let still_current = {
                    let state = inner_for_open.state.lock();
                    state.session_id == session_id
                        && !state.cancelled
                        && state.phase != SessionPhase::Idle
                };
                if !still_current {
                    final_asr_for_open.cancel();
                    log::info!(
                        "[coord] embedded Volcengine ASR opened after stale session {session_id} - discarded"
                    );
                    return;
                }
                let target: Arc<dyn crate::asr::AudioConsumer> = final_asr_for_open.clone();
                let flushed_bytes = bridge.attach(target);
                final_asr_for_open.mark_audio_delivery_ready();
                log::info!(
                    "[coord] embedded Volcengine ASR connected; flushed {flushed_bytes} deferred audio bytes"
                );
            }
            Err(err) => {
                let still_current = {
                    let state = inner_for_open.state.lock();
                    state.session_id == session_id
                        && !state.cancelled
                        && state.phase != SessionPhase::Idle
                };
                if still_current {
                    let target: Arc<dyn crate::asr::AudioConsumer> = final_asr_for_open.clone();
                    let retained_bytes = bridge.attach(target);
                    final_asr_for_open.mark_audio_delivery_failed(err.clone());
                    log::warn!(
                        "[coord] embedded Volcengine ASR open failed; retained {retained_bytes} deferred audio bytes for one finalization replay: {err}"
                    );
                } else {
                    final_asr_for_open.cancel();
                }
            }
        }
    });
    Ok((consumer, Some(final_asr)))
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

fn embedded_pcm_visual_level(pcm: &[u8]) -> f32 {
    let sample_count = pcm.len() / 2;
    if sample_count == 0 {
        return 0.0;
    }

    let sum = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f64)
        .sum::<f64>();
    let mean = sum / sample_count as f64;
    let sum_deviation_squares = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f64 - mean)
        .map(|sample| sample * sample)
        .sum::<f64>();
    let rms = (sum_deviation_squares / sample_count as f64).sqrt();

    // This is a display-only raw-audio meter. It deliberately does not use the
    // ASR gain, whose session calibration would make a quiet and a loud voice
    // appear similarly bright after firmware capture headroom was restored.
    (rms / EMBEDDED_AUDIO_VISUAL_RMS_REFERENCE).clamp(0.0, 1.0) as f32
}

fn embedded_pcm_capsule_level(pcm: &[u8], raw_input_level_percent: Option<u8>) -> f32 {
    raw_input_level_percent
        .map(embedded_raw_input_level_to_capsule_level)
        .unwrap_or_else(|| embedded_pcm_visual_level(pcm))
}

fn embedded_raw_input_level_to_capsule_level(level_percent: u8) -> f32 {
    const CAPSULE_SILENCE_GATE: f32 = 0.012;
    const CAPSULE_RESPONSE_CEILING: f32 = 0.34;

    let level_percent = level_percent.min(100);
    if level_percent == 0 {
        return 0.0;
    }
    CAPSULE_SILENCE_GATE
        + (f32::from(level_percent) / 100.0) * (CAPSULE_RESPONSE_CEILING - CAPSULE_SILENCE_GATE)
}

#[derive(Debug, Clone, Copy)]
struct EmbeddedPcmGainStats {
    rms_before: f64,
    peak_before: u16,
    rms_after: f64,
    peak_after: u16,
    gain: f64,
    limiter_reduction_db: f64,
    upstream_clipped_samples: usize,
    clipped_samples: usize,
}

#[derive(Debug)]
struct EmbeddedStreamingAgcState {
    gain: f64,
    gain_calibrated: bool,
    first_voiced_pcm_ms: Option<u64>,
    voiced_chunks: usize,
    quiet_chunks: usize,
    observed_signal_rms_min: Option<f64>,
    observed_signal_rms_max: f64,
    observed_signal_peak_max: u16,
    pre_calibration_quiet_chunks: usize,
    pre_calibration_signal_rms_max: f64,
    pre_calibration_signal_peak_max: u16,
    first_eligible_signal_rms: Option<f64>,
    first_eligible_signal_peak: Option<u16>,
    first_gain: Option<f64>,
    max_gain: f64,
    gain_update_count: usize,
    limiter_reduction_db_max: f64,
    upstream_clipped_samples: usize,
    clipped_samples: usize,
}

impl Default for EmbeddedStreamingAgcState {
    fn default() -> Self {
        Self {
            gain: 1.0,
            gain_calibrated: false,
            first_voiced_pcm_ms: None,
            voiced_chunks: 0,
            quiet_chunks: 0,
            observed_signal_rms_min: None,
            observed_signal_rms_max: 0.0,
            observed_signal_peak_max: 0,
            pre_calibration_quiet_chunks: 0,
            pre_calibration_signal_rms_max: 0.0,
            pre_calibration_signal_peak_max: 0,
            first_eligible_signal_rms: None,
            first_eligible_signal_peak: None,
            first_gain: None,
            max_gain: 1.0,
            gain_update_count: 0,
            limiter_reduction_db_max: 0.0,
            upstream_clipped_samples: 0,
            clipped_samples: 0,
        }
    }
}

fn normalize_embedded_pcm_for_asr(pcm: &[u8]) -> (Vec<u8>, EmbeddedPcmGainStats) {
    let (rms_before, peak_before) = embedded_pcm_rms_and_peak(pcm);
    let upstream_clipped_samples = pcm
        .chunks_exact(2)
        .filter(|chunk| {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            sample == i16::MIN || sample == i16::MAX
        })
        .count();
    let gain = if peak_before as f64 > EMBEDDED_AUDIO_HOST_LIMITER_PEAK {
        EMBEDDED_AUDIO_HOST_LIMITER_PEAK / peak_before as f64
    } else {
        1.0
    };
    let (normalized, clipped_samples) = apply_embedded_pcm_gain(pcm, gain);
    let (rms_after, peak_after) = embedded_pcm_rms_and_peak(&normalized);
    let stats = EmbeddedPcmGainStats {
        rms_before,
        peak_before,
        rms_after,
        peak_after,
        gain,
        limiter_reduction_db: if gain < 1.0 {
            -20.0 * gain.log10()
        } else {
            0.0
        },
        upstream_clipped_samples,
        clipped_samples,
    };
    (normalized, stats)
}

fn apply_embedded_pcm_gain(pcm: &[u8], gain: f64) -> (Vec<u8>, usize) {
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
    (normalized, clipped_samples)
}

fn normalize_embedded_streaming_pcm_for_asr(
    pcm: &[u8],
    agc: &mut EmbeddedStreamingAgcState,
) -> (Vec<u8>, EmbeddedPcmGainStats) {
    let (signal_rms, signal_peak) = embedded_pcm_streaming_agc_signal_level(pcm);
    let (normalized, stats) = normalize_embedded_pcm_for_asr(pcm);

    let has_speech_energy = embedded_streaming_chunk_has_speech_energy(signal_rms, signal_peak);
    agc.observed_signal_rms_min = Some(
        agc.observed_signal_rms_min
            .map_or(signal_rms, |minimum| minimum.min(signal_rms)),
    );
    agc.observed_signal_rms_max = agc.observed_signal_rms_max.max(signal_rms);
    agc.observed_signal_peak_max = agc.observed_signal_peak_max.max(signal_peak);
    if !has_speech_energy {
        agc.quiet_chunks += 1;
        agc.pre_calibration_quiet_chunks += 1;
        agc.pre_calibration_signal_rms_max = agc.pre_calibration_signal_rms_max.max(signal_rms);
        agc.pre_calibration_signal_peak_max = agc.pre_calibration_signal_peak_max.max(signal_peak);
    } else {
        agc.voiced_chunks += 1;
        agc.first_eligible_signal_rms.get_or_insert(signal_rms);
        agc.first_eligible_signal_peak.get_or_insert(signal_peak);
    }
    if stats.gain < 1.0 {
        agc.gain_update_count += 1;
    }
    agc.gain_calibrated = true;
    agc.first_gain.get_or_insert(stats.gain);
    agc.gain = stats.gain;
    agc.limiter_reduction_db_max = agc.limiter_reduction_db_max.max(stats.limiter_reduction_db);
    agc.upstream_clipped_samples += stats.upstream_clipped_samples;
    agc.clipped_samples += stats.clipped_samples;
    (normalized, stats)
}

fn embedded_streaming_chunk_has_speech_energy(rms: f64, peak: u16) -> bool {
    rms >= EMBEDDED_AUDIO_STREAMING_SPEECH_RMS
        || (rms >= EMBEDDED_AUDIO_STREAMING_QUIET_SPEECH_RMS
            && peak >= EMBEDDED_AUDIO_STREAMING_QUIET_SPEECH_PEAK)
}

fn embedded_pcm_streaming_agc_signal_level(pcm: &[u8]) -> (f64, u16) {
    let mut magnitudes: Vec<u16> = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]).unsigned_abs())
        .collect();
    if magnitudes.is_empty() {
        return (0.0, 0);
    }

    magnitudes.sort_unstable();
    let retained_count = ((magnitudes.len()
        * EMBEDDED_AUDIO_STREAMING_AGC_SIGNAL_PERCENTILE_NUMERATOR)
        / EMBEDDED_AUDIO_STREAMING_AGC_SIGNAL_PERCENTILE_DENOMINATOR)
        .max(1);
    let retained = &magnitudes[..retained_count];
    let sum_squares: f64 = retained
        .iter()
        .map(|&magnitude| {
            let sample = magnitude as f64;
            sample * sample
        })
        .sum();
    (
        (sum_squares / retained_count as f64).sqrt(),
        retained[retained_count - 1],
    )
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
    end_session_with_stop_origin(inner, true).await
}

async fn end_session_with_stop_origin(
    inner: &Arc<Inner>,
    user_initiated_stop: bool,
) -> Result<(), String> {
    let transition = begin_stop_session_transition(inner, user_initiated_stop);
    finish_end_session_after_stop_transition(inner, transition).await
}

async fn end_embedded_ble_session(
    inner: &Arc<Inner>,
    user_initiated_stop: bool,
    detail: impl Into<String>,
) -> Result<(), String> {
    let session_id = inner.state.lock().session_id;
    let transition = dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::StopCommand,
        Some(session_id),
        detail,
        |_| begin_stop_session_transition(inner, user_initiated_stop),
    );
    finish_end_session_after_stop_transition(inner, transition).await
}

fn begin_stop_session_transition(inner: &Arc<Inner>, user_initiated: bool) -> DictationTransition {
    let mut state = inner.state.lock();
    let session_id = state.session_id;
    apply_dictation_event(
        &mut state,
        DictationEvent::Stop {
            session_id,
            user_initiated,
        },
    )
}

async fn finish_end_session_after_stop_transition(
    inner: &Arc<Inner>,
    transition: DictationTransition,
) -> Result<(), String> {
    let current_session_id = match transition {
        DictationTransition::Applied {
            session_id: Some(session_id),
            ..
        } => session_id,
        _ => {
            return Ok(());
        }
    };
    let user_initiated_stop = {
        let state = inner.state.lock();
        state.session_id == current_session_id && state.user_initiated_stop
    };
    publish_dictation_transition(
        inner,
        transition,
        0.0,
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
            set_device_ai_processing_async(inner, false, "dictation_processing_no_asr");
            set_phase_idle_if_session_matches(inner, current_session_id);
            return Ok(());
        }
    };
    let mut device_ai_processing = DeviceAiProcessingGuard::defer(inner);
    device_ai_processing.start_if_needed("dictation_transcribing_processing_start");

    let uses_global_timeout = asr_transcribe_uses_global_timeout(&asr);
    // F2（2026-08-09 12:47:04 云端空转）：终稿空但本地持续人声时要用留存音频
    // 向新 ASR 会话重试一次——match 会移走 asr，先留 Arc 句柄。
    let volcengine_for_empty_retry = match &asr {
        ActiveAsr::Volcengine(asr) => Some(Arc::clone(asr)),
        _ => None,
    };
    let raw = match asr {
        ActiveAsr::Volcengine(asr) => {
            debug_assert!(uses_global_timeout);
            let timeout_duration = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
            let primary = match asr.send_last_frame().await {
                Ok(()) => {
                    // 添加全局超时保护：防止 await_final_result() 永远挂起
                    match tokio::time::timeout(timeout_duration, asr.await_final_result()).await {
                        Ok(result) => result.map_err(|error| (error, false)),
                        Err(_) => Err((
                            crate::asr::volcengine::VolcengineASRError::FinalResultTimeout,
                            true,
                        )),
                    }
                }
                Err(error) => Err((error, false)),
            };
            match primary {
                Ok(result) => result,
                Err((primary_error, _)) if primary_error.permits_full_audio_replay() => {
                    log::warn!(
                        "[coord] Volcengine primary stream failed; attempting one retained-audio replay: {primary_error}"
                    );
                    asr.cancel();
                    match tokio::time::timeout(timeout_duration, asr.replay_retained_audio_once())
                        .await
                    {
                        Ok(Ok(result)) => result,
                        Ok(Err(recovery_error)) => {
                            log::error!(
                                "[coord] Volcengine retained-audio replay failed after primary error ({primary_error}): {recovery_error}"
                            );
                            finish_dictation_pipeline_error(
                                inner,
                                current_session_id,
                                format!("识别恢复失败: {recovery_error}"),
                            );
                            return Err(recovery_error.to_string());
                        }
                        Err(_) => {
                            log::error!(
                                "[coord] Volcengine retained-audio replay timed out after {} seconds (primary_error={primary_error})",
                                COORDINATOR_GLOBAL_TIMEOUT_SECS
                            );
                            finish_dictation_timeout(
                                inner,
                                current_session_id,
                                "识别恢复超时".to_string(),
                            );
                            return Err("recovery replay timeout".to_string());
                        }
                    }
                }
                Err((primary_error, primary_global_timeout)) => {
                    log::error!("[coord] Volcengine finalization failed: {primary_error}");
                    asr.cancel();
                    if primary_global_timeout {
                        finish_dictation_timeout(inner, current_session_id, "识别超时".to_string());
                    } else {
                        finish_dictation_pipeline_error(
                            inner,
                            current_session_id,
                            format!("识别失败: {primary_error}"),
                        );
                    }
                    return Err(primary_error.to_string());
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
                    finish_dictation_pipeline_error(
                        inner,
                        current_session_id,
                        format!("识别失败: {e}"),
                    );
                    return Err(e.to_string());
                }
                Err(_) => {
                    log::error!(
                        "[coord] whisper 全局超时 {} 秒",
                        COORDINATOR_GLOBAL_TIMEOUT_SECS
                    );
                    finish_dictation_timeout(inner, current_session_id, "识别超时".to_string());
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
                    finish_dictation_pipeline_error(
                        inner,
                        current_session_id,
                        format!("识别失败: {e}"),
                    );
                    return Err(e.to_string());
                }
                Err(_) => {
                    log::error!(
                        "[coord] Bailian 全局超时 {} 秒",
                        COORDINATOR_GLOBAL_TIMEOUT_SECS
                    );
                    asr.cancel();
                    finish_dictation_timeout(inner, current_session_id, "识别超时".to_string());
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
                    finish_dictation_pipeline_error(
                        inner,
                        current_session_id,
                        format!("本地识别失败: {e}"),
                    );
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
                    finish_dictation_pipeline_error(
                        inner,
                        current_session_id,
                        format!("本地识别失败: {e}"),
                    );
                    return Err(e.to_string());
                }
                Err(_) => {
                    log::error!(
                        "[coord] local Qwen3-ASR 动态超时 {}s（音频 {:.2}s）",
                        timeout_duration.as_secs(),
                        audio_secs
                    );
                    finish_dictation_timeout(inner, current_session_id, "识别超时".to_string());
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

    let unfiltered_text = raw.text.clone();
    raw.text = filter_automatic_wake_text(inner, current_session_id, &raw.text, false);
    if raw.text != unfiltered_text.trim() {
        log::info!(
            "[wake-phrase] removed automatic activation prefix from final transcript session_id={} before_chars={} after_chars={}",
            current_session_id,
            unfiltered_text.chars().count(),
            raw.text.chars().count()
        );
    }
    // F2 云端空转兜底重试（2026-08-09 12:47:04：包全收、音频足量、云端
    // audio_duration 正常增长，但终稿只有唤醒词残段）。终稿空且本地证据显示
    // 整段持续人声时，用 retained_pcm 向新 ASR 会话有界重试一次；仍空才走
    // 下面的 emptyTranscript 护栏。replay_retained_audio_once 自带一次性闸。
    if raw.text.trim().is_empty() {
        if let Some(asr) = volcengine_for_empty_retry.as_ref() {
            let automatic_wake = automatic_wake_session_active(inner, current_session_id);
            let retry_allowed = asr.has_sustained_local_speech_evidence()
                || (!automatic_wake && asr.has_local_speech_evidence());
            if retry_allowed {
                log::warn!(
                    "[coord] empty final with local speech evidence; retrying once with bounded retained audio session_id={current_session_id} automatic_wake={automatic_wake}"
                );
                asr.cancel();
                let retry_timeout = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
                match tokio::time::timeout(
                    retry_timeout,
                    asr.replay_retained_audio_once_for_empty_final(),
                )
                .await
                {
                    Ok(Ok(replayed)) if !replayed.text.trim().is_empty() => {
                        log::info!(
                            "[coord] empty-spin retained-audio retry recovered session_id={} chars={}",
                            current_session_id,
                            replayed.text.chars().count()
                        );
                        raw = replayed;
                        raw.text =
                            filter_automatic_wake_text(inner, current_session_id, &raw.text, false);
                    }
                    Ok(Ok(_)) => {
                        log::info!(
                            "[coord] empty-spin retained-audio retry still empty session_id={current_session_id}"
                        );
                    }
                    Ok(Err(error)) => {
                        log::warn!("[coord] empty-spin retained-audio retry failed: {error}");
                    }
                    Err(_) => {
                        log::warn!("[coord] empty-spin retained-audio retry timed out");
                    }
                }
            }
        }
    }
    // Live multi-speaker finals can collapse to empty after speaker filtering
    // even when the capsule already streamed a long owner preview. Prefer that
    // preview over the false "没有识别到语音" failure path.
    if raw.text.trim().is_empty() {
        if let Some(preview) = current_embedded_audio_partial_preview(inner) {
            let recovered = filter_automatic_wake_text(inner, current_session_id, &preview, false);
            if !recovered.trim().is_empty() {
                log::warn!(
                    "[coord] empty ASR final recovered from partial preview session_id={} preview_chars={} recovered_chars={}",
                    current_session_id,
                    preview.chars().count(),
                    recovered.chars().count()
                );
                raw.text = recovered;
            }
        }
    }
    if inner.prefs.get().remove_filler_words {
        let before = raw.text.clone();
        raw.text = remove_standalone_dictation_fillers(&raw.text);
        if raw.text != before {
            log::info!(
                "[coord] removed standalone filler words session_id={} before_chars={} after_chars={}",
                current_session_id,
                before.chars().count(),
                raw.text.chars().count()
            );
        }
    }

    if raw.text.trim().is_empty() {
        let wake_only_expired = automatic_wake_session_active(inner, current_session_id)
            && !automatic_wake_body_started(inner, current_session_id);
        if wake_only_expired {
            log::info!(
                "[coord] wake-only body window expired silently session_id={current_session_id}"
            );
            device_ai_processing
                .complete_success("wake_only_body_window_expired")
                .await;
            store_embedded_audio_final_result(
                inner,
                crate::embedded_audio::EmbeddedAudioTranscriptResult {
                    session_id: current_session_id.to_string(),
                    raw_transcript: String::new(),
                    final_text: String::new(),
                    error_code: None,
                },
            );
            let published = publish_embedded_ble_wake_only_expired(inner, current_session_id);
            if !published {
                let mut state = inner.state.lock();
                if state.session_id == current_session_id {
                    state.phase = SessionPhase::Idle;
                    state.focus_target = None;
                }
            }
            clear_automatic_wake_text_guard(inner);
            clear_embedded_audio_partial_preview(inner);
            clear_embedded_audio_stats(inner);
            restore_prepared_windows_ime_session(inner, current_session_id);
            return Ok(());
        }
        let session = DictationSession {
            // The WAV archive was written with `current_session_id` before
            // ASR finalization. Keep History on the same identity even when
            // the provider returns no text, otherwise it asks for a different
            // `<id>.wav` and the captured recording cannot be loaded.
            id: current_session_id.to_string(),
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
        store_embedded_audio_final_result(
            inner,
            crate::embedded_audio::EmbeddedAudioTranscriptResult {
                session_id: current_session_id.to_string(),
                raw_transcript: raw.text.clone(),
                final_text: String::new(),
                error_code: Some("emptyTranscript".to_string()),
            },
        );
        device_ai_processing
            .complete_warning("dictation_empty_transcript")
            .await;
        let published = publish_embedded_ble_asr_final(
            inner,
            current_session_id,
            true,
            Some("没有识别到语音".to_string()),
        );
        // Cancel-during-Processing used to leave phase=Processing; AsrFinal then
        // Ignored(CancelledSession) never cleared it. Force Idle either way.
        if !published {
            let _ = cleanup_cancelled_processing_session(inner, current_session_id);
        }
        {
            let mut state = inner.state.lock();
            if state.session_id == current_session_id && state.phase == SessionPhase::Processing {
                log::warn!(
                    "[coord] empty transcript force-idle stuck Processing session_id={current_session_id} cancelled={}",
                    state.cancelled
                );
                state.phase = SessionPhase::Idle;
                state.focus_target = None;
            }
        }
        restore_prepared_windows_ime_session(inner, current_session_id);
        schedule_empty_transcript_capsule_idle(inner, current_session_id);
        return Err("ASR returned empty transcript".to_string());
    }

    if let Some(preview) = current_embedded_audio_partial_preview(inner) {
        raw.text = reconcile_final_transcript_with_preview_hotwords(
            &raw.text,
            &preview,
            &enabled_hotwords(inner),
        );
    }
    publish_embedded_ble_asr_final(inner, current_session_id, false, Some(raw.text.clone()));

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
                    "[coord] active style pack unavailable, falling back to builtin raw: {error}"
                );
                crate::types::builtin_style_pack_for_mode(PolishMode::Raw)
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
    let raw_uses_llm =
        !force_raw_output && mode == PolishMode::Raw && super::raw_style_pack_uses_llm(&pack);
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
    // Installed sessions 7ce523e5 / 6a86d8e3: LLM key 401 opened the auth
    // circuit, but we still entered streaming_insert, switched ABC, logged
    // FAILED, typed 0 chars, then clipboard-pasted. Skip polish entirely when
    // the circuit is open so stop→Done stays snappy and the UX is quiet raw
    // insert (owner: "体验一般般").
    let llm_auth_blocked = current_llm_auth_fingerprint()
        .ok()
        .is_some_and(current_llm_auth_is_rejected);
    let llm_stall_blocked = llm_stall_circuit_open();
    let needs_llm_polish = mode != PolishMode::Raw || raw_uses_llm;
    let streaming_eligible = streaming_insert_eligible(
        prefs.streaming_insert,
        translation_active,
        mode,
        raw_uses_llm,
        wayland_session,
    ) && !llm_auth_blocked
        && !llm_stall_blocked;
    log::info!(
        "[coord] polish dispatch: translation={translation_active} mode={mode:?} wayland_session={wayland_session} streaming_eligible={streaming_eligible} llm_auth_blocked={llm_auth_blocked} llm_stall_blocked={llm_stall_blocked}"
    );

    // 取出 endpoint 时刻发起的预热润色（若本会话有）。只有流式分支会尝试采用；
    // 其他分支（翻译/熔断/一次性）一律取消丢弃。
    let mut polish_prefetch = take_polish_prefetch(inner, current_session_id);

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
    } else if (llm_auth_blocked || llm_stall_blocked) && needs_llm_polish {
        log::info!(
            "[coord] LLM circuit open (auth={llm_auth_blocked} stall={llm_stall_blocked}); inserting raw transcript without polish wait (raw_chars={})",
            raw.text.chars().count()
        );
        (raw.text.clone(), None, false)
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
            polish_prefetch.take(),
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
        // 一次性路径的成败也喂 stall 熔断：成功复位；非 auth 失败累计。
        // （auth 401/403 已由 polish_text 写入 auth 熔断，这里不重复计。）
        match &e {
            None => note_llm_polish_success(),
            Some(err) => {
                let auth_failure = err.contains("credentials were already rejected")
                    || err.contains("AuthenticationError")
                    || err.contains("status 401")
                    || err.contains("status 403")
                    || err.contains("Unauthorized");
                if !auth_failure {
                    note_llm_polish_stall_failure();
                }
            }
        }
        (p, e, false)
    };

    // 非流式分支（翻译/熔断/一次性）：预热用不上，取消丢弃。
    if let Some(prefetch) = polish_prefetch {
        prefetch.cancel.store(true, Ordering::SeqCst);
    }

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
    publish_dictation_capsule(
        inner,
        current_session_id,
        DictationUiState::Polishing,
        0.0,
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
    let insert_transition = {
        let mut state = inner.state.lock();
        apply_dictation_event(
            &mut state,
            DictationEvent::InsertionStarted {
                session_id: current_session_id,
                already_streamed,
            },
        )
    };
    if matches!(insert_transition, DictationTransition::Ignored { .. }) {
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
    let retain_plain_dictation =
        prefs.copy_dictation_to_clipboard && !translation_active && !polished.trim().is_empty();
    let restore_clipboard =
        should_restore_clipboard_after_dictation(&prefs, retain_plain_dictation);
    let allow_clipboard_fallback = translation_active || retain_plain_dictation;
    let allow_non_tsf_insertion_fallback = prefs.allow_non_tsf_insertion_fallback;
    let allow_foreground_insert_fallback =
        std::env::var("LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK")
            .map(|value| value == "1")
            .unwrap_or(false);
    let paste_shortcut = prefs.paste_shortcut;
    // 流式键入和非 TSF 输入只能证明事件已发出。自动提交要求 TSF 确认原目标接受文本。
    let (status, original_target_confirmed) = if already_streamed {
        log::info!(
            "[coord] insertion skipped: {} chars already streamed via unicode_keystroke (polish_error={:?})",
            polished.chars().count(),
            polish_error
        );
        (InsertStatus::Inserted, false)
    } else if wayland_session {
        if allow_clipboard_fallback {
            log::info!(
                "[coord] Wayland session detected; retaining final text without synthetic paste ({} chars)",
                polished.chars().count()
            );
            (inner.inserter.copy_fallback(&polished), false)
        } else {
            log::warn!(
                "[coord] Wayland insertion skipped because final clipboard retention is disabled"
            );
            (InsertStatus::Failed, false)
        }
    } else if focus_ready_for_paste {
        #[cfg(target_os = "windows")]
        {
            let ime_target = capture_ime_submit_target();
            let result = insert_with_windows_ime_first(
                inner,
                current_session_id,
                &polished,
                restore_clipboard,
                allow_non_tsf_insertion_fallback,
                allow_clipboard_fallback,
                paste_shortcut,
                ime_target,
            )
            .await;
            (result.status, result.target_confirmed)
        }
        #[cfg(not(target_os = "windows"))]
        {
            if allow_clipboard_fallback {
                (
                    inner
                        .inserter
                        .insert(&polished, restore_clipboard, paste_shortcut),
                    false,
                )
            } else {
                (InsertStatus::Failed, false)
            }
        }
    } else if allow_foreground_insert_fallback {
        log::warn!(
            "[coord] original insertion target is not foreground; inserting into current foreground by LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK"
        );
        #[cfg(target_os = "windows")]
        {
            let ime_target = capture_ime_submit_target();
            let result = insert_with_windows_ime_first(
                inner,
                current_session_id,
                &polished,
                restore_clipboard,
                allow_non_tsf_insertion_fallback,
                allow_clipboard_fallback,
                paste_shortcut,
                ime_target,
            )
            .await;
            (result.status, false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            if allow_clipboard_fallback {
                (
                    inner
                        .inserter
                        .insert(&polished, restore_clipboard, paste_shortcut),
                    false,
                )
            } else {
                (InsertStatus::Failed, false)
            }
        }
    } else {
        if allow_clipboard_fallback {
            log::warn!(
                "[coord] original insertion target is not foreground; retaining final output without paste"
            );
            (inner.inserter.copy_fallback(&polished), false)
        } else {
            log::warn!(
                "[coord] original insertion target is not foreground and final clipboard retention is disabled"
            );
            (InsertStatus::Failed, false)
        }
    };
    restore_prepared_windows_ime_session(inner, current_session_id);

    let clipboard_retention_satisfied = if retain_plain_dictation {
        if inner.inserter.copy_fallback(&polished) == InsertStatus::Failed {
            log::warn!(
                "[coord] final clipboard retention failed session_id={} chars={}",
                current_session_id,
                polished.chars().count()
            );
            false
        } else {
            log::info!(
                "[coord] final clipboard retention complete session_id={} chars={}",
                current_session_id,
                polished.chars().count()
            );
            true
        }
    } else {
        true
    };

    let mut post_dictation_key_result = "not_eligible";
    if let Some(binding) = should_send_post_dictation_key(
        prefs.send_key_after_dictation,
        prefs.post_dictation_key,
        status,
        !polished.trim().is_empty(),
        focus_ready_for_paste,
        clipboard_retention_satisfied,
        translation_active,
    ) {
        tokio::time::sleep(POST_DICTATION_KEY_DELAY).await;
        if !restore_focus_target_if_possible(focus_target) {
            post_dictation_key_result = "original_target_lost";
            log::warn!(
                "[coord] post-dictation shortcut skipped session_id={} reason=original_target_lost",
                current_session_id
            );
        } else if claim_post_dictation_key(inner, current_session_id) {
            match crate::shortcut_dispatch::send_shortcut(&binding) {
                Ok(()) => {
                    post_dictation_key_result = "sent";
                    log::info!(
                        "[coord] post-dictation shortcut sent session_id={} shortcut={} original_target_restored=true",
                        current_session_id,
                        binding.display_label()
                    );
                }
                Err(error) => {
                    post_dictation_key_result = "failed";
                    log::warn!(
                        "[coord] post-dictation shortcut failed session_id={} shortcut={}: {error}",
                        current_session_id,
                        binding.display_label()
                    );
                }
            }
        } else {
            post_dictation_key_result = "already_claimed_or_stale";
            log::warn!(
                "[coord] post-dictation shortcut skipped session_id={} reason=already_claimed_or_stale",
                current_session_id
            );
        }
    }
    let clipboard_result = if !retain_plain_dictation {
        "disabled"
    } else if clipboard_retention_satisfied {
        "stored"
    } else {
        "failed"
    };
    let stop_to_done_ms = take_stop_to_done_ms(inner, current_session_id);
    if let Some(ms) = stop_to_done_ms {
        log::info!(
            "[coord] stop_to_done_ms={} session_id={} insertion_status={:?} polish_failed={} clipboard={}",
            ms,
            current_session_id,
            status,
            polish_error.is_some(),
            clipboard_result
        );
    }
    log::info!(
        "[coord] final completion actions session_id={} chars={} insertion_status={:?} target_confirmed={} target_restored={} user_stop={} clipboard={} post_key={} stop_to_done_ms={:?}",
        current_session_id,
        polished.chars().count(),
        status,
        original_target_confirmed,
        focus_ready_for_paste,
        user_initiated_stop,
        clipboard_result,
        post_dictation_key_result,
        stop_to_done_ms
    );

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
    let transcript_error_code = error_code.clone();
    let tsf_required_insert_failed = error_code.as_deref() == Some("windowsImeTsfRequired");
    let device_processing_succeeded =
        device_processing_final_succeeded(status, error_code.as_deref());
    let device_processing_success_reason = if error_code.as_deref() == Some("polishFailed") {
        "dictation_processing_done_raw_inserted"
    } else {
        "dictation_processing_done"
    };
    if device_processing_succeeded {
        device_ai_processing.complete_success_async(device_processing_success_reason);
    } else {
        device_ai_processing.complete_warning_async("dictation_processing_warning");
    }

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
    store_embedded_audio_final_result(
        inner,
        crate::embedded_audio::EmbeddedAudioTranscriptResult {
            session_id: current_session_id.to_string(),
            raw_transcript: raw.text.clone(),
            final_text: polished.clone(),
            error_code: transcript_error_code,
        },
    );
    // LLM auth 熔断打开时发一次可见提示（每个凭据指纹一次）：后续会话仍走
    // 静默原文的合同路径，但用户必须有机会知道「润色已失效」——2026-08-05
    // owner 的 key 401 了一整天、48 次听写全走原文而无人察觉。仅在本会话
    // 需要 LLM 润色时提示（raw 原文用户与坏 key 无关，不打扰）。
    if (mode != PolishMode::Raw || raw_uses_llm) && !translation_active {
        if let Ok(fingerprint) = current_llm_auth_fingerprint() {
            if current_llm_auth_is_rejected(fingerprint)
                && take_llm_auth_rejection_notice(fingerprint)
            {
                let notice_inner = Arc::clone(inner);
                async_runtime::spawn(async move {
                    // 等本次 Done 先落位，再弹出 2.5s 自消的错误胶囊。
                    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                    log::info!("[coord] LLM auth rejection notice shown (once per credential set)");
                    emit_capsule(
                        &notice_inner,
                        CapsuleState::Error,
                        0.0,
                        0,
                        Some("润色 API key 失效，已改用原文上屏；请到设置更新 key".to_string()),
                        None,
                    );
                });
            }
        }
    }
    let done_message = if status == InsertStatus::Inserted
        && !polish_error.is_some()
        && !tsf_required_insert_failed
        && !wayland_session
    {
        None
    } else if tsf_required_insert_failed {
        if clipboard_retention_satisfied && retain_plain_dictation {
            Some("TSF 未上屏，内容在剪贴板，请 Ctrl+V".to_string())
        } else {
            Some("TSF 未上屏，已禁止非 TSF 兜底".to_string())
        }
    } else if wayland_session {
        wayland_done_message(status, polish_error.is_some())
    } else {
        default_done_message(
            status,
            polish_error.is_some(),
            clipboard_retention_satisfied && retain_plain_dictation,
        )
    };

    apply_and_publish_dictation_event(
        inner,
        DictationEvent::InsertionComplete {
            session_id: current_session_id,
        },
        0.0,
        done_message,
        Some(inserted_chars),
    );

    schedule_capsule_idle(
        inner,
        CAPSULE_SUCCESS_HIDE_DELAY_MS,
        Some(current_session_id),
    );

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
    discard_terminal_wake_continuation(inner);
    if embedded_ble_host_cancel_context_active(inner) {
        cancel_embedded_ble_session_through_actor(inner);
        return;
    }
    cancel_session_direct(inner);
}

/// OTA owns BLE exclusively — cancel any dictation/wake path and force capsule idle.
pub(super) fn suppress_dictation_pipeline_for_firmware_ota(inner: &Arc<Inner>) {
    log::info!("[firmware-ota] suppressing dictation/capsule pipeline for exclusive OTA transfer");
    clear_hidden_automatic_candidate();
    cancel_session(inner);
    // Force hide even when cancel is a no-op (Idle with no capture flag).
    emit_capsule(inner, CapsuleState::Idle, 0.0, 0, None, None);
}

fn cancel_embedded_ble_session_through_actor(inner: &Arc<Inner>) {
    let session_id = inner.state.lock().session_id;
    record_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::CancelCommand,
        Some(session_id),
        "cancel command applied to embedded BLE session",
    );
    let cancelled = begin_cancel_session_transition(inner);
    let firmware_cancel_sent = cancelled.as_ref().is_some_and(|(session_id, phase, _)| {
        request_embedded_ble_firmware_cancel_on_active_recording(*session_id, *phase)
    });
    if !firmware_cancel_sent {
        set_device_ai_processing_async(inner, false, "embedded_session_cancel");
    }
    if cancelled.is_none() {
        if request_embedded_ble_capture_cancel_flag(inner) {
            log::info!("[coord] embedded BLE capture cancel requested without active session");
        }
        if inner.state.lock().phase == SessionPhase::Idle {
            emit_capsule(inner, CapsuleState::Idle, 0.0, 0, None, None);
        }
    }
    finish_cancel_session_after_transition(inner, cancelled, true);
}

fn request_embedded_ble_firmware_cancel_on_active_recording(
    session_id: SessionId,
    phase: SessionPhase,
) -> bool {
    if !matches!(phase, SessionPhase::Starting | SessionPhase::Listening) {
        return false;
    }

    #[cfg(test)]
    {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            "firmware_cancel_skipped_test",
            format!("session_id={session_id} phase={phase:?}"),
        );
        return true;
    }

    #[cfg(not(test))]
    {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            "firmware_cancel_requested",
            format!("session_id={session_id} phase={phase:?}"),
        );
        // Send before local capture teardown; once cancel closes notify, the active
        // capture control queue is gone and a queued firmware cancel can time out.
        match crate::embedded_ble::send_recording_control_cancel(
            EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
        ) {
            Ok(()) => {
                crate::timeline::mark(
                    "backend.embedded_ble_session_actor",
                    "firmware_cancel_sent",
                    format!("session_id={session_id} phase={phase:?}"),
                );
                log::info!(
                    "[coord] embedded BLE firmware cancel sent session_id={session_id} phase={phase:?}"
                );
                true
            }
            Err(err) => {
                crate::timeline::mark(
                    "backend.embedded_ble_session_actor",
                    "firmware_cancel_failed",
                    format!("session_id={session_id} phase={phase:?} error={err}"),
                );
                log::warn!(
                    "[coord] embedded BLE firmware cancel failed session_id={session_id} phase={phase:?}: {err}"
                );
                false
            }
        }
    }
}

fn cancel_session_direct(inner: &Arc<Inner>) {
    let cancelled = begin_cancel_session_transition(inner);
    finish_cancel_session_after_transition(inner, cancelled, false);
}

fn begin_cancel_session_transition(
    inner: &Arc<Inner>,
) -> Option<(SessionId, SessionPhase, DictationTransition)> {
    let (session_id, phase, transition) = {
        let mut state = inner.state.lock();
        let session_id = state.session_id;
        let phase = state.phase;
        let transition = apply_dictation_event(&mut state, DictationEvent::Cancel { session_id });
        if matches!(transition, DictationTransition::Ignored { .. }) {
            if phase == SessionPhase::Inserting {
                log::info!("[coord] cancel ignored — already in Inserting phase, can't undo paste");
            }
            return None;
        }
        (session_id, phase, transition)
    };
    Some((session_id, phase, transition))
}

fn finish_cancel_session_after_transition(
    inner: &Arc<Inner>,
    cancelled: Option<(SessionId, SessionPhase, DictationTransition)>,
    embedded_ble_actor_owned: bool,
) {
    let Some((session_id, phase, transition)) = cancelled else {
        return;
    };

    stop_recorder_for_session(inner, session_id);
    cancel_asr_for_session(inner, session_id);
    restore_prepared_windows_ime_session(inner, session_id);
    let capture_cancelled = if embedded_ble_actor_owned {
        request_embedded_ble_capture_cancel_flag(inner)
    } else {
        request_embedded_ble_capture_cancel(inner)
    };
    if capture_cancelled {
        log::info!("[coord] embedded BLE capture cancel requested");
    }
    publish_dictation_transition(inner, transition, 0.0, None, None);
    log::info!("[coord] session cancelled (was {:?})", phase);
    schedule_capsule_idle(inner, CAPSULE_AUTO_HIDE_DELAY_MS, Some(session_id));
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
#[path = "dictation_tests.rs"]
mod tests;
