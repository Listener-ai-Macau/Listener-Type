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
use super::resources::*;
use super::*;

/// 同一个 hotkey 边沿之间的最小间隔。低于此阈值的连按整体作为误触丢弃 ——
/// 避免微动开关回弹 / 用户手抖双击造成的空转写报错和 ASR session 抢资源。
pub(super) const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(250);
const EMBEDDED_AUDIO_FEED_CHUNK_BYTES: usize = 3_200;
const EMBEDDED_AUDIO_TARGET_RMS: f64 = 2_300.0;
const EMBEDDED_AUDIO_MAX_GAIN: f64 = 16.0;
const EMBEDDED_AUDIO_MIN_GAIN: f64 = 1.05;
const EMBEDDED_AUDIO_VISUAL_RMS_REFERENCE: f64 = 700.0;
// Volcengine streaming is latency sensitive. Calibrate from the first voiced
// 100 ms block, then only raise that session gain when later confirmed speech
// is quieter. Ignore the noisiest one percent of a block while calibrating:
// a PDM impulse must not make an otherwise quiet spoken block look loud.
const EMBEDDED_AUDIO_STREAMING_SPEECH_RMS: f64 = 120.0;
const EMBEDDED_AUDIO_STREAMING_QUIET_SPEECH_RMS: f64 = 45.0;
const EMBEDDED_AUDIO_STREAMING_QUIET_SPEECH_PEAK: u16 = 256;
const EMBEDDED_AUDIO_STREAMING_AGC_PEAK_HEADROOM: f64 = 0.90;
const EMBEDDED_AUDIO_STREAMING_AGC_SIGNAL_PERCENTILE_NUMERATOR: usize = 99;
const EMBEDDED_AUDIO_STREAMING_AGC_SIGNAL_PERCENTILE_DENOMINATOR: usize = 100;
// Firmware preserves microphone headroom instead of pre-amplifying it, so a
// quiet first voiced block may need the full bounded streaming gain. Each
// later block still has its own peak guard before it reaches the provider.
const EMBEDDED_AUDIO_STREAMING_INITIAL_MAX_GAIN: f64 = EMBEDDED_AUDIO_MAX_GAIN;
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
const WAKE_DIAGNOSTIC_MAX_CANDIDATES: usize = 5;
const WAKE_DIAGNOSTIC_MAX_PCM_BYTES: usize = 5 * 16_000 * 2;
const POST_DICTATION_KEY_DELAY: Duration = Duration::from_millis(60);

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

fn set_volcengine_preview_callbacks(
    asr: &Arc<VolcengineStreamingASR>,
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let inner_for_stream = Arc::clone(inner);
    asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
        update_embedded_audio_partial_preview(&inner_for_stream, session_id, text);
    })));

    let inner_for_partial = Arc::clone(inner);
    asr.set_final_intermediate_transcript_callback(Some(Arc::new(move |update| {
        update_embedded_audio_partial_preview_from_final_supplement(
            &inner_for_partial,
            session_id,
            update,
        );
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
    asr.open_session().await?;
    log::info!(
        "[asr] authoritative optimized-bidirectional ASR ready; preview and final share one provider session elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(())
}

fn apply_and_publish_dictation_event(
    inner: &Arc<Inner>,
    event: DictationEvent,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    let transition = {
        let mut state = inner.state.lock();
        apply_dictation_event(&mut state, event)
    };
    publish_dictation_transition(inner, transition, level, message, inserted_chars)
}

fn apply_embedded_ble_session_actor_dictation_event(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: SessionId,
    detail: impl Into<String>,
    event: DictationEvent,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    apply_embedded_ble_session_actor_dictation_event_with_trace(
        inner,
        command,
        session_id,
        detail,
        true,
        event,
        level,
        message,
        inserted_chars,
    )
}

fn apply_embedded_ble_session_actor_dictation_event_with_trace(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: SessionId,
    detail: impl Into<String>,
    trace_timeline: bool,
    event: DictationEvent,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    dispatch_embedded_ble_session_actor_command_with_trace(
        inner,
        command,
        Some(session_id),
        detail,
        trace_timeline,
        |_| apply_and_publish_dictation_event(inner, event, level, message, inserted_chars),
    )
}

fn embedded_ble_actor_context_active(inner: &Arc<Inner>) -> bool {
    inner.embedded_ble_cancel_flag.lock().is_some() || inner.embedded_audio_stats.lock().is_some()
}

fn embedded_ble_host_recording_control_context_active(inner: &Arc<Inner>) -> bool {
    embedded_ble_actor_context_active(inner)
        || inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
}

fn embedded_ble_host_cancel_context_active(inner: &Arc<Inner>) -> bool {
    if embedded_ble_actor_context_active(inner) {
        return true;
    }
    let phase = inner.state.lock().phase;
    inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
        && matches!(
            phase,
            SessionPhase::Starting | SessionPhase::Listening | SessionPhase::Processing
        )
}

fn publish_dictation_pipeline_error(
    inner: &Arc<Inner>,
    session_id: SessionId,
    message: String,
) -> bool {
    apply_and_publish_dictation_event(
        inner,
        DictationEvent::PipelineError { session_id },
        0.0,
        Some(message),
        None,
    )
}

fn publish_dictation_timeout(inner: &Arc<Inner>, session_id: SessionId, message: String) -> bool {
    apply_and_publish_dictation_event(
        inner,
        DictationEvent::Timeout { session_id },
        0.0,
        Some(message),
        None,
    )
}

fn schedule_actionable_error_capsule_idle(inner: &Arc<Inner>, session_id: SessionId) {
    schedule_capsule_idle(
        inner,
        CAPSULE_ACTIONABLE_ERROR_HIDE_DELAY_MS,
        Some(session_id),
    );
}

fn schedule_empty_transcript_capsule_idle(inner: &Arc<Inner>, session_id: SessionId) {
    schedule_capsule_idle(
        inner,
        CAPSULE_EMPTY_TRANSCRIPT_HIDE_DELAY_MS,
        Some(session_id),
    );
}

fn publish_embedded_ble_asr_final(
    inner: &Arc<Inner>,
    session_id: SessionId,
    transcript_empty: bool,
    message: Option<String>,
) -> bool {
    let detail = format!("transcript_empty={transcript_empty}");
    let published = if embedded_ble_actor_context_active(inner) {
        apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::AsrFinal,
            session_id,
            detail,
            DictationEvent::AsrFinal {
                session_id,
                transcript_empty,
            },
            0.0,
            message,
            None,
        )
    } else {
        apply_and_publish_dictation_event(
            inner,
            DictationEvent::AsrFinal {
                session_id,
                transcript_empty,
            },
            0.0,
            message,
            None,
        )
    };
    if published {
        crate::observability::record_embedded_audio_final(session_id);
    }
    published
}

fn finish_dictation_pipeline_error(
    inner: &Arc<Inner>,
    session_id: SessionId,
    message: String,
) -> bool {
    let observability_message = message.clone();
    set_device_ai_processing_warning_async(
        inner,
        "dictation_pipeline_error",
        Duration::from_millis(0),
    );
    if !publish_dictation_pipeline_error(inner, session_id, message)
        && cleanup_cancelled_processing_session(inner, session_id)
    {
        return false;
    }
    restore_prepared_windows_ime_session(inner, session_id);
    schedule_actionable_error_capsule_idle(inner, session_id);
    crate::observability::record_embedded_audio_failure(session_id, &observability_message);
    true
}

fn finish_dictation_timeout(inner: &Arc<Inner>, session_id: SessionId, message: String) -> bool {
    set_device_ai_processing_warning_async(inner, "dictation_timeout", Duration::from_millis(0));
    let published = if embedded_ble_actor_context_active(inner) {
        apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::Timeout,
            session_id,
            message.clone(),
            DictationEvent::Timeout { session_id },
            0.0,
            Some(message),
            None,
        )
    } else {
        publish_dictation_timeout(inner, session_id, message)
    };
    if !published && cleanup_cancelled_processing_session(inner, session_id) {
        return false;
    }
    restore_prepared_windows_ime_session(inner, session_id);
    schedule_actionable_error_capsule_idle(inner, session_id);
    crate::observability::record_embedded_audio_timeout(session_id);
    true
}

fn cleanup_cancelled_processing_session(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let should_cleanup = {
        let state = inner.state.lock();
        state.session_id == session_id && state.cancelled && state.phase == SessionPhase::Processing
    };
    if !should_cleanup {
        return false;
    }

    restore_prepared_windows_ime_session(inner, session_id);
    clear_embedded_audio_stats(inner);
    {
        let mut state = inner.state.lock();
        if state.session_id != session_id
            || !state.cancelled
            || state.phase != SessionPhase::Processing
        {
            return false;
        }
        state.phase = SessionPhase::Idle;
        state.focus_target = None;
    }
    true
}

fn clear_embedded_audio_stats(inner: &Arc<Inner>) {
    *inner.embedded_audio_stats.lock() = None;
}

fn clear_embedded_audio_partial_preview(inner: &Arc<Inner>) {
    *inner.embedded_audio_partial_preview.lock() = None;
    *inner.embedded_audio_last_capsule_level.lock() = 0.0;
}

fn set_embedded_audio_wake_phrase_filter(
    inner: &Arc<Inner>,
    session_id: SessionId,
    phrase: String,
) {
    *inner.embedded_audio_wake_phrase_filter.lock() = Some((session_id, phrase));
}

fn clear_embedded_audio_wake_phrase_filter(inner: &Arc<Inner>) {
    *inner.embedded_audio_wake_phrase_filter.lock() = None;
}

fn current_embedded_audio_capsule_level(inner: &Arc<Inner>) -> f32 {
    *inner.embedded_audio_last_capsule_level.lock()
}

fn remember_embedded_audio_capsule_level(inner: &Arc<Inner>, level: f32) -> f32 {
    let clamped = level.clamp(0.0, 1.0);
    *inner.embedded_audio_last_capsule_level.lock() = clamped;
    clamped
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

fn embedded_ble_processing_sync_disabled() -> bool {
    std::env::var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn device_ai_processing_io_allowed() -> bool {
    !cfg!(test)
}

fn should_sync_device_ai_processing(inner: &Arc<Inner>) -> bool {
    device_ai_processing_io_allowed()
        && embedded_ble_host_recording_control_context_active(inner)
        && !embedded_ble_processing_sync_disabled()
}

fn set_device_ai_processing_async(inner: &Arc<Inner>, active: bool, reason: &'static str) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let session_id = inner.state.lock().session_id;
    async_runtime::spawn_blocking(move || {
        match crate::embedded_ble::send_recording_processing_state(active, Duration::from_secs(2)) {
            Ok(()) => log::info!(
                "[embedded-ble] device AI processing LED synced active={active} reason={reason} session_id={session_id}"
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED sync failed active={active} reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

fn device_ai_processing_completion_delay(started_at: Option<Instant>, now: Instant) -> Duration {
    let Some(started_at) = started_at else {
        return Duration::from_millis(0);
    };
    let min_visible = Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS);
    min_visible.saturating_sub(now.saturating_duration_since(started_at))
}

fn schedule_device_ai_processing_max_visible_timeout(
    inner: &Arc<Inner>,
    cancel: Arc<AtomicBool>,
    reason: &'static str,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    let inner = Arc::clone(inner);
    async_runtime::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS));
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let session_id = inner.state.lock().session_id;
        if session_id != expected_session_id {
            log::info!(
                "[embedded-ble] skipped stale device AI processing LED max-visible timeout reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
            );
            return;
        }
        if !should_sync_device_ai_processing(&inner) {
            log::info!(
                "[embedded-ble] skipped device AI processing LED max-visible timeout reason={reason} session_id={session_id}; processing sync no longer active"
            );
            return;
        }
        match crate::embedded_ble::send_recording_processing_done(Duration::from_secs(2)) {
            Ok(()) => log::warn!(
                "[embedded-ble] device AI processing LED max-visible timeout completed reason={reason} session_id={session_id} max_visible_ms={DEVICE_AI_PROCESSING_MAX_VISIBLE_MS}"
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED max-visible timeout completion failed reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

fn set_device_ai_processing_done_async(inner: &Arc<Inner>, reason: &'static str, delay: Duration) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    let inner = Arc::clone(inner);
    async_runtime::spawn_blocking(move || {
        if delay > Duration::from_millis(0) {
            std::thread::sleep(delay);
        }
        let session_id = inner.state.lock().session_id;
        if session_id != expected_session_id {
            log::info!(
                "[embedded-ble] skipped stale device AI processing LED completion reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
            );
            return;
        }
        match crate::embedded_ble::send_recording_processing_done(Duration::from_secs(2)) {
            Ok(()) => log::info!(
                "[embedded-ble] device AI processing LED completed reason={reason} session_id={session_id} delayed_ms={}",
                delay.as_millis()
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED completion sync failed reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

async fn set_device_ai_processing_done_wait(
    inner: &Arc<Inner>,
    reason: &'static str,
    delay: Duration,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    if delay > Duration::from_millis(0) {
        tokio::time::sleep(delay).await;
    }
    let session_id = inner.state.lock().session_id;
    if session_id != expected_session_id {
        log::info!(
            "[embedded-ble] skipped stale device AI processing LED completion reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
        );
        return;
    }
    let result = async_runtime::spawn_blocking(move || {
        crate::embedded_ble::send_recording_processing_done(Duration::from_secs(2))
    })
    .await;
    match result {
        Ok(Ok(())) => log::info!(
            "[embedded-ble] device AI processing LED completed reason={reason} session_id={session_id} delayed_ms={}",
            delay.as_millis()
        ),
        Ok(Err(err)) => log::warn!(
            "[embedded-ble] device AI processing LED completion sync failed reason={reason} session_id={session_id}: {err}"
        ),
        Err(err) => log::warn!(
            "[embedded-ble] device AI processing LED completion task failed reason={reason} session_id={session_id}: {err}"
        ),
    }
}

fn set_device_ai_processing_warning_async(
    inner: &Arc<Inner>,
    reason: &'static str,
    delay: Duration,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    let inner = Arc::clone(inner);
    async_runtime::spawn_blocking(move || {
        if delay > Duration::from_millis(0) {
            std::thread::sleep(delay);
        }
        let session_id = inner.state.lock().session_id;
        if session_id != expected_session_id {
            log::info!(
                "[embedded-ble] skipped stale device AI processing LED warning reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
            );
            return;
        }
        match crate::embedded_ble::send_recording_processing_warning(Duration::from_secs(2)) {
            Ok(()) => log::info!(
                "[embedded-ble] device AI processing LED warning reason={reason} session_id={session_id} delayed_ms={}",
                delay.as_millis()
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED warning sync failed reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

async fn set_device_ai_processing_warning_wait(
    inner: &Arc<Inner>,
    reason: &'static str,
    delay: Duration,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    if delay > Duration::from_millis(0) {
        tokio::time::sleep(delay).await;
    }
    let session_id = inner.state.lock().session_id;
    if session_id != expected_session_id {
        log::info!(
            "[embedded-ble] skipped stale device AI processing LED warning reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
        );
        return;
    }
    let result = async_runtime::spawn_blocking(move || {
        crate::embedded_ble::send_recording_processing_warning(Duration::from_secs(2))
    })
    .await;
    match result {
        Ok(Ok(())) => log::info!(
            "[embedded-ble] device AI processing LED warning reason={reason} session_id={session_id} delayed_ms={}",
            delay.as_millis()
        ),
        Ok(Err(err)) => log::warn!(
            "[embedded-ble] device AI processing LED warning sync failed reason={reason} session_id={session_id}: {err}"
        ),
        Err(err) => log::warn!(
            "[embedded-ble] device AI processing LED warning task failed reason={reason} session_id={session_id}: {err}"
        ),
    }
}

fn device_ai_processing_completion_allowed(active: bool, completed: bool) -> bool {
    active && !completed
}

struct DeviceAiProcessingGuard {
    inner: Arc<Inner>,
    active: bool,
    completed: bool,
    started_at: Option<Instant>,
    max_visible_cancel: Option<Arc<AtomicBool>>,
}

impl DeviceAiProcessingGuard {
    fn defer(inner: &Arc<Inner>) -> Self {
        Self {
            inner: Arc::clone(inner),
            active: false,
            completed: false,
            started_at: None,
            max_visible_cancel: None,
        }
    }

    fn start_if_needed(&mut self, reason: &'static str) {
        if self.completed || self.active || !should_sync_device_ai_processing(&self.inner) {
            return;
        }
        set_device_ai_processing_async(&self.inner, true, reason);
        let cancel = Arc::new(AtomicBool::new(false));
        schedule_device_ai_processing_max_visible_timeout(
            &self.inner,
            Arc::clone(&cancel),
            "dictation_processing_max_visible_timeout",
        );
        self.max_visible_cancel = Some(cancel);
        self.active = true;
        self.started_at = Some(Instant::now());
    }

    fn cancel_max_visible_timeout(&mut self) {
        if let Some(cancel) = self.max_visible_cancel.take() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    async fn complete_success(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_done_wait(&self.inner, reason, delay).await;
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }

    fn complete_success_async(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_done_async(&self.inner, reason, delay);
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }

    async fn complete_warning(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_warning_wait(&self.inner, reason, delay).await;
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }

    fn complete_warning_async(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_warning_async(&self.inner, reason, delay);
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }
}

impl Drop for DeviceAiProcessingGuard {
    fn drop(&mut self) {
        if self.active && !self.completed {
            self.cancel_max_visible_timeout();
            set_device_ai_processing_async(&self.inner, false, "dictation_processing_end");
        }
    }
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

fn request_embedded_ble_capture_cancel_flag(inner: &Arc<Inner>) -> bool {
    let flag = inner.embedded_ble_cancel_flag.lock().clone();
    if let Some(flag) = flag {
        flag.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

fn request_embedded_ble_capture_cancel(inner: &Arc<Inner>) -> bool {
    if inner.embedded_ble_cancel_flag.lock().is_none() {
        return false;
    }
    let session_id = inner.state.lock().session_id;
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::CancelCommand,
        Some(session_id),
        "cancel requested for active BLE capture",
        |_| request_embedded_ble_capture_cancel_flag(inner),
    )
}

pub(super) fn current_embedded_audio_partial_preview(inner: &Arc<Inner>) -> Option<String> {
    inner.embedded_audio_partial_preview.lock().clone()
}

fn reconcile_final_transcript_with_preview_hotwords(
    final_text: &str,
    preview: &str,
    hotwords: &[DictionaryHotword],
) -> String {
    let preview_key = ascii_alphanumeric_key(preview);
    if preview_key.is_empty() {
        return final_text.to_string();
    }

    let mut corrected = final_text.to_string();
    for hotword in hotwords {
        if !hotword.enabled {
            continue;
        }
        let phrase = hotword.phrase.trim();
        if phrase.len() < 3 || !phrase.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            continue;
        }
        let phrase_key = phrase.to_ascii_lowercase();
        if !preview_key.contains(&phrase_key)
            || ascii_alphanumeric_key(&corrected).contains(&phrase_key)
        {
            continue;
        }

        let replacement =
            spaced_ascii_token_spans(&corrected)
                .into_iter()
                .find_map(|(start, end, candidate)| {
                    (ascii_edit_distance_at_most_one(&candidate, &phrase_key) == Some(1))
                        .then_some((start, end))
                });
        if let Some((start, end)) = replacement {
            corrected.replace_range(start..end, phrase);
            log::info!(
                "[coord] restored preview-confirmed ASR hotword in final transcript: {phrase}"
            );
        }
    }
    corrected
}

fn ascii_alphanumeric_key(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn spaced_ascii_token_spans(text: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let (start, first) = chars[index];
        if !first.is_ascii_alphanumeric() {
            index += 1;
            continue;
        }

        let mut end = start + first.len_utf8();
        let mut token = String::from(first.to_ascii_lowercase());
        index += 1;
        loop {
            let separator_start = index;
            while index < chars.len() && chars[index].1.is_ascii_whitespace() {
                index += 1;
            }
            if index < chars.len() && chars[index].1.is_ascii_alphanumeric() {
                let (next_offset, next) = chars[index];
                token.push(next.to_ascii_lowercase());
                end = next_offset + next.len_utf8();
                index += 1;
                continue;
            }
            index = if separator_start == index {
                index
            } else {
                separator_start
            };
            break;
        }
        spans.push((start, end, token));
    }
    spans
}

fn ascii_edit_distance_at_most_one(left: &str, right: &str) -> Option<usize> {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len().abs_diff(right.len()) > 1 {
        return None;
    }

    let (mut left_index, mut right_index, mut edits) = (0, 0, 0);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            left_index += 1;
            right_index += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return None;
        }
        if left.len() > right.len() {
            left_index += 1;
        } else if right.len() > left.len() {
            right_index += 1;
        } else {
            left_index += 1;
            right_index += 1;
        }
    }
    edits += left.len().saturating_sub(left_index) + right.len().saturating_sub(right_index);
    (edits <= 1).then_some(edits)
}

fn update_embedded_audio_partial_preview(inner: &Arc<Inner>, session_id: SessionId, text: String) {
    let preview = filter_dictation_preview_text(inner, session_id, &text);
    if preview.is_empty() {
        return;
    }
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!("chars={}", preview.chars().count()),
        |_| {
            let mut slot = inner.embedded_audio_partial_preview.lock();
            let Some(provider_preview) = provider_preview_change(slot.as_deref(), &preview) else {
                return false;
            };
            *slot = Some(provider_preview.clone());
            let emitted =
                emit_embedded_audio_partial_preview_if_active(inner, session_id, provider_preview);
            if emitted {
                crate::observability::record_embedded_audio_preview_published(
                    session_id,
                    crate::observability::PreviewSource::ProviderStream,
                    embedded_audio_stop_feedback_latched(inner),
                );
            }
            emitted
        },
    );
}

fn update_embedded_audio_partial_preview_from_final_supplement(
    inner: &Arc<Inner>,
    session_id: SessionId,
    update: crate::asr::volcengine::FinalIntermediateTranscript,
) {
    let authoritative_two_pass = update.authoritative_two_pass;
    let preview = filter_dictation_preview_text(inner, session_id, &update.text);
    if preview.is_empty() {
        return;
    }
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!(
            "final_supplement chars={} authoritative_two_pass={}",
            preview.chars().count(),
            authoritative_two_pass
        ),
        |_| {
            let mut slot = inner.embedded_audio_partial_preview.lock();
            let Some(provider_preview) = provider_preview_change(slot.as_deref(), &preview) else {
                return false;
            };
            *slot = Some(provider_preview.clone());
            let emitted =
                emit_embedded_audio_partial_preview_if_active(inner, session_id, provider_preview);
            if emitted {
                crate::observability::record_embedded_audio_preview_published(
                    session_id,
                    crate::observability::PreviewSource::FinalSupplement,
                    embedded_audio_stop_feedback_latched(inner),
                );
            }
            emitted
        },
    );
}

// `stream` and `two_pass` originate from the same authoritative ASR session.
// A newer non-identical candidate must replace the capsule text, including an
// early rewrite; only an exact duplicate is safe to suppress.
fn provider_preview_change(current: Option<&str>, candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() || current.is_some_and(|value| value.trim() == candidate) {
        return None;
    }
    Some(candidate.to_string())
}

fn stabilize_embedded_audio_partial_preview(
    current: Option<&str>,
    candidate: &str,
) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }
    let Some(current) = current.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(candidate.to_string());
    };
    let current_key = embedded_audio_partial_preview_stability_key(current);
    let candidate_key = embedded_audio_partial_preview_stability_key(candidate);
    if current_key == candidate_key || candidate_key.is_empty() {
        return None;
    }
    if current_key.is_empty() {
        return Some(candidate.to_string());
    }
    if candidate_key.starts_with(&current_key) {
        let current_key_chars = current_key.chars().count();
        if embedded_audio_partial_preview_repeats_recent_short_tail(
            &current_key,
            &candidate_key,
            current_key_chars,
        ) {
            return None;
        }
        return stitch_embedded_audio_partial_preview(current, candidate, current_key_chars);
    }
    if current_key.starts_with(&candidate_key) {
        return None;
    }

    let current_chars = current_key.chars().count();
    let candidate_chars = candidate_key.chars().count();
    if current_chars <= 4 && candidate_chars >= current_chars.saturating_add(3) {
        return Some(candidate.to_string());
    }

    None
}

fn stabilize_embedded_audio_final_supplemental_preview(
    current: Option<&str>,
    candidate: &str,
) -> Option<String> {
    stabilize_embedded_audio_final_supplemental_preview_with_provider_authority(
        current, candidate, false,
    )
}

fn stabilize_embedded_audio_final_supplemental_preview_with_provider_authority(
    current: Option<&str>,
    candidate: &str,
    authoritative_two_pass: bool,
) -> Option<String> {
    const FINAL_SUPPLEMENT_SEED_CHARS: usize = 2;
    const SHORT_PREFIX_REPAIR_MAX_CURRENT_CHARS: usize = 12;
    const SHORT_PREFIX_REPAIR_MAX_REWRITE_CHARS: usize = 2;
    const SHORT_PREFIX_REPAIR_MIN_EXTENSION_CHARS: usize = 3;
    const SHORT_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS: usize = 2;
    const LONG_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS: usize = 12;
    const LONG_PREFIX_REPAIR_MIN_EXTENSION_CHARS: usize = 6;
    const LONG_SHIFTED_REPAIR_MIN_TOTAL_GROWTH_CHARS: usize = 2;
    const AUTHORITATIVE_EARLY_REWRITE_MIN_SHARED_PREFIX_CHARS: usize = 2;
    const AUTHORITATIVE_EARLY_REWRITE_MIN_EXTENSION_CHARS: usize = 4;
    const AUTHORITATIVE_EARLY_REWRITE_MAX_EDIT_CHARS: usize = 4;
    const AUTHORITATIVE_LONG_REVISION_MIN_SHARED_PREFIX_CHARS: usize = 12;
    const AUTHORITATIVE_LONG_REVISION_MAX_LENGTH_DELTA_CHARS: usize = 4;
    const AUTHORITATIVE_LONG_REVISION_MAX_EDIT_CHARS: usize = 4;
    const AUTHORITATIVE_REWRITE_MIN_SHARED_PREFIX_CHARS: usize = 6;
    const AUTHORITATIVE_REWRITE_MIN_EXTENSION_CHARS: usize = 8;

    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }
    let candidate_key = embedded_audio_partial_preview_stability_key(candidate);
    if candidate_key.is_empty() {
        return None;
    }
    let candidate_key_chars = candidate_key.chars().count();
    let Some(current) = current.map(str::trim).filter(|value| !value.is_empty()) else {
        return (candidate_key_chars >= FINAL_SUPPLEMENT_SEED_CHARS).then(|| candidate.to_string());
    };
    let current_key = embedded_audio_partial_preview_stability_key(current);
    if current_key.is_empty() {
        return None;
    }
    if authoritative_two_pass && current_key != candidate_key {
        log::info!(
            "[coord] applied provider-authoritative two-pass preview correction current_chars={} candidate_chars={}",
            current_key.chars().count(),
            candidate_key_chars
        );
        return Some(candidate.to_string());
    }
    if current_key == candidate_key {
        return embedded_audio_final_supplement_adds_decorative_progress(current, candidate)
            .then(|| candidate.to_string());
    }
    if authoritative_two_pass
        && embedded_audio_final_supplement_is_brief_bounded_revision(&current_key, &candidate_key)
    {
        return Some(candidate.to_string());
    }
    let current_key_chars = current_key.chars().count();
    if candidate_key.starts_with(&current_key) {
        if embedded_audio_partial_preview_repeats_recent_short_tail(
            &current_key,
            &candidate_key,
            current_key_chars,
        ) {
            return None;
        }
        return stitch_embedded_audio_partial_preview(current, candidate, current_key_chars);
    }
    if current_key.starts_with(&candidate_key) {
        return None;
    }
    let shared_prefix =
        embedded_audio_partial_preview_common_prefix_chars(&current_key, &candidate_key);
    let bounded_long_revision = current_key_chars > SHORT_PREFIX_REPAIR_MAX_CURRENT_CHARS
        && embedded_audio_final_supplement_has_bounded_long_rewrite_alignment(
            &current_key,
            &candidate_key,
            shared_prefix,
            AUTHORITATIVE_LONG_REVISION_MIN_SHARED_PREFIX_CHARS,
            AUTHORITATIVE_LONG_REVISION_MAX_LENGTH_DELTA_CHARS,
            AUTHORITATIVE_LONG_REVISION_MAX_EDIT_CHARS,
        );
    if candidate_key_chars
        < current_key_chars.saturating_add(SHORT_PREFIX_REPAIR_MIN_EXTENSION_CHARS)
        && !bounded_long_revision
    {
        return None;
    }
    if current_key_chars > SHORT_PREFIX_REPAIR_MAX_CURRENT_CHARS {
        let stable_long_prefix = shared_prefix >= LONG_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS
            && candidate_key_chars
                >= current_key_chars.saturating_add(LONG_PREFIX_REPAIR_MIN_EXTENSION_CHARS);
        let bounded_prefix_insertion =
            embedded_audio_final_supplement_has_bounded_prefix_insertion_alignment(
                &current_key,
                &candidate_key,
                shared_prefix,
                LONG_SHIFTED_REPAIR_MIN_TOTAL_GROWTH_CHARS,
            );
        let bounded_early_rewrite =
            embedded_audio_final_supplement_has_bounded_early_rewrite_alignment(
                &current_key,
                &candidate_key,
                shared_prefix,
                AUTHORITATIVE_EARLY_REWRITE_MIN_SHARED_PREFIX_CHARS,
                AUTHORITATIVE_EARLY_REWRITE_MIN_EXTENSION_CHARS,
                AUTHORITATIVE_EARLY_REWRITE_MAX_EDIT_CHARS,
            );
        let authoritative_midstream_rewrite = shared_prefix
            >= AUTHORITATIVE_REWRITE_MIN_SHARED_PREFIX_CHARS
            && candidate_key_chars
                >= current_key_chars.saturating_add(AUTHORITATIVE_REWRITE_MIN_EXTENSION_CHARS);
        if bounded_early_rewrite {
            log::info!(
                "[coord] accepted bounded authoritative early preview rewrite current_chars={} candidate_chars={} shared_prefix_chars={}",
                current_key_chars,
                candidate_key_chars,
                shared_prefix
            );
            return Some(candidate.to_string());
        }
        if bounded_long_revision {
            log::info!(
                "[coord] accepted bounded authoritative long preview revision current_chars={} candidate_chars={} shared_prefix_chars={}",
                current_key_chars,
                candidate_key_chars,
                shared_prefix
            );
            return Some(candidate.to_string());
        }
        return (stable_long_prefix || bounded_prefix_insertion || authoritative_midstream_rewrite)
            .then(|| candidate.to_string());
    }
    if current_key_chars <= 4 || shared_prefix >= SHORT_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS {
        return Some(candidate.to_string());
    }
    if current_key_chars.saturating_sub(shared_prefix) > SHORT_PREFIX_REPAIR_MAX_REWRITE_CHARS {
        return None;
    }
    Some(candidate.to_string())
}

fn embedded_audio_final_supplement_adds_decorative_progress(
    current: &str,
    candidate: &str,
) -> bool {
    candidate
        .strip_prefix(current)
        .filter(|suffix| !suffix.is_empty())
        .is_some_and(|suffix| {
            suffix
                .chars()
                .all(is_embedded_audio_partial_preview_decorative)
        })
}

// Final-session two-pass corrections can replace a short early branch without
// adding characters. Both ends must still agree before the visible preview is
// allowed to change, so unrelated short phrases cannot overwrite it.
fn embedded_audio_final_supplement_is_brief_bounded_revision(
    current_key: &str,
    candidate_key: &str,
) -> bool {
    const MIN_CHARS: usize = 5;
    const MAX_CHARS: usize = 12;
    const MAX_LENGTH_DELTA_CHARS: usize = 2;
    const MIN_SHARED_PREFIX_CHARS: usize = 2;
    const MIN_SHARED_SUFFIX_CHARS: usize = 2;

    let current_chars = current_key.chars().count();
    let candidate_chars = candidate_key.chars().count();
    current_chars >= MIN_CHARS
        && candidate_chars >= MIN_CHARS
        && current_chars <= MAX_CHARS
        && candidate_chars <= MAX_CHARS
        && current_chars.abs_diff(candidate_chars) <= MAX_LENGTH_DELTA_CHARS
        && embedded_audio_partial_preview_common_prefix_chars(current_key, candidate_key)
            >= MIN_SHARED_PREFIX_CHARS
        && embedded_audio_partial_preview_common_suffix_chars(current_key, candidate_key)
            >= MIN_SHARED_SUFFIX_CHARS
}

fn embedded_audio_final_supplement_has_bounded_prefix_insertion_alignment(
    current_key: &str,
    candidate_key: &str,
    shared_prefix_chars: usize,
    min_total_growth_chars: usize,
) -> bool {
    const MIN_SHARED_PREFIX_CHARS: usize = 2;
    const MAX_INSERTED_PREFIX_CHARS: usize = 2;

    if shared_prefix_chars < MIN_SHARED_PREFIX_CHARS {
        return false;
    }

    let current: Vec<char> = current_key.chars().collect();
    let candidate: Vec<char> = candidate_key.chars().collect();
    if candidate.len() < current.len().saturating_add(min_total_growth_chars) {
        return false;
    }

    let mut current_index = shared_prefix_chars;
    let mut candidate_index = shared_prefix_chars;
    let mut inserted_chars = 0usize;
    while current_index < current.len() && candidate_index < candidate.len() {
        if current[current_index] == candidate[candidate_index] {
            current_index += 1;
            candidate_index += 1;
        } else if inserted_chars < MAX_INSERTED_PREFIX_CHARS {
            inserted_chars += 1;
            candidate_index += 1;
        } else {
            return false;
        }
    }

    current_index == current.len() && inserted_chars > 0
}

fn embedded_audio_final_supplement_has_bounded_early_rewrite_alignment(
    current_key: &str,
    candidate_key: &str,
    shared_prefix_chars: usize,
    min_shared_prefix_chars: usize,
    min_extension_chars: usize,
    max_edit_chars: usize,
) -> bool {
    if shared_prefix_chars < min_shared_prefix_chars {
        return false;
    }

    let current: Vec<char> = current_key.chars().collect();
    let candidate: Vec<char> = candidate_key.chars().collect();
    if candidate.len() < current.len().saturating_add(min_extension_chars) {
        return false;
    }

    let min_prefix_len = current.len().saturating_sub(max_edit_chars);
    let max_prefix_len = current
        .len()
        .saturating_add(max_edit_chars)
        .min(candidate.len());
    (min_prefix_len..=max_prefix_len).any(|candidate_prefix_len| {
        embedded_audio_partial_preview_edit_distance_at_most(
            &current,
            &candidate[..candidate_prefix_len],
            max_edit_chars,
        )
    })
}

fn embedded_audio_final_supplement_has_bounded_long_rewrite_alignment(
    current_key: &str,
    candidate_key: &str,
    shared_prefix_chars: usize,
    min_shared_prefix_chars: usize,
    max_length_delta_chars: usize,
    max_edit_chars: usize,
) -> bool {
    if shared_prefix_chars < min_shared_prefix_chars {
        return false;
    }

    let current: Vec<char> = current_key.chars().collect();
    let candidate: Vec<char> = candidate_key.chars().collect();
    current.len().abs_diff(candidate.len()) <= max_length_delta_chars
        && embedded_audio_partial_preview_edit_distance_at_most(
            &current,
            &candidate,
            max_edit_chars,
        )
}

fn embedded_audio_partial_preview_edit_distance_at_most(
    left: &[char],
    right: &[char],
    max_distance: usize,
) -> bool {
    if left.len().abs_diff(right.len()) > max_distance {
        return false;
    }

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (left_index, left_char) in left.iter().enumerate() {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(left_index + 1);
        for (right_index, right_char) in right.iter().enumerate() {
            let replace_cost = previous[right_index] + usize::from(left_char != right_char);
            let insert_cost = current[right_index] + 1;
            let delete_cost = previous[right_index + 1] + 1;
            current.push(replace_cost.min(insert_cost).min(delete_cost));
        }
        if current.iter().copied().min().unwrap_or_default() > max_distance {
            return false;
        }
        previous = current;
    }

    previous[right.len()] <= max_distance
}

fn embedded_audio_partial_preview_common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn embedded_audio_partial_preview_common_suffix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .rev()
        .zip(right.chars().rev())
        .take_while(|(left, right)| left == right)
        .count()
}

fn embedded_audio_partial_preview_stability_key(text: &str) -> String {
    let mut key = String::new();
    for ch in text.chars() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            key.push(lower);
        }
    }
    key
}

fn is_embedded_audio_partial_preview_decorative(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。'
                | '、'
                | '；'
                | '：'
                | '？'
                | '！'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '…'
                | '—'
        )
}

fn wake_phrase_character_matches(actual: char, expected: char) -> bool {
    if actual == expected {
        return true;
    }
    use pinyin::ToPinyin;
    actual
        .to_pinyin()
        .zip(expected.to_pinyin())
        .is_some_and(|(actual, expected)| actual.plain() == expected.plain())
}

fn strip_bounded_wake_phrase_suffix_fragment(text: &str, phrase: &[char]) -> Option<String> {
    if phrase.len() < 2 {
        return None;
    }

    for suffix_start in 1..phrase.len() {
        let mut text_chars = text.char_indices();
        let mut consumed_end = 0usize;
        let mut matched = true;
        for expected in &phrase[suffix_start..] {
            let Some((index, actual)) = text_chars.next() else {
                matched = false;
                break;
            };
            if is_embedded_audio_partial_preview_decorative(actual)
                || !wake_phrase_character_matches(actual, *expected)
            {
                matched = false;
                break;
            }
            consumed_end = index + actual.len_utf8();
        }
        if !matched
            || !text[consumed_end..]
                .chars()
                .next()
                .is_some_and(is_embedded_audio_partial_preview_decorative)
        {
            continue;
        }
        return Some(
            text[consumed_end..]
                .trim_start_matches(is_embedded_audio_partial_preview_decorative)
                .trim()
                .to_string(),
        );
    }
    None
}

fn strip_wake_phrase_prefix(text: &str, phrase: &str, suppress_partial: bool) -> String {
    let text = text.trim();
    let phrase = phrase
        .chars()
        .filter(|ch| !is_embedded_audio_partial_preview_decorative(*ch))
        .collect::<Vec<_>>();
    if text.is_empty() || phrase.is_empty() {
        return text.to_string();
    }

    let mut phrase_index = 0usize;
    let mut consumed_end = 0usize;
    for (index, ch) in text.char_indices() {
        let next = index + ch.len_utf8();
        if is_embedded_audio_partial_preview_decorative(ch) {
            consumed_end = next;
            continue;
        }
        if phrase_index == phrase.len() {
            break;
        }
        if !wake_phrase_character_matches(ch, phrase[phrase_index]) {
            return strip_bounded_wake_phrase_suffix_fragment(text, &phrase)
                .unwrap_or_else(|| text.to_string());
        }
        phrase_index += 1;
        consumed_end = next;
    }

    if phrase_index == phrase.len() {
        text[consumed_end..]
            .trim_start_matches(is_embedded_audio_partial_preview_decorative)
            .trim()
            .to_string()
    } else if suppress_partial && phrase_index > 0 {
        String::new()
    } else {
        text.to_string()
    }
}

fn filter_automatic_wake_phrase_text(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: &str,
    suppress_partial: bool,
) -> String {
    let phrase = inner
        .embedded_audio_wake_phrase_filter
        .lock()
        .as_ref()
        .filter(|(filter_session_id, _)| *filter_session_id == session_id)
        .map(|(_, phrase)| phrase.clone());
    phrase.map_or_else(
        || text.trim().to_string(),
        |phrase| strip_wake_phrase_prefix(text, &phrase, suppress_partial),
    )
}

fn is_dictation_filler_word(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    matches!(first, '嗯' | '呃' | '额' | '唔')
        && chars.all(|ch| matches!(ch, '嗯' | '呃' | '额' | '唔'))
}

fn collapsed_filler_gap(gap: &str, terminal: bool) -> String {
    let punctuation = gap
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<Vec<_>>();
    if punctuation.is_empty() {
        return (!terminal).then_some(" ").unwrap_or_default().to_string();
    }
    let chosen = punctuation
        .iter()
        .rev()
        .find(|ch| matches!(ch, '。' | '！' | '？' | '.' | '!' | '?'))
        .or_else(|| punctuation.last())
        .copied();
    chosen.map(|ch| ch.to_string()).unwrap_or_default()
}

fn remove_standalone_dictation_fillers(text: &str) -> String {
    let text = text.trim();
    let mut output = String::new();
    let mut word = String::new();
    let mut gap = String::new();
    let mut have_retained_word = false;
    let mut filler_removed_in_gap = false;

    let flush_word = |word: &mut String,
                      gap: &mut String,
                      output: &mut String,
                      have_retained_word: &mut bool,
                      filler_removed_in_gap: &mut bool| {
        if word.is_empty() {
            return;
        }
        if is_dictation_filler_word(word) {
            *filler_removed_in_gap = true;
            word.clear();
            return;
        }
        if *have_retained_word {
            if *filler_removed_in_gap {
                output.push_str(&collapsed_filler_gap(gap, false));
            } else {
                output.push_str(gap);
            }
        }
        output.push_str(word);
        *have_retained_word = true;
        *filler_removed_in_gap = false;
        gap.clear();
        word.clear();
    };

    for ch in text.chars() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            flush_word(
                &mut word,
                &mut gap,
                &mut output,
                &mut have_retained_word,
                &mut filler_removed_in_gap,
            );
            gap.push(ch);
        } else {
            word.push(ch);
        }
    }
    flush_word(
        &mut word,
        &mut gap,
        &mut output,
        &mut have_retained_word,
        &mut filler_removed_in_gap,
    );
    if have_retained_word {
        if filler_removed_in_gap {
            output.push_str(&collapsed_filler_gap(&gap, true));
        } else {
            output.push_str(&gap);
        }
    }
    output.trim().to_string()
}

fn filter_dictation_preview_text(inner: &Arc<Inner>, session_id: SessionId, text: &str) -> String {
    let text = filter_automatic_wake_phrase_text(inner, session_id, text, true);
    if inner.prefs.get().remove_filler_words {
        remove_standalone_dictation_fillers(&text)
    } else {
        text
    }
}

fn embedded_audio_partial_preview_repeats_recent_short_tail(
    current_key: &str,
    candidate_key: &str,
    current_key_chars: usize,
) -> bool {
    const MIN_CURRENT_CHARS: usize = 6;
    const MIN_SUFFIX_CHARS: usize = 2;
    const MAX_SUFFIX_CHARS: usize = 6;

    if current_key_chars < MIN_CURRENT_CHARS {
        return false;
    }

    let suffix_key: String = candidate_key.chars().skip(current_key_chars).collect();
    let suffix_chars = suffix_key.chars().count();
    if !(MIN_SUFFIX_CHARS..=MAX_SUFFIX_CHARS).contains(&suffix_chars) {
        return false;
    }
    if !suffix_key
        .chars()
        .all(is_embedded_audio_partial_preview_cjk)
    {
        return false;
    }

    current_key.ends_with(&suffix_key)
}

fn is_embedded_audio_partial_preview_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn stitch_embedded_audio_partial_preview(
    current: &str,
    candidate: &str,
    current_key_chars: usize,
) -> Option<String> {
    let suffix_start = byte_index_after_stability_chars(candidate, current_key_chars);
    let mut suffix = &candidate[suffix_start..];
    if suffix.is_empty() {
        return None;
    }
    if current
        .chars()
        .last()
        .is_some_and(is_embedded_audio_partial_preview_decorative)
    {
        suffix = suffix.trim_start_matches(is_embedded_audio_partial_preview_decorative);
    }
    if suffix.is_empty() {
        return None;
    }
    let mut stitched = current.to_string();
    stitched.push_str(suffix);
    Some(stitched)
}

fn byte_index_after_stability_chars(text: &str, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let mut seen = 0usize;
    for (idx, ch) in text.char_indices() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        seen = seen.saturating_add(1);
        if seen == count {
            return idx + ch.len_utf8();
        }
    }
    text.len()
}

fn emit_embedded_audio_partial_preview_if_active(
    inner: &Arc<Inner>,
    session_id: SessionId,
    preview: String,
) -> bool {
    apply_and_publish_dictation_event(
        inner,
        DictationEvent::AsrPartial {
            session_id,
            after_stop: embedded_audio_stop_feedback_latched(inner),
        },
        current_embedded_audio_capsule_level(inner),
        Some(preview),
        None,
    )
}

fn emit_embedded_audio_pcm_capsule_if_active(
    inner: &Arc<Inner>,
    session_id: SessionId,
    capsule_state: CapsuleState,
    level: f32,
) -> bool {
    let after_stop = match capsule_state {
        CapsuleState::Recording => false,
        CapsuleState::Transcribing => true,
        _ => return false,
    };
    let level = remember_embedded_audio_capsule_level(inner, level);
    if embedded_ble_actor_context_active(inner) {
        let trace_timeline = should_trace_embedded_ble_pcm_capsule(inner, session_id, after_stop);
        apply_embedded_ble_session_actor_dictation_event_with_trace(
            inner,
            EmbeddedBleSessionActorCommand::BlePacket,
            session_id,
            format!("pcm_capsule after_stop={after_stop}"),
            trace_timeline,
            DictationEvent::BlePcm {
                session_id,
                after_stop,
            },
            level,
            None,
            None,
        )
    } else {
        apply_and_publish_dictation_event(
            inner,
            DictationEvent::BlePcm {
                session_id,
                after_stop,
            },
            level,
            None,
            None,
        )
    }
}

fn emit_embedded_audio_transcribing_if_active(
    inner: &Arc<Inner>,
    session_id: SessionId,
    message: Option<String>,
) -> bool {
    if embedded_ble_actor_context_active(inner) {
        apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::BlePacket,
            session_id,
            "transcribing feedback after stop boundary",
            DictationEvent::BleStop { session_id },
            0.0,
            message,
            None,
        )
    } else {
        apply_and_publish_dictation_event(
            inner,
            DictationEvent::BleStop { session_id },
            0.0,
            message,
            None,
        )
    }
}

fn embedded_audio_streaming_session_accepts_pcm(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let state = inner.state.lock();
    state.session_id == session_id && !state.cancelled && state.phase == SessionPhase::Listening
}

pub(super) fn request_embedded_audio_stop_feedback(
    inner: &Arc<Inner>,
    _reason: &'static str,
) -> bool {
    let session_id = {
        let state = inner.state.lock();
        if state.phase != SessionPhase::Listening {
            return false;
        }
        state.session_id
    };
    latch_embedded_audio_stop_feedback(inner);
    emit_embedded_audio_transcribing_if_active(
        inner,
        session_id,
        current_embedded_audio_partial_preview(inner),
    )
}

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
    archive_pcm: Option<Vec<u8>>,
    streamed_pcm_bytes: usize,
    normalized_pcm_bytes: usize,
    streaming_pcm_buffer: Vec<u8>,
    streaming_agc: EmbeddedStreamingAgcState,
    device_ai_processing_started: bool,
}

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
    let Ok(directory) = std::env::var(WAKE_DIAGNOSTIC_DIR_ENV) else {
        return;
    };
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
        crate::embedded_audio::SessionStartOrigin::VoiceActivation if enrolled => {
            Some(BufferedSpeakerCandidateKind::Verification)
        }
        crate::embedded_audio::SessionStartOrigin::VoiceActivation
        | crate::embedded_audio::SessionStartOrigin::Unknown(_) => {
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
const OWNER_VERIFICATION_START_MS: usize = 1_100;
const OWNER_VERIFICATION_START_BYTES: usize = OWNER_VERIFICATION_START_MS * 32;
const OWNER_VERIFICATION_SNAPSHOT_MS: [usize; 3] = [OWNER_VERIFICATION_START_MS, 1_800, 2_400];
const LOCAL_CONFIRMATION_START_MS: usize =
    denzic_voice_activation_v1_core::DEFAULT_LOCAL_CONFIRMATION_START_MS as usize;
const LOCAL_CONFIRMATION_START_BYTES: usize = LOCAL_CONFIRMATION_START_MS * 32;
const LOCAL_CONFIRMATION_SNAPSHOT_MS: [usize; 3] = [LOCAL_CONFIRMATION_START_MS, 2_400, 3_000];

fn owner_verification_window_ready(pcm_bytes: usize) -> bool {
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
    clear_embedded_audio_final_result(inner);
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
        apply_and_publish_dictation_event(
            inner,
            DictationEvent::BleStart {
                session_id: current_session_id,
            },
            0.0,
            None,
            None,
        );
        log::info!("[coord] session started (hotkey-injection dry-run)");
        return Ok(());
    }

    let active_asr = active_asr_provider_from_preferences(inner);
    sync_active_asr_provider_to_credentials_for_runtime(&active_asr);
    log_dictation_asr_engine_selection(current_session_id, &active_asr);

    if let Err(message) = ensure_asr_credentials(&active_asr) {
        log::warn!("[coord] ASR credential gate failed: {message}");
        publish_dictation_pipeline_error(inner, current_session_id, message.clone());
        restore_prepared_windows_ime_session(inner, current_session_id);
        return Err(message);
    }

    if let Err(message) = ensure_microphone_permission(inner) {
        log::warn!("[coord] microphone permission gate failed: {message}");
        publish_dictation_pipeline_error(inner, current_session_id, message.clone());
        restore_prepared_windows_ime_session(inner, current_session_id);
        schedule_actionable_error_capsule_idle(inner, current_session_id);
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
                publish_dictation_pipeline_error(
                    inner,
                    current_session_id,
                    format!("本地模型初始化失败: {e}"),
                );
                restore_prepared_windows_ime_session(inner, current_session_id);
                schedule_actionable_error_capsule_idle(inner, current_session_id);
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
            publish_dictation_pipeline_error(
                inner,
                current_session_id,
                format!("ASR 连接失败: {e}"),
            );
            restore_prepared_windows_ime_session(inner, current_session_id);
            schedule_actionable_error_capsule_idle(inner, current_session_id);
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
            read_whisper_credentials(&active_asr).map_err(|e| e.to_string())?;
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
        let asr = build_volcengine_asr(inner, current_session_id);
        let bridge = Arc::new(DeferredAsrBridge::new());
        let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge.clone();
        store_asr_for_session(
            inner,
            current_session_id,
            ActiveAsr::Volcengine(Arc::clone(&asr)),
        );
        start_recorder_for_starting(inner, current_session_id, &active_asr, consumer).await?;

        if let Err(e) = open_volcengine_asr(&asr).await {
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
            publish_dictation_pipeline_error(
                inner,
                current_session_id,
                format!("ASR 连接失败: {e}"),
            );
            restore_prepared_windows_ime_session(inner, current_session_id);
            schedule_actionable_error_capsule_idle(inner, current_session_id);
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
        let final_target: Arc<dyn crate::asr::AudioConsumer> = asr.clone();
        let flushed_bytes = bridge.attach(final_target);
        asr.mark_audio_delivery_ready();
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
    let session_id_for_level = session_id;
    let asr_quality_warning = Arc::new(dictation_asr_quality_warning(active_asr));
    let asr_quality_warning_pending = Arc::new(AtomicBool::new(asr_quality_warning.is_some()));
    // 节流：电平回调本身约 185 Hz（cpal 默认音频块），全部转发到前端会让 CSS
    // transition 互相覆盖、视觉上"被平均"成静止。限制为 ~30 Hz（33ms 最少间隔），
    // 配合 CSS 短 transition 让每次 emit 完整可见。
    let last_emit_at = Arc::new(Mutex::new(None::<Instant>));
    const LEVEL_EMIT_MIN_INTERVAL_MS: u64 = 33;
    let level_handler: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |level| {
        let active = {
            let state = inner_for_level.state.lock();
            state.session_id == session_id_for_level
                && (state.phase == SessionPhase::Listening || state.phase == SessionPhase::Starting)
        };
        if !active {
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
        let message = if asr_quality_warning_pending.swap(false, Ordering::SeqCst) {
            asr_quality_warning.as_ref().clone()
        } else {
            None
        };
        publish_dictation_capsule(
            &inner_for_level,
            session_id_for_level,
            DictationUiState::Recording,
            level,
            message,
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
            publish_dictation_pipeline_error(inner, session_id, format!("录音启动失败: {e}"));
            restore_prepared_windows_ime_session(inner, session_id);
            release_recording_mute(inner, "dictation");
            schedule_actionable_error_capsule_idle(inner, session_id);
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

    apply_and_publish_dictation_event(
        inner,
        DictationEvent::RecordingAbort {
            session_id: abort.session_id,
        },
        0.0,
        Some(message),
        None,
    );
    schedule_actionable_error_capsule_idle(inner, abort.session_id);
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
    let transcript = take_latest_embedded_audio_final_result(inner);
    Ok(crate::embedded_audio::EmbeddedAudioSubmissionResult {
        stats,
        reconstructed_pcm_bytes,
        transcript,
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
    if embedded_ble_stats_only_enabled() {
        log::info!(
            "[embedded-ble] headless stats-only one-shot enabled by {EMBEDDED_BLE_STATS_ONLY_ENV}"
        );
        return submit_embedded_audio_ble_stats_only(timeout).await;
    }

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
    submit_embedded_audio_ble_stream_impl(
        inner,
        timeout_ms,
        true,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .await
}

pub(super) async fn submit_embedded_audio_ble_stream_background(
    inner: &Arc<Inner>,
    cancel_capture: Arc<AtomicBool>,
    leave_notify_cccd_enabled_on_cancel: Arc<AtomicBool>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    submit_embedded_audio_ble_stream_impl(
        inner,
        None,
        false,
        cancel_capture,
        Some(leave_notify_cccd_enabled_on_cancel),
    )
    .await
}

fn embedded_ble_stream_idle_timeout(
    timeout: Duration,
    emit_idle_capture_errors: bool,
) -> Option<Duration> {
    emit_idle_capture_errors.then_some(timeout)
}

enum EmbeddedBleStreamSignal {
    Ready,
    Notification(Vec<u8>),
}

fn embedded_ble_stats_only_enabled() -> bool {
    std::env::var(EMBEDDED_BLE_STATS_ONLY_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn configured_embedded_ble_control_signal_path(env_name: &str) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn wait_for_embedded_ble_control_signal(path: &str, timeout: Duration, label: &str) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if std::path::Path::new(path).exists() {
            if let Err(err) = fs::remove_file(path) {
                log::warn!("[embedded-ble] {label} signal remove failed ({path}): {err}");
            }
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn start_embedded_ble_control_signal_worker_if_configured() {
    let start_signal =
        configured_embedded_ble_control_signal_path(EMBEDDED_BLE_CONTROL_START_SIGNAL_ENV);
    let stop_signal =
        configured_embedded_ble_control_signal_path(EMBEDDED_BLE_CONTROL_STOP_SIGNAL_ENV);
    if start_signal.is_none() && stop_signal.is_none() {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("listener-type-embedded-ble-control-signals".into())
        .spawn(move || {
            log::info!(
                "[embedded-ble] headless control signal worker started start_signal={:?} stop_signal={:?}",
                start_signal,
                stop_signal
            );
            if let Some(path) = start_signal.as_deref() {
                if wait_for_embedded_ble_control_signal(path, Duration::from_secs(60), "start") {
                    match crate::embedded_ble::send_recording_control_toggle(
                        EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                    ) {
                        Ok(()) => log::info!(
                            "[embedded-ble] headless control start signal sent VREC:TOGGLE via Type"
                        ),
                        Err(err) => log::warn!(
                            "[embedded-ble] headless control start signal failed: {err}"
                        ),
                    }
                } else {
                    log::warn!(
                        "[embedded-ble] headless control start signal timed out waiting for {path}"
                    );
                    return;
                }
            }
            if let Some(path) = stop_signal.as_deref() {
                if wait_for_embedded_ble_control_signal(path, Duration::from_secs(300), "stop") {
                    match crate::embedded_ble::send_recording_control_stop(
                        EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                    ) {
                        Ok(()) => log::info!(
                            "[embedded-ble] headless control stop signal sent VREC:STOP via Type"
                        ),
                        Err(err) => log::warn!(
                            "[embedded-ble] headless control stop signal failed: {err}"
                        ),
                    }
                } else {
                    log::warn!(
                        "[embedded-ble] headless control stop signal timed out waiting for {path}"
                    );
                }
            }
        });
}

async fn submit_embedded_audio_ble_stats_only(
    timeout: Duration,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let notifications = tauri::async_runtime::spawn_blocking(move || {
        crate::embedded_ble::capture_notifications_once(timeout)
    })
    .await
    .map_err(|err| format!("嵌入式 BLE stats-only 抓音任务失败: {err}"))??;
    let collector =
        crate::embedded_audio::collect_notifications(notifications.iter().map(Vec::as_slice))
            .map_err(|err| format!("嵌入式 BLE stats-only 包解析失败: {err}"))?;
    let stats = collector.stats();
    if !stats.terminal_received {
        return Err("嵌入式 BLE stats-only 会话尚未收到结束包".to_string());
    }
    if stats.end_reason != Some(crate::embedded_audio::SessionEndReason::Stop) {
        return Err(format!(
            "嵌入式 BLE stats-only 会话未正常结束: {:?}",
            stats.end_reason
        ));
    }
    if stats.reconstructed_pcm_bytes == 0 {
        return Err("嵌入式 BLE stats-only 会话没有可识别的 PCM 数据".to_string());
    }
    log::info!(
        "[embedded-ble] stats-only capture done: pcm_bytes={} missing_packets={} received_packets={}",
        stats.reconstructed_pcm_bytes,
        stats.missing_packet_count,
        stats.received_packet_count
    );
    Ok(crate::embedded_audio::EmbeddedAudioSubmissionResult {
        reconstructed_pcm_bytes: stats.reconstructed_pcm_bytes,
        stats,
        transcript: None,
    })
}

async fn submit_embedded_audio_ble_stream_impl(
    inner: &Arc<Inner>,
    timeout_ms: Option<u64>,
    emit_idle_capture_errors: bool,
    cancel_capture: Arc<AtomicBool>,
    leave_notify_cccd_enabled_on_cancel: Option<Arc<AtomicBool>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(120_000).max(1_000));
    if emit_idle_capture_errors && embedded_ble_stats_only_enabled() {
        log::info!(
            "[embedded-ble] headless stats-only stream enabled by {EMBEDDED_BLE_STATS_ONLY_ENV}"
        );
        return submit_embedded_audio_ble_stats_only(timeout).await;
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EmbeddedBleStreamSignal>();
    register_embedded_ble_cancel_flag(inner, &cancel_capture);
    let cancel_capture_for_task = Arc::clone(&cancel_capture);
    let leave_notify_cccd_enabled_on_cancel_for_task =
        leave_notify_cccd_enabled_on_cancel.map(|handoff| Arc::clone(&handoff));
    let ready_inner = (!emit_idle_capture_errors).then(|| Arc::clone(inner));
    let ready_cancel = Arc::clone(&cancel_capture);
    let capture_task = tauri::async_runtime::spawn_blocking(move || {
        let ready_tx = tx.clone();
        let mut on_ready = || {
            if let Some(inner) = ready_inner.as_ref() {
                mark_embedded_ble_listener_ready(inner, &ready_cancel);
            } else {
                ready_tx
                    .send(EmbeddedBleStreamSignal::Ready)
                    .map_err(|_| "嵌入式音频流式处理已结束".to_string())?;
            }
            Ok(())
        };
        let capture_result = if emit_idle_capture_errors {
            crate::embedded_ble::capture_notification_events_until_cancelled(
                embedded_ble_stream_idle_timeout(timeout, emit_idle_capture_errors),
                cancel_capture_for_task,
                &mut on_ready,
                &mut |event| {
                    tx.send(EmbeddedBleStreamSignal::Notification(event.notification))
                        .map_err(|_| "嵌入式音频流式处理已结束".to_string())
                },
            )
        } else {
            crate::embedded_ble::capture_notification_events_continuous_with_connection_handoff_until_cancelled(
                embedded_ble_stream_idle_timeout(timeout, emit_idle_capture_errors),
                cancel_capture_for_task,
                leave_notify_cccd_enabled_on_cancel_for_task
                    .expect("background Listener capture always has a cancellation handoff flag"),
                &mut on_ready,
                &mut |event| {
                    tx.send(EmbeddedBleStreamSignal::Notification(event.notification))
                        .map_err(|_| "嵌入式音频流式处理已结束".to_string())
                },
            )
        };
        capture_result
    });

    let mut streaming = if emit_idle_capture_errors {
        EmbeddedStreamingDictation::default()
    } else {
        EmbeddedStreamingDictation::background_listener()
    };
    let mut ready_capsule_shown = false;
    let mut control_signal_worker_started = false;
    while let Some(signal) = rx.recv().await {
        let notification = match signal {
            EmbeddedBleStreamSignal::Ready => {
                if emit_idle_capture_errors && !control_signal_worker_started {
                    control_signal_worker_started = true;
                    start_embedded_ble_control_signal_worker_if_configured();
                }
                if emit_idle_capture_errors && !ready_capsule_shown {
                    ready_capsule_shown = true;
                    emit_capsule(
                        inner,
                        CapsuleState::Recording,
                        0.0,
                        0,
                        Some(EMBEDDED_BLE_READY_CAPSULE_MESSAGE.to_string()),
                        None,
                    );
                }
                continue;
            }
            EmbeddedBleStreamSignal::Notification(notification) => notification,
        };
        match streaming.handle_notification(inner, &notification).await {
            Ok(true) => {
                if emit_idle_capture_errors {
                    cancel_capture.store(true, Ordering::SeqCst);
                    break;
                }
                match streaming.submission_result() {
                    Ok(result) => {
                        log::info!(
                            "[embedded-ble] background session completed while keeping notify open pcm_bytes={} missing_packets={}",
                            result.reconstructed_pcm_bytes,
                            result.stats.missing_packet_count
                        );
                    }
                    Err(err) => {
                        cancel_capture.store(true, Ordering::SeqCst);
                        clear_embedded_ble_cancel_flag(inner, &cancel_capture);
                        return Err(err);
                    }
                }
                streaming.reset_for_next_session();
                record_embedded_ble_session_actor_command(
                    inner,
                    EmbeddedBleSessionActorCommand::ActorRestart,
                    None,
                    "background listener ready for next BLE session without reopening notify",
                );
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
    if let Err(err) = &capture_result {
        if !emit_idle_capture_errors
            && crate::embedded_ble::is_background_listener_deferred_for_ota_error(err)
        {
            return Err(err.clone());
        }
    }
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
    fn background_listener() -> Self {
        Self {
            keep_listening_after_pipeline_errors: true,
            ..Self::default()
        }
    }

    async fn handle_notification(
        &mut self,
        inner: &Arc<Inner>,
        notification: &[u8],
    ) -> Result<bool, String> {
        let event = self
            .collector
            .handle_notification(notification)
            .map_err(|err| format!("嵌入式音频流式包解析失败: {err}"))?;
        self.handle_ble_packet_actor_command(inner, event).await
    }

    async fn handle_ble_packet_actor_command(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
    ) -> Result<bool, String> {
        let event_detail = embedded_ble_session_event_detail(&event);
        let trace_timeline = embedded_ble_session_event_should_trace(&event);
        dispatch_embedded_ble_session_actor_command_with_trace(
            inner,
            EmbeddedBleSessionActorCommand::BlePacket,
            self.session.as_ref().map(|session| session.session_id),
            event_detail,
            trace_timeline,
            |_| (),
        );
        self.apply_ble_packet_actor_command(inner, event).await
    }

    async fn apply_ble_packet_actor_command(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
    ) -> Result<bool, String> {
        match event {
            crate::embedded_audio::StreamingSessionEvent::Started { session_id, origin } => {
                self.begin_candidate_or_session(inner, session_id, origin)
                    .await?;
                Ok(false)
            }
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => {
                let chunk_session_id = chunk.session_id;
                if let Some(candidate) = self.speaker_candidate.as_mut() {
                    if candidate.kind == BufferedSpeakerCandidateKind::Rejected {
                        return Ok(false);
                    }
                    if candidate.pcm.len().saturating_add(chunk.pcm.len())
                        > MAX_BUFFERED_SPEAKER_CANDIDATE_BYTES
                    {
                        return Err("声纹候选录音超过安全缓冲上限".to_string());
                    }
                    candidate.pcm.extend_from_slice(&chunk.pcm);
                    if self
                        .promote_hidden_candidate_if_requested(inner, chunk_session_id)
                        .await?
                    {
                        return Ok(false);
                    }
                    if self
                        .try_release_automatic_candidate(inner, chunk_session_id)
                        .await?
                    {
                        return Ok(false);
                    }
                    if let Some(expected_packet_count) = self.pending_stop_expected_packet_count {
                        if self.collector.inner().has_successful_complete_session() {
                            self.finish_completed_streaming_session(
                                inner,
                                chunk_session_id,
                                expected_packet_count,
                            )
                            .await?;
                            return Ok(true);
                        }
                    }
                    return Ok(false);
                }
                if embedded_streaming_chunk_is_asr_input(&chunk) {
                    self.begin_session_if_needed(inner, chunk.session_id)
                        .await?;
                    let session = self
                        .session
                        .as_mut()
                        .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
                    crate::observability::record_embedded_audio_first_packet(session.session_id);
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
                        self.finish_completed_streaming_session(
                            inner,
                            chunk_session_id,
                            expected_packet_count,
                        )
                        .await?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            crate::embedded_audio::StreamingSessionEvent::Stopped {
                session_id,
                expected_packet_count,
                ..
            } => {
                if let Some(session) = self.session.as_ref() {
                    crate::observability::record_embedded_audio_stop(session.session_id);
                }
                self.pending_stop_expected_packet_count = Some(expected_packet_count);
                self.show_transcribing_after_stop(inner);
                if self.collector.inner().has_successful_complete_session() {
                    self.finish_completed_streaming_session(
                        inner,
                        session_id,
                        expected_packet_count,
                    )
                    .await?;
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
                if self.session.is_none() {
                    if let Some(candidate) = self.speaker_candidate.take() {
                        clear_hidden_automatic_candidate();
                        if candidate.kind == BufferedSpeakerCandidateKind::Enrollment {
                            crate::speaker_verification::fail_enrollment("嵌入式音频会话已取消");
                            complete_voiceprint_enrollment_candidate(
                                "voiceprint_enrollment_cancelled",
                            );
                        } else {
                            reject_hidden_automatic_candidate("hidden_candidate_cancelled");
                        }
                        self.terminal_received = true;
                        return Ok(true);
                    }
                }
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
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        log::info!(
            "[coord] embedded audio streaming dictation started (embedded_session_id={embedded_session_id}, coordinator_session_id={}, asr={})",
            session.session_id,
            session.active_asr
        );
        self.session = Some(session);
        Ok(())
    }

    async fn begin_candidate_or_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        start_origin: crate::embedded_audio::SessionStartOrigin,
    ) -> Result<(), String> {
        if self.session.is_some() || self.speaker_candidate.is_some() {
            if self.embedded_session_id != Some(embedded_session_id) {
                return Err(format!(
                    "嵌入式音频流式 session 不一致: current={:?}, incoming={embedded_session_id}",
                    self.embedded_session_id
                ));
            }
            return Ok(());
        }
        self.embedded_session_id = Some(embedded_session_id);
        let mut kind = buffered_speaker_candidate_kind(
            start_origin,
            crate::speaker_verification::take_enrollment_arm(),
            crate::speaker_verification::is_enrolled(),
        );
        if let Some(mut candidate_kind) = kind.take() {
            let wake_detector = if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                let phrase = inner.prefs.get().voice_wake_phrase;
                match tauri::async_runtime::spawn_blocking(move || {
                    crate::wake_phrase::StreamingDetector::new(&phrase)
                })
                .await
                {
                    Ok(Ok(detector)) => Some(detector),
                    Ok(Err(err)) => {
                        log::warn!(
                            "[wake-phrase] hidden candidate rejected because streaming detector initialization failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        candidate_kind = BufferedSpeakerCandidateKind::Rejected;
                        None
                    }
                    Err(err) => {
                        log::warn!(
                            "[wake-phrase] hidden candidate rejected because streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        candidate_kind = BufferedSpeakerCandidateKind::Rejected;
                        None
                    }
                }
            } else {
                None
            };
            if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                mark_hidden_automatic_candidate_active();
            } else {
                clear_hidden_automatic_candidate();
            }
            log::info!(
                "[speaker-verification] buffering embedded candidate kind={candidate_kind:?} embedded_session_id={embedded_session_id}"
            );
            self.speaker_candidate = Some(BufferedSpeakerCandidate {
                kind: candidate_kind,
                pcm: Vec::new(),
                wake_detector,
                pending_phrase_match: None,
                #[cfg(target_os = "windows")]
                local_confirmation_task: None,
                local_confirmation_attempts: 0,
                local_confirmation_last_snapshot_bytes: 0,
                kws_fed_bytes: 0,
                kws_total_ms: 0,
                started_at: Instant::now(),
            });
            return Ok(());
        }
        self.begin_session_if_needed(inner, embedded_session_id)
            .await
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
        self.promote_hidden_candidate_if_requested(inner, embedded_session_id)
            .await?;
        if self.speaker_candidate.is_some()
            && self
                .finish_buffered_speaker_candidate(inner, embedded_session_id)
                .await?
        {
            return Ok(());
        }
        self.show_transcribing_after_stop(inner);

        let mut session = self
            .session
            .take()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        // STOP ends device delivery, so commit the final partial provider block before
        // finalizing ASR. This keeps the final syllables in the authoritative stream.
        session.flush_streaming_pcm();
        let archive_active = session
            .archive_pcm
            .as_deref()
            .map(|pcm| archive_embedded_audio_if_enabled(inner, session.session_id, pcm))
            .unwrap_or(false);
        inner
            .audio_archive_active
            .store(archive_active, std::sync::atomic::Ordering::Relaxed);
        if session.active_asr == "volcengine" {
            log::info!(
                "[coord] embedded audio streaming AGC summary (mode=voice_gated_fixed_session_gain, first_voiced_pcm_ms={:?}, voiced_chunks={}, quiet_chunks={}, observed_signal_rms_min={:?}, observed_signal_rms_max={:.2}, observed_signal_peak_max={}, pre_calibration_quiet_chunks={}, pre_calibration_signal_rms_max={:.2}, pre_calibration_signal_peak_max={}, first_eligible_signal_rms={:?}, first_eligible_signal_peak={:?}, first_gain={:?}, final_gain={:.2}, max_gain={:.2}, gain_updates={}, clipped_samples={})",
                session.streaming_agc.first_voiced_pcm_ms,
                session.streaming_agc.voiced_chunks,
                session.streaming_agc.quiet_chunks,
                session.streaming_agc.observed_signal_rms_min,
                session.streaming_agc.observed_signal_rms_max,
                session.streaming_agc.observed_signal_peak_max,
                session.streaming_agc.pre_calibration_quiet_chunks,
                session.streaming_agc.pre_calibration_signal_rms_max,
                session.streaming_agc.pre_calibration_signal_peak_max,
                session.streaming_agc.first_eligible_signal_rms,
                session.streaming_agc.first_eligible_signal_peak,
                session.streaming_agc.first_gain,
                session.streaming_agc.gain,
                session.streaming_agc.max_gain,
                session.streaming_agc.gain_update_count,
                session.streaming_agc.clipped_samples
            );
        }
        log::info!(
            "[coord] embedded audio streaming submitted to dictation pipeline (asr={}, input_mode={}, pcm_bytes={}, asr_pcm_bytes={}, archive={})",
            session.active_asr,
            if session.active_asr == "volcengine" {
                "voice_gated_fixed_session_gain"
            } else {
                "normalized_pcm"
            },
            session.streamed_pcm_bytes,
            session.normalized_pcm_bytes,
            archive_active
        );
        let coordinator_session_id = session.session_id;
        let user_initiated_stop =
            embedded_audio_stop_is_user_initiated(self.collector.inner().stats().stop_origin);
        let end_result = end_embedded_ble_session(
            inner,
            user_initiated_stop,
            format!(
                "embedded_session_id={embedded_session_id} coordinator_session_id={} expected_packets={expected_packet_count}",
                coordinator_session_id
            ),
        )
        .await;
        if end_result.is_ok() {
            self.transcript = take_embedded_audio_final_result(inner, coordinator_session_id);
        }
        end_result
    }

    async fn finish_buffered_speaker_candidate(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        let Some(mut candidate) = self.speaker_candidate.take() else {
            return Ok(false);
        };
        clear_hidden_automatic_candidate();
        if candidate.kind == BufferedSpeakerCandidateKind::Rejected {
            reject_hidden_automatic_candidate("automatic_candidate_rejected");
            return Ok(true);
        }
        if candidate.kind == BufferedSpeakerCandidateKind::Enrollment {
            let pcm = candidate.pcm;
            let phrase = inner.prefs.get().voice_wake_phrase;
            let result = tauri::async_runtime::spawn_blocking(move || {
                crate::wake_phrase::calibrate(&pcm, &phrase)?;
                crate::speaker_verification::finish_enrollment(&pcm)
            })
            .await
            .map_err(|err| format!("声纹登记处理任务失败: {err}"))
            .and_then(|result| result);
            let status = match result {
                Ok(status) => status,
                Err(err) => {
                    crate::speaker_verification::fail_enrollment(&err);
                    log::warn!(
                        "[speaker-verification] enrollment failed embedded_session_id={embedded_session_id}: {err}"
                    );
                    complete_voiceprint_enrollment_candidate("voiceprint_enrollment_failed");
                    return Ok(true);
                }
            };
            log::info!(
                "[speaker-verification] enrollment complete embedded_session_id={} enrolled={} score={:?}",
                embedded_session_id,
                status.enrolled,
                status.score
            );
            #[cfg(target_os = "windows")]
            {
                tauri::async_runtime::spawn_blocking(move || {
                    match crate::asr::local::wake_helper::preload() {
                        Ok(()) => log::info!(
                            "[wake-phrase] isolated local confirmation helper prepared after enrollment"
                        ),
                        Err(err) => log::info!(
                            "[wake-phrase] isolated local confirmation helper remains unavailable after enrollment: {err}"
                        ),
                    }
                });
            }
            complete_voiceprint_enrollment_candidate("voiceprint_enrollment_complete");
            return Ok(true);
        }

        let automatic = candidate.kind == BufferedSpeakerCandidateKind::Verification;
        let mut automatic_wake_phrase = None;
        if automatic {
            let phrase = inner.prefs.get().voice_wake_phrase;
            let Some(mut detector) = candidate.wake_detector.take() else {
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "detector-unavailable",
                    &candidate.pcm,
                );
                reject_hidden_automatic_candidate("wake_phrase_detection_failed");
                return Ok(true);
            };
            let remaining_pcm = candidate.pcm[candidate.kws_fed_bytes..].to_vec();
            candidate.kws_fed_bytes = candidate.pcm.len();
            let wake_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result = detector
                    .accept_pcm(&remaining_pcm)
                    .and_then(|found| match found {
                        Some(found) => Ok(Some(found)),
                        None => detector.finish(),
                    });
                (detector, result, started.elapsed().as_millis() as u64)
            })
            .await;
            let (_, wake_match, final_kws_ms) = match wake_task {
                Ok(result) => result,
                Err(err) => {
                    log::warn!(
                        "[wake-phrase] terminal streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                    );
                    reject_hidden_automatic_candidate("wake_phrase_detection_failed");
                    return Ok(true);
                }
            };
            candidate.kws_total_ms = candidate.kws_total_ms.saturating_add(final_kws_ms);
            let mut phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
            let mut local_confirmation_ms = 0u64;
            let wake_match = match wake_match {
                Ok(Some(found)) => Some(found),
                Ok(None) => {
                    #[cfg(target_os = "windows")]
                    {
                        let mut task = candidate.local_confirmation_task.take();
                        let mut matched = None;
                        loop {
                            if task.is_none()
                                && candidate.local_confirmation_attempts
                                    < LOCAL_CONFIRMATION_SNAPSHOT_MS.len()
                                && candidate.pcm.len() >= LOCAL_CONFIRMATION_START_BYTES
                                && candidate.pcm.len()
                                    > candidate.local_confirmation_last_snapshot_bytes
                            {
                                candidate.local_confirmation_attempts += 1;
                                candidate.local_confirmation_last_snapshot_bytes =
                                    candidate.pcm.len();
                                task = Some(spawn_local_wake_confirmation(
                                    inner,
                                    candidate.pcm.clone(),
                                    phrase.clone(),
                                ));
                                log::info!(
                                    "[wake-phrase] terminal local confirmation started embedded_session_id={} attempt={} snapshot_pcm_ms={}",
                                    embedded_session_id,
                                    candidate.local_confirmation_attempts,
                                    candidate.pcm.len() / 32
                                );
                            }
                            let Some(current_task) = task.take() else {
                                break;
                            };
                            match current_task.await {
                                Ok(Ok(result)) => {
                                    local_confirmation_ms =
                                        local_confirmation_ms.saturating_add(result.inference_ms);
                                    log::info!(
                                        "[wake-phrase] terminal local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} inference_ms={}",
                                        embedded_session_id,
                                        result.matched,
                                        result.phrase_relation,
                                        result.snapshot_pcm_ms,
                                        result.transcript_chars,
                                        result.inference_ms
                                    );
                                    if result.matched {
                                        phrase_signal =
                                            denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                        matched =
                                            Some(crate::wake_phrase::Match { end_seconds: 0.0 });
                                        break;
                                    }
                                }
                                Ok(Err(err)) => {
                                    log::warn!(
                                        "[wake-phrase] terminal local confirmation unavailable embedded_session_id={embedded_session_id}: {err}"
                                    );
                                }
                                Err(err) => {
                                    log::warn!(
                                        "[wake-phrase] terminal local confirmation task failed embedded_session_id={embedded_session_id}: {err}"
                                    );
                                }
                            }
                        }
                        matched
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        None
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[wake-phrase] automatic candidate rejected because detection failed embedded_session_id={embedded_session_id}: {err}"
                    );
                    save_bounded_wake_diagnostic(
                        embedded_session_id,
                        "detector-failed",
                        &candidate.pcm,
                    );
                    reject_hidden_automatic_candidate("wake_phrase_detection_failed");
                    return Ok(true);
                }
            };
            let voiceprint_pcm = candidate.pcm.clone();
            let verification_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result = crate::speaker_verification::verify(&voiceprint_pcm);
                (result, started.elapsed().as_millis() as u64)
            })
            .await;
            let (verification, voiceprint_ms) = match verification_task {
                Ok(result) => result,
                Err(err) => (Err(format!("声纹验证任务失败: {err}")), 0),
            };
            let total_ms = candidate
                .kws_total_ms
                .saturating_add(local_confirmation_ms)
                .saturating_add(voiceprint_ms);
            let gate_decision = denzic_voice_activation_v1_core::decide_gate(
                denzic_voice_activation_v1_core::GateInput {
                    phrase_signal: wake_match
                        .as_ref()
                        .map(|_| phrase_signal)
                        .unwrap_or(denzic_voice_activation_v1_core::PhraseSignal::None),
                    owner_match: verification.as_ref().ok().map(|result| result.matched),
                    terminal: true,
                },
            );
            log::info!(
                "[wake-phrase] automatic streaming gate embedded_session_id={} terminal=true pcm_ms={} kws_fed_bytes={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} total_compute_ms={} phrase_signal={:?} gate_decision={:?} owner_matched={}",
                embedded_session_id,
                candidate.pcm.len() / 32,
                candidate.kws_fed_bytes,
                candidate.kws_total_ms,
                local_confirmation_ms,
                voiceprint_ms,
                total_ms,
                wake_match
                    .as_ref()
                    .map(|_| phrase_signal)
                    .unwrap_or(denzic_voice_activation_v1_core::PhraseSignal::None),
                gate_decision,
                verification.as_ref().is_ok_and(|result| result.matched)
            );
            let Some(wake_match) = wake_match else {
                log::info!(
                    "[wake-phrase] automatic candidate rejected embedded_session_id={} phrase={}",
                    embedded_session_id,
                    phrase
                );
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "phrase-non-match",
                    &candidate.pcm,
                );
                reject_hidden_automatic_candidate("wake_phrase_non_match");
                return Ok(true);
            };
            if gate_decision != denzic_voice_activation_v1_core::GateDecision::Accept {
                let reason = match verification {
                    Ok(result) => {
                        log::info!(
                            "[speaker-verification] automatic candidate rejected embedded_session_id={} score={:.4}",
                            embedded_session_id,
                            result.score
                        );
                        "voiceprint_non_match"
                    }
                    Err(err) => {
                        log::warn!(
                            "[speaker-verification] automatic candidate rejected because verification failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        "voiceprint_verification_failed"
                    }
                };
                save_bounded_wake_diagnostic(embedded_session_id, reason, &candidate.pcm);
                reject_hidden_automatic_candidate(reason);
                return Ok(true);
            }
            let result = match verification {
                Ok(result) => result,
                Err(_) => {
                    unreachable!("fail-closed speaker verification rejected the error above")
                }
            };
            log::info!(
                "[speaker-verification] automatic candidate decision embedded_session_id={} matched={} score={:.4} wake_phrase={} wake_end_s={:.3}",
                embedded_session_id,
                result.matched,
                result.score,
                phrase,
                wake_match.end_seconds
            );
            save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
            if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                persist_verified_wake_phrase_calibration(phrase.clone()).await;
            }
            let post_wake_offset =
                if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                    ((wake_match.end_seconds + 0.12) * 32_000.0) as usize
                } else {
                    0
                };
            let post_wake_offset = post_wake_offset.min(candidate.pcm.len()) & !1usize;
            candidate.pcm.drain(..post_wake_offset);
            if candidate.pcm.is_empty() {
                reject_hidden_automatic_candidate("wake_phrase_without_dictation");
                return Ok(true);
            }
            automatic_wake_phrase = Some(phrase);
        } else {
            log::info!(
                "[speaker-verification] physical recording bypass embedded_session_id={embedded_session_id}"
            );
        }

        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        if let Some(phrase) = automatic_wake_phrase {
            set_embedded_audio_wake_phrase_filter(inner, session.session_id, phrase);
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        for chunk in candidate.pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            session.consume_streaming_pcm(inner, chunk)?;
        }
        log::info!(
            "[speaker-verification] released buffered candidate to ASR embedded_session_id={} pcm_bytes={}",
            embedded_session_id,
            candidate.pcm.len()
        );
        Ok(false)
    }

    async fn promote_hidden_candidate_if_requested(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        if !take_hidden_automatic_candidate_promotion() {
            return Ok(false);
        }
        let Some(mut candidate) = self.speaker_candidate.take() else {
            return Ok(false);
        };
        if candidate.kind != BufferedSpeakerCandidateKind::Verification {
            self.speaker_candidate = Some(candidate);
            return Ok(false);
        }
        if self.embedded_session_id != Some(embedded_session_id) {
            return Err(format!(
                "物理录音接管 session 不一致: current={:?}, incoming={embedded_session_id}",
                self.embedded_session_id
            ));
        }

        let discarded_pcm_bytes = discard_pre_press_candidate_pcm(&mut candidate.pcm);
        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("物理录音接管会话已被取消".to_string());
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "物理录音接管 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        log::info!(
            "[speaker-verification] physical recording promoted hidden candidate at post-press boundary embedded_session_id={} discarded_pre_press_pcm_bytes={}",
            embedded_session_id,
            discarded_pcm_bytes
        );
        Ok(true)
    }

    async fn try_release_automatic_candidate(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        let pending_phrase_match = {
            let Some(candidate) = self.speaker_candidate.as_mut() else {
                return Ok(false);
            };
            if candidate.kind != BufferedSpeakerCandidateKind::Verification {
                return Ok(false);
            }
            match candidate.pending_phrase_match.take() {
                Some(pending) if candidate.pcm.len() < pending.owner_verification_start_ms * 32 => {
                    candidate.pending_phrase_match = Some(pending);
                    return Ok(false);
                }
                pending => pending,
            }
        };
        let phrase = inner.prefs.get().voice_wake_phrase;
        let (wake_match, phrase_signal, local_confirmation_ms, kws_step_ms) = if let Some(pending) =
            pending_phrase_match
        {
            log::info!(
                    "[wake-phrase] pending phrase hit reached real owner window embedded_session_id={} pcm_ms={} owner_window_ms={}",
                    embedded_session_id,
                    self.speaker_candidate
                        .as_ref()
                        .map(|candidate| candidate.pcm.len() / 32)
                        .unwrap_or_default(),
                    OWNER_VERIFICATION_START_MS
                );
            (
                pending.wake_match,
                pending.phrase_signal,
                pending.local_confirmation_ms,
                0,
            )
        } else {
            let (mut detector, new_pcm) = {
                let candidate = self
                    .speaker_candidate
                    .as_mut()
                    .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                if candidate.pcm.len().saturating_sub(candidate.kws_fed_bytes)
                    < STREAMING_KWS_FEED_BATCH_BYTES
                {
                    return Ok(false);
                }
                let Some(detector) = candidate.wake_detector.take() else {
                    candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                    candidate.pcm.clear();
                    clear_hidden_automatic_candidate();
                    log::warn!(
                            "[wake-phrase] hidden candidate rejected because streaming detector is unavailable embedded_session_id={embedded_session_id}"
                        );
                    return Ok(false);
                };
                let new_pcm = candidate.pcm[candidate.kws_fed_bytes..].to_vec();
                candidate.kws_fed_bytes = candidate.pcm.len();
                (detector, new_pcm)
            };
            let wake_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result = detector.accept_pcm(&new_pcm);
                (detector, result, started.elapsed().as_millis() as u64)
            })
            .await;
            let (detector, wake_match, kws_step_ms) = match wake_task {
                Ok(result) => result,
                Err(err) => {
                    if let Some(candidate) = self.speaker_candidate.as_mut() {
                        candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                        candidate.pcm.clear();
                    }
                    clear_hidden_automatic_candidate();
                    log::warn!(
                            "[wake-phrase] streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                        );
                    return Ok(false);
                }
            };
            let wake_match = {
                let candidate = self
                    .speaker_candidate
                    .as_mut()
                    .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                candidate.wake_detector = Some(detector);
                candidate.kws_total_ms = candidate.kws_total_ms.saturating_add(kws_step_ms);
                match wake_match {
                    Ok(found) => found,
                    Err(err) => {
                        candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                        candidate.pcm.clear();
                        clear_hidden_automatic_candidate();
                        log::warn!(
                                "[wake-phrase] streaming detector failed embedded_session_id={embedded_session_id}: {err}"
                            );
                        return Ok(false);
                    }
                }
            };
            let mut phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
            let mut local_confirmation_ms = 0u64;
            let wake_match = if wake_match.is_some() {
                wake_match
            } else {
                #[cfg(target_os = "windows")]
                {
                    let completed_task = {
                        let candidate = self
                            .speaker_candidate
                            .as_mut()
                            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                        if candidate.local_confirmation_task.is_none() {
                            if let Some(snapshot_bytes) = next_local_confirmation_snapshot_bytes(
                                candidate.local_confirmation_attempts,
                            )
                            .filter(|snapshot_bytes| candidate.pcm.len() >= *snapshot_bytes)
                            {
                                candidate.local_confirmation_attempts += 1;
                                candidate.local_confirmation_last_snapshot_bytes =
                                    candidate.pcm.len();
                                candidate.local_confirmation_task =
                                    Some(spawn_local_wake_confirmation(
                                        inner,
                                        candidate.pcm.clone(),
                                        phrase.clone(),
                                    ));
                                log::info!(
                                        "[wake-phrase] bounded local confirmation started embedded_session_id={} attempt={} threshold_pcm_ms={} snapshot_pcm_ms={}",
                                        embedded_session_id,
                                        candidate.local_confirmation_attempts,
                                        snapshot_bytes / 32,
                                        candidate.pcm.len() / 32
                                    );
                            }
                        }
                        if candidate
                            .local_confirmation_task
                            .as_ref()
                            .is_some_and(|task| task.inner().is_finished())
                        {
                            candidate.local_confirmation_task.take()
                        } else {
                            None
                        }
                    };
                    if let Some(task) = completed_task {
                        match task.await {
                            Ok(Ok(result)) => {
                                local_confirmation_ms = result.inference_ms;
                                log::info!(
                                        "[wake-phrase] bounded local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} inference_ms={}",
                                        embedded_session_id,
                                        result.matched,
                                        result.phrase_relation,
                                        result.snapshot_pcm_ms,
                                        result.transcript_chars,
                                        result.inference_ms
                                    );
                                if result.matched {
                                    phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                    Some(crate::wake_phrase::Match { end_seconds: 0.0 })
                                } else {
                                    None
                                }
                            }
                            Ok(Err(err)) => {
                                log::warn!(
                                        "[wake-phrase] bounded local confirmation unavailable embedded_session_id={embedded_session_id}: {err}"
                                    );
                                None
                            }
                            Err(err) => {
                                log::warn!(
                                        "[wake-phrase] bounded local confirmation task failed embedded_session_id={embedded_session_id}: {err}"
                                    );
                                None
                            }
                        }
                    } else {
                        None
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    None
                }
            };
            let Some(wake_match) = wake_match else {
                return Ok(false);
            };
            if self
                .speaker_candidate
                .as_ref()
                .is_some_and(|candidate| !owner_verification_window_ready(candidate.pcm.len()))
            {
                let candidate = self
                    .speaker_candidate
                    .as_mut()
                    .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                log::info!(
                        "[wake-phrase] phrase hit pending real owner window embedded_session_id={} pcm_ms={} owner_window_ms={} phrase_signal={:?}",
                        embedded_session_id,
                        candidate.pcm.len() / 32,
                        OWNER_VERIFICATION_START_MS,
                        phrase_signal
                    );
                candidate.pending_phrase_match = Some(PendingAutomaticPhraseMatch {
                    wake_match,
                    phrase_signal,
                    local_confirmation_ms,
                    owner_verification_start_ms: OWNER_VERIFICATION_START_MS,
                });
                return Ok(false);
            }
            (
                wake_match,
                phrase_signal,
                local_confirmation_ms,
                kws_step_ms,
            )
        };

        let candidate = self
            .speaker_candidate
            .as_mut()
            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
        let pcm = candidate.pcm.clone();
        let pcm_ms = pcm.len() / 32;
        let kws_ms = candidate.kws_total_ms;
        let verification_task = tauri::async_runtime::spawn_blocking(move || {
            let started = Instant::now();
            let result = crate::speaker_verification::verify(&pcm);
            (result, started.elapsed().as_millis() as u64)
        })
        .await;
        let (verification, voiceprint_ms) = match verification_task {
            Ok(result) => result,
            Err(err) => (Err(format!("声纹验证任务失败: {err}")), 0),
        };
        let total_ms = kws_ms
            .saturating_add(local_confirmation_ms)
            .saturating_add(voiceprint_ms);
        let gate_decision = denzic_voice_activation_v1_core::decide_gate(
            denzic_voice_activation_v1_core::GateInput {
                phrase_signal,
                owner_match: verification.as_ref().ok().map(|result| result.matched),
                terminal: false,
            },
        );
        log::info!(
            "[wake-phrase] automatic streaming gate embedded_session_id={} terminal=false pcm_ms={} kws_fed_bytes={} kws_step_ms={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} total_compute_ms={} phrase_signal={:?} gate_decision={:?} owner_matched={}",
            embedded_session_id,
            pcm_ms,
            candidate.kws_fed_bytes,
            kws_step_ms,
            kws_ms,
            local_confirmation_ms,
            voiceprint_ms,
            total_ms,
            phrase_signal,
            gate_decision,
            verification.as_ref().is_ok_and(|result| result.matched)
        );

        if gate_decision != denzic_voice_activation_v1_core::GateDecision::Accept {
            if let Ok(result) = &verification {
                if !result.matched {
                    if let Some(retry_ms) = next_owner_verification_retry_ms(pcm_ms) {
                        candidate.pending_phrase_match = Some(PendingAutomaticPhraseMatch {
                            wake_match,
                            phrase_signal,
                            local_confirmation_ms,
                            owner_verification_start_ms: retry_ms,
                        });
                        log::info!(
                            "[wake-phrase] phrase hit retained for owner retry embedded_session_id={} pcm_ms={} next_owner_window_ms={} score={}",
                            embedded_session_id,
                            pcm_ms,
                            retry_ms,
                            result.score
                        );
                        return Ok(false);
                    }
                }
            }
            save_bounded_wake_diagnostic(
                embedded_session_id,
                "voiceprint-non-match",
                &candidate.pcm,
            );
            if let Some(candidate) = self.speaker_candidate.as_mut() {
                candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                candidate.pcm.clear();
            }
            clear_hidden_automatic_candidate();
            log::info!(
                "[wake-phrase] phrase matched but owner verification rejected embedded_session_id={} phrase={} result={:?}",
                embedded_session_id,
                phrase,
                verification.as_ref().map(|result| result.score)
            );
            return Ok(false);
        }
        if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
            persist_verified_wake_phrase_calibration(phrase.clone()).await;
        }

        let recording_control_task = tauri::async_runtime::spawn_blocking(|| {
            let started = Instant::now();
            let result =
                crate::embedded_ble::send_recording_control_activate(Duration::from_secs(2));
            (result, started.elapsed().as_millis() as u64)
        });
        let mut candidate = self
            .speaker_candidate
            .take()
            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
        save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
        clear_hidden_automatic_candidate();
        let post_wake_offset =
            if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                (wake_match.end_seconds * 32_000.0) as usize
            } else {
                0
            };
        let post_wake_offset = post_wake_offset.min(candidate.pcm.len()) & !1usize;
        candidate.pcm.drain(..post_wake_offset);
        let capsule_request_ms = candidate.started_at.elapsed().as_millis() as u64;
        let latency_target_pass = capsule_request_ms <= 1_200;
        let latency_ceiling_pass = capsule_request_ms <= 1_500;
        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        set_embedded_audio_wake_phrase_filter(inner, session.session_id, phrase.clone());
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        for pcm in candidate.pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            session.consume_streaming_pcm(inner, pcm)?;
        }
        let recording_control_ms = match recording_control_task.await {
            Ok((Ok(()), elapsed_ms)) => elapsed_ms,
            Ok((Err(err), elapsed_ms)) => {
                log::warn!(
                    "[embedded-ble] accepted automatic recording LED activation failed after capsule release embedded_session_id={} elapsed_ms={}: {}",
                    embedded_session_id,
                    elapsed_ms,
                    err
                );
                elapsed_ms
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] accepted automatic recording LED activation task failed after capsule release embedded_session_id={embedded_session_id}: {err}"
                );
                0
            }
        };
        log::info!(
            "[wake-phrase] live automatic session activated and released embedded_session_id={} phrase={} phrase_signal={:?} wake_end_s={:.3} post_wake_pcm_bytes={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} gate_total_ms={} recording_control_ms={} wake_to_capsule_request_ms={} latency_target_ms=1200 latency_target_pass={} latency_ceiling_ms=1500 latency_ceiling_pass={}",
            embedded_session_id,
            phrase,
            phrase_signal,
            wake_match.end_seconds,
            candidate.pcm.len(),
            kws_ms,
            local_confirmation_ms,
            voiceprint_ms,
            total_ms,
            recording_control_ms,
            capsule_request_ms,
            latency_target_pass,
            latency_ceiling_pass
        );
        Ok(true)
    }

    async fn finish_completed_streaming_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        expected_packet_count: u16,
    ) -> Result<(), String> {
        match self
            .finish_streaming_session(inner, embedded_session_id, expected_packet_count)
            .await
        {
            Ok(()) => {
                self.terminal_received = true;
                Ok(())
            }
            Err(err) if self.keep_notify_ready_after_completed_pipeline_error(inner, &err) => {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn keep_notify_ready_after_completed_pipeline_error(
        &mut self,
        inner: &Arc<Inner>,
        err: &str,
    ) -> bool {
        if !self.keep_listening_after_pipeline_errors || self.session.is_some() {
            return false;
        }
        self.terminal_received = true;
        record_embedded_ble_session_actor_command(
            inner,
            EmbeddedBleSessionActorCommand::ActorRestart,
            None,
            format!("completed session pipeline error kept notify ready: {err}"),
        );
        log::warn!(
            "[embedded-ble] background session completed with dictation pipeline error; keeping notify open: {err}"
        );
        true
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
        self.finish_completed_streaming_session(inner, embedded_session_id, expected_packet_count)
            .await
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
        clear_hidden_automatic_candidate();
        set_device_ai_processing_async(inner, false, "embedded_stream_abort");
        if matches!(
            self.speaker_candidate
                .as_ref()
                .map(|candidate| candidate.kind),
            Some(BufferedSpeakerCandidateKind::Enrollment)
        ) {
            crate::speaker_verification::fail_enrollment(message);
        }
        let event_session_id = self.session.as_ref().map(|session| session.session_id);
        if let Some(session) = self.session.take() {
            crate::observability::record_embedded_audio_failure(session.session_id, message);
            cancel_asr_for_session(inner, session.session_id);
            restore_prepared_windows_ime_session(inner, session.session_id);
            publish_dictation_pipeline_error(inner, session.session_id, message.to_string());
        } else {
            let elapsed = inner.state.lock().started_at.elapsed().as_millis() as u64;
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                elapsed,
                Some(message.to_string()),
                None,
            );
        }
        schedule_capsule_idle(inner, CAPSULE_STREAM_ERROR_HIDE_DELAY_MS, event_session_id);
        self.terminal_received = true;
    }

    fn show_transcribing_after_stop(&self, inner: &Arc<Inner>) {
        if let Some(session) = self.session.as_ref() {
            let already_latched = embedded_audio_stop_feedback_latched(inner);
            latch_embedded_audio_stop_feedback(inner);
            if !already_latched {
                let _ = emit_embedded_audio_transcribing_if_active(
                    inner,
                    session.session_id,
                    current_embedded_audio_partial_preview(inner),
                );
            }
        }
    }

    fn submission_result(
        &self,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submission_result_from_stats(
            self.terminal_received,
            self.collector.inner().stats(),
            self.transcript.clone(),
        )
    }

    fn reset_for_next_session(&mut self) {
        clear_hidden_automatic_candidate();
        self.collector.reset();
        self.session = None;
        self.speaker_candidate = None;
        self.embedded_session_id = None;
        self.transcript = None;
        self.pending_stop_expected_packet_count = None;
        self.terminal_received = false;
    }

    fn into_submission_result(
        self,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submission_result_from_stats(
            self.terminal_received,
            self.collector.into_inner().stats(),
            self.transcript,
        )
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
            transcript: None,
        }
    }
}

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
    clear_embedded_audio_wake_phrase_filter(inner);
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
    let consumer =
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
        archive_pcm,
        streamed_pcm_bytes: 0,
        normalized_pcm_bytes: 0,
        streaming_pcm_buffer: Vec::new(),
        streaming_agc: EmbeddedStreamingAgcState::default(),
        device_ai_processing_started: false,
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

fn reject_hidden_automatic_candidate(reason: &'static str) {
    log::info!(
        "[speaker-verification] hidden automatic candidate rejected silently reason={reason}"
    );
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
            "event=pcm embedded_session_id={} packet_sequence={} pcm_bytes={} after_stop={}",
            chunk.session_id,
            chunk.packet_sequence,
            chunk.pcm.len(),
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
) -> Result<Arc<dyn crate::recorder::AudioConsumer>, String> {
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
        return Ok(consumer);
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

    let final_asr = build_volcengine_asr(inner, session_id);
    let bridge = Arc::new(DeferredAsrBridge::new());
    let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge.clone();
    store_asr_for_session(
        inner,
        session_id,
        ActiveAsr::Volcengine(Arc::clone(&final_asr)),
    );
    let inner_for_open = Arc::clone(inner);
    tauri::async_runtime::spawn(async move {
        match open_volcengine_asr(&final_asr).await {
            Ok(()) => {
                let still_current = {
                    let state = inner_for_open.state.lock();
                    state.session_id == session_id
                        && !state.cancelled
                        && state.phase != SessionPhase::Idle
                };
                if !still_current {
                    final_asr.cancel();
                    log::info!(
                        "[coord] embedded Volcengine ASR opened after stale session {session_id} - discarded"
                    );
                    return;
                }
                let target: Arc<dyn crate::asr::AudioConsumer> = final_asr.clone();
                let flushed_bytes = bridge.attach(target);
                final_asr.mark_audio_delivery_ready();
                log::info!(
                    "[coord] embedded Volcengine ASR connected; flushed {flushed_bytes} deferred audio bytes"
                );
            }
            Err(err) => {
                final_asr.cancel();
                let still_current = {
                    let state = inner_for_open.state.lock();
                    state.session_id == session_id
                        && !state.cancelled
                        && state.phase != SessionPhase::Idle
                };
                if still_current {
                    log::error!("[coord] embedded Volcengine ASR open failed: {err}");
                    finish_dictation_pipeline_error(
                        &inner_for_open,
                        session_id,
                        format!("ASR 连接失败: {err}"),
                    );
                }
            }
        }
    });
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

#[derive(Debug, Clone, Copy)]
struct EmbeddedPcmGainStats {
    rms_before: f64,
    peak_before: u16,
    gain: f64,
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
            clipped_samples: 0,
        }
    }
}

fn normalize_embedded_pcm_for_asr(pcm: &[u8]) -> (Vec<u8>, EmbeddedPcmGainStats) {
    let (rms_before, peak_before) = embedded_pcm_rms_and_peak(pcm);
    let stats = EmbeddedPcmGainStats {
        rms_before,
        peak_before,
        gain: 1.0,
        clipped_samples: 0,
    };

    normalize_embedded_pcm_for_asr_with_stats(pcm, stats)
}

fn normalize_embedded_pcm_for_asr_with_stats(
    pcm: &[u8],
    mut stats: EmbeddedPcmGainStats,
) -> (Vec<u8>, EmbeddedPcmGainStats) {
    if stats.rms_before <= 0.0 || stats.rms_before >= EMBEDDED_AUDIO_TARGET_RMS {
        return (pcm.to_vec(), stats);
    }

    let gain = (EMBEDDED_AUDIO_TARGET_RMS / stats.rms_before).min(EMBEDDED_AUDIO_MAX_GAIN);
    if gain < EMBEDDED_AUDIO_MIN_GAIN {
        return (pcm.to_vec(), stats);
    }

    let (normalized, clipped_samples) = apply_embedded_pcm_gain(pcm, gain);
    stats.gain = gain;
    stats.clipped_samples = clipped_samples;
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
    let (rms_before, peak_before) = embedded_pcm_rms_and_peak(pcm);
    let (signal_rms, signal_peak) = embedded_pcm_streaming_agc_signal_level(pcm);
    let mut stats = EmbeddedPcmGainStats {
        rms_before,
        peak_before,
        gain: 1.0,
        clipped_samples: 0,
    };

    let has_speech_energy = embedded_streaming_chunk_has_speech_energy(signal_rms, signal_peak);
    agc.observed_signal_rms_min = Some(
        agc.observed_signal_rms_min
            .map_or(signal_rms, |minimum| minimum.min(signal_rms)),
    );
    agc.observed_signal_rms_max = agc.observed_signal_rms_max.max(signal_rms);
    agc.observed_signal_peak_max = agc.observed_signal_peak_max.max(signal_peak);
    if !has_speech_energy {
        agc.quiet_chunks += 1;
        // Leading silence must not calibrate the session. Once calibration has
        // happened, however, weak phonemes and word endings need the same
        // stable gain as voiced blocks or the provider repeatedly sees gaps.
        if !agc.gain_calibrated {
            agc.pre_calibration_quiet_chunks += 1;
            agc.pre_calibration_signal_rms_max = agc.pre_calibration_signal_rms_max.max(signal_rms);
            agc.pre_calibration_signal_peak_max =
                agc.pre_calibration_signal_peak_max.max(signal_peak);
            return (pcm.to_vec(), stats);
        }
    } else {
        agc.voiced_chunks += 1;
        agc.first_eligible_signal_rms.get_or_insert(signal_rms);
        agc.first_eligible_signal_peak.get_or_insert(signal_peak);
    }
    let peak_limited_gain = if signal_peak == 0 {
        EMBEDDED_AUDIO_MAX_GAIN
    } else {
        (i16::MAX as f64 * EMBEDDED_AUDIO_STREAMING_AGC_PEAK_HEADROOM / signal_peak as f64)
            .min(EMBEDDED_AUDIO_MAX_GAIN)
    };
    let requested_gain = (EMBEDDED_AUDIO_TARGET_RMS / signal_rms)
        .min(peak_limited_gain)
        .clamp(1.0, EMBEDDED_AUDIO_STREAMING_INITIAL_MAX_GAIN);
    let previous_gain = agc.gain;
    let session_gain = if !agc.gain_calibrated {
        agc.gain_calibrated = true;
        requested_gain
    } else if has_speech_energy {
        // A later quieter phrase may otherwise fall below the provider's
        // streaming recognition floor. Raising is monotonic for this session;
        // a loud block is handled below without making later speech quieter.
        agc.gain.max(requested_gain)
    } else {
        // Silence does not calibrate the session or amplify background noise.
        agc.gain
    };

    if (session_gain - previous_gain).abs() > f64::EPSILON {
        agc.gain_update_count += 1;
    }
    agc.first_gain.get_or_insert(session_gain);
    agc.max_gain = agc.max_gain.max(session_gain);
    agc.gain = session_gain;

    // Limit only this over-peak block. Sparse PDM impulses may clip after the
    // gain is applied, but must not make the rest of the block inaudible.
    // Persisting a lower gain would make later normal speech too quiet and
    // reintroduce accumulating ASR lag.
    let block_gain = session_gain.min(peak_limited_gain);
    stats.gain = block_gain;
    if block_gain < EMBEDDED_AUDIO_MIN_GAIN {
        return (pcm.to_vec(), stats);
    }

    let (normalized, clipped_samples) = apply_embedded_pcm_gain(pcm, block_gain);
    stats.clipped_samples = clipped_samples;
    agc.clipped_samples += clipped_samples;
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
    let raw = match asr {
        ActiveAsr::Volcengine(asr) => {
            debug_assert!(uses_global_timeout);
            if let Err(e) = asr.send_last_frame().await {
                log::error!("[coord] send last frame failed: {e}");
                asr.cancel();
                finish_dictation_pipeline_error(
                    inner,
                    current_session_id,
                    format!("识别收尾失败: {e}"),
                );
                return Err(e.to_string());
            }
            // 添加全局超时保护：防止 await_final_result() 永远挂起
            let timeout_duration = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
            match tokio::time::timeout(timeout_duration, asr.await_final_result()).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::error!("[coord] await final failed: {e}");
                    finish_dictation_pipeline_error(
                        inner,
                        current_session_id,
                        format!("识别失败: {e}"),
                    );
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
                    finish_dictation_timeout(inner, current_session_id, "识别超时".to_string());
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
    raw.text = filter_automatic_wake_phrase_text(inner, current_session_id, &raw.text, false);
    if raw.text != unfiltered_text.trim() {
        log::info!(
            "[wake-phrase] removed automatic activation phrase from final transcript session_id={} before_chars={} after_chars={}",
            current_session_id,
            unfiltered_text.chars().count(),
            raw.text.chars().count()
        );
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
        publish_embedded_ble_asr_final(
            inner,
            current_session_id,
            true,
            Some("没有识别到语音".to_string()),
        );
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
    log::info!(
        "[coord] final completion actions session_id={} chars={} insertion_status={:?} target_confirmed={} target_restored={} user_stop={} clipboard={} post_key={}",
        current_session_id,
        polished.chars().count(),
        status,
        original_target_confirmed,
        focus_ready_for_paste,
        user_initiated_stop,
        clipboard_result,
        post_dictation_key_result
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
    let done_message = if status == InsertStatus::Inserted
        && !polish_error.is_some()
        && !tsf_required_insert_failed
        && !wayland_session
    {
        None
    } else if tsf_required_insert_failed {
        Some("TSF 未上屏，已禁止非 TSF 兜底".to_string())
    } else if wayland_session {
        wayland_done_message(status, polish_error.is_some())
    } else {
        default_done_message(status, polish_error.is_some())
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
    if embedded_ble_host_cancel_context_active(inner) {
        cancel_embedded_ble_session_through_actor(inner);
        return;
    }
    cancel_session_direct(inner);
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
mod tests {
    use super::{
        append_typed_prefix, begin_embedded_audio_dictation_session_id,
        cancel_embedded_ble_listener_capture, cancel_session, claim_post_dictation_key,
        clear_embedded_ble_cancel_flag, current_embedded_audio_partial_preview,
        default_done_message, device_ai_processing_completion_delay,
        device_ai_processing_io_allowed, device_processing_final_succeeded,
        dictation_asr_engine_backend_id, dictation_asr_quality_warning,
        dictation_asr_uses_core_accurate_engine, dictation_error_code,
        embedded_audio_stop_feedback_latched, embedded_audio_stop_is_user_initiated,
        embedded_ble_listener_capture_ready, embedded_ble_processing_sync_disabled,
        embedded_ble_session_actor_history, embedded_ble_session_event_should_trace,
        embedded_ble_stream_idle_timeout, embedded_pcm_rms_and_peak, embedded_pcm_visual_level,
        embedded_streaming_chunk_is_asr_input, emit_embedded_audio_transcribing_if_active,
        end_embedded_ble_session, finalize_polished_text, finish_dictation_pipeline_error,
        finish_dictation_timeout, install_embedded_ble_listener_cancel,
        mark_embedded_ble_listener_ready, normalize_embedded_pcm_for_asr,
        normalize_embedded_streaming_pcm_for_asr, provider_preview_change,
        publish_embedded_ble_asr_final, record_embedded_ble_session_actor_command,
        register_embedded_ble_cancel_flag, remove_standalone_dictation_fillers,
        request_embedded_audio_stop_feedback, request_embedded_ble_recording_stop_from_host,
        should_restore_clipboard_after_dictation, should_send_post_dictation_key,
        stabilize_embedded_audio_final_supplemental_preview,
        stabilize_embedded_audio_partial_preview, store_embedded_audio_stats,
        streaming_insert_eligible, strip_wake_phrase_prefix, update_embedded_audio_partial_preview,
        wayland_done_message, EmbeddedAudioDictationSession, EmbeddedBleSessionActorCommand,
        EmbeddedStreamingAgcState, EmbeddedStreamingDictation, DEVICE_AI_PROCESSING_MAX_VISIBLE_MS,
        DEVICE_AI_PROCESSING_MIN_VISIBLE_MS, EMBEDDED_AUDIO_FEED_CHUNK_BYTES,
        EMBEDDED_AUDIO_MAX_GAIN, EMBEDDED_AUDIO_STREAMING_SPEECH_RMS, EMBEDDED_AUDIO_TARGET_RMS,
        EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, LOCAL_CONFIRMATION_START_BYTES,
        LOCAL_CONFIRMATION_START_MS,
    };
    use crate::coordinator::Coordinator;
    use crate::coordinator_state::{new_session_id, SessionPhase};
    use crate::embedded_audio::{
        build_audio_data_notification, build_session_start_notification,
        build_session_stop_notification, StreamingPcmChunk, StreamingSessionEvent,
    };
    use crate::types::{
        ChineseScriptPreference, CorrectionRule, DictationInputSource, InsertStatus, PolishMode,
        PostDictationKey, UserPreferences,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn local_confirmation_waits_for_pre_roll_plus_speech_observation() {
        assert_eq!(LOCAL_CONFIRMATION_START_MS, 1_800);
        assert_eq!(LOCAL_CONFIRMATION_START_BYTES / 32, 1_800);
    }
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct CountingConsumer {
        bytes: AtomicUsize,
    }

    impl crate::recorder::AudioConsumer for CountingConsumer {
        fn consume_pcm_chunk(&self, pcm: &[u8]) {
            self.bytes.fetch_add(pcm.len(), Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct CapturingConsumer {
        chunks: Mutex<Vec<Vec<u8>>>,
    }

    impl crate::recorder::AudioConsumer for CapturingConsumer {
        fn consume_pcm_chunk(&self, pcm: &[u8]) {
            self.chunks.lock().expect("capture lock").push(pcm.to_vec());
        }
    }

    #[test]
    fn embedded_audio_partial_preview_ignores_punctuation_only_revision() {
        assert_eq!(
            stabilize_embedded_audio_partial_preview(
                Some("这个预览波动太大了。然后呢？对于用户"),
                "这个预览波动太大了，然后呢？对于用户"
            ),
            None
        );
    }

    #[test]
    fn provider_preview_change_keeps_authoritative_early_rewrite_visible() {
        assert_eq!(
            provider_preview_change(Some("明天下午4点"), "明天下午4:15提醒我"),
            Some("明天下午4:15提醒我".to_string())
        );
        assert_eq!(
            provider_preview_change(Some("明天下午4:15提醒我"), "明天下午4:15提醒我"),
            None
        );
        assert_eq!(strip_wake_phrase_prefix("开始。", "开始录音", true), "");
        assert_eq!(
            strip_wake_phrase_prefix("开始录音，今天自动唤醒测试正常。", "开始录音", true),
            "今天自动唤醒测试正常。"
        );
        assert_eq!(
            strip_wake_phrase_prefix("开始录音，今天自动唤醒测试正常。", "开始录音", false),
            "今天自动唤醒测试正常。"
        );
        assert_eq!(
            strip_wake_phrase_prefix("开使录因，今天自动唤醒测试正常。", "开始录音", false),
            "今天自动唤醒测试正常。"
        );
        assert_eq!(
            strip_wake_phrase_prefix("开始录像，今天测试。", "开始录音", false),
            "开始录像，今天测试。"
        );
        assert_eq!(
            strip_wake_phrase_prefix("音，你帮我看这个东西行不行。", "开始录音", true),
            "你帮我看这个东西行不行。"
        );
        assert_eq!(
            strip_wake_phrase_prefix("录因，你帮我看这个东西行不行。", "开始录音", false),
            "你帮我看这个东西行不行。"
        );
        assert_eq!(
            strip_wake_phrase_prefix("音频测试继续。", "开始录音", true),
            "音频测试继续。"
        );
        assert_eq!(
            strip_wake_phrase_prefix("今天要说开始录音这个词。", "开始录音", true),
            "今天要说开始录音这个词。"
        );
        assert_eq!(
            remove_standalone_dictation_fillers("嗯，呃，今天自动唤醒测试正常。"),
            "今天自动唤醒测试正常。"
        );
        assert_eq!(
            remove_standalone_dictation_fillers("今天，嗯，我要测试。"),
            "今天，我要测试。"
        );
        assert_eq!(
            remove_standalone_dictation_fillers("那个文件就是额外版本。"),
            "那个文件就是额外版本。"
        );
    }

    #[test]
    fn embedded_audio_partial_preview_extends_without_rewriting_visible_prefix() {
        assert_eq!(
            stabilize_embedded_audio_partial_preview(
                Some("这个预览波动太大了。然后呢？对于用户"),
                "这个预览波动太大了，然后呢？对于用户的观感"
            )
            .as_deref(),
            Some("这个预览波动太大了。然后呢？对于用户的观感")
        );
    }

    #[test]
    fn dictation_asr_quality_warning_marks_non_core_engines() {
        assert_eq!(dictation_asr_engine_backend_id("volcengine"), "volcengine");
        assert!(dictation_asr_uses_core_accurate_engine("volcengine"));
        assert_eq!(dictation_asr_quality_warning("volcengine"), None);

        assert_eq!(
            dictation_asr_engine_backend_id("whisper"),
            "whisper-compatible"
        );
        assert!(!dictation_asr_uses_core_accurate_engine("whisper"));
        assert_eq!(
            dictation_asr_quality_warning("whisper").as_deref(),
            Some("当前识别引擎为Whisper-compatible (whisper)，不是核心 Volcengine 准确引擎，识别可能不准。")
        );
    }

    #[test]
    fn embedded_audio_final_supplement_seeds_short_prefix_extends_or_repairs() {
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(None, "帮"),
            None
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(None, "帮我"),
            Some("帮我".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(None, "帮我录音"),
            Some("帮我录音".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(Some("帮我录音"), "帮我录音。"),
            Some("帮我录音。".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(Some("帮我录音"), "帮我录音。怎么"),
            Some("帮我录音。怎么".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some("帮我录音。怎么"),
                "帮我录音，怎么"
            ),
            None
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some("帮我录音，怎么退"),
                "帮我录音，怎么"
            ),
            None
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some("帮我录音，怎么退"),
                "帮我落音，怎么退"
            ),
            None
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some("主要是这个露"),
                "主要是这个录音的指标你需要固化"
            ),
            Some("主要是这个录音的指标你需要固化".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some("这个浏览被截断"),
                "这个预览被截断了，需要更准确"
            ),
            Some("这个预览被截断了，需要更准确".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some("灵敏"),
                "预览灵敏稳定才算通过"
            ),
            Some("预览灵敏稳定才算通过".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some("然后你看那个浏览器头好像还是有点奇怪"),
                "然后你看那个浏览系统好像还是有点奇怪"
            ),
            None
        );
    }

    #[test]
    fn embedded_audio_final_supplement_repairs_observed_early_cjk_rewrite_without_unrelated_takeover(
    ) {
        let current = "请把3下午2点客户沟通安排近日历资料你先";
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some(current),
                "请把周三下午2点的客户沟通安排进日历资料你先发给陈林确认"
            ),
            Some("请把周三下午2点的客户沟通安排进日历资料你先发给陈林确认".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some(current),
                "请把明天的采购清单交给财务，然后准备下周发布会材料"
            ),
            None
        );
    }

    #[test]
    fn embedded_audio_final_supplement_repairs_observed_long_same_length_tail_revision() {
        let current = "晚上回家以后提醒我把洗好的衣服晾起来。再给家里打个电话，问问周日午饭怎么安排，最后别忘了把门禁卡。傍徨外道口袋里";
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some(current),
                "晚上回家以后提醒我把洗好的衣服晾起来。再给家里打个电话，问问周日午饭怎么安排，最后别忘了把门禁卡。放回外套口袋里。"
            ),
            Some("晚上回家以后提醒我把洗好的衣服晾起来。再给家里打个电话，问问周日午饭怎么安排，最后别忘了把门禁卡。放回外套口袋里。".to_string())
        );
        assert_eq!(
            stabilize_embedded_audio_final_supplemental_preview(
                Some(current),
                "晚上回家以后提醒我把洗好的衣服晾起来。再给家里打个电话，问问周日午饭怎么安排，最后请把文件寄到另一个地址。"
            ),
            None
        );
    }

    #[test]
    fn embedded_audio_partial_preview_does_not_shrink_visible_text() {
        assert_eq!(
            stabilize_embedded_audio_partial_preview(
                Some("这个预览波动太大了。然后呢？对于用户的观感"),
                "这个预览波动太大了。然后呢？对于用户"
            ),
            None
        );
    }

    #[test]
    fn embedded_audio_partial_preview_ignores_repeated_short_tail_extension() {
        assert_eq!(
            stabilize_embedded_audio_partial_preview(
                Some("帮我录音，怎么退"),
                "帮我录音，怎么退？怎么退"
            ),
            None
        );
    }

    #[test]
    fn embedded_audio_partial_preview_keeps_new_short_continuation() {
        assert_eq!(
            stabilize_embedded_audio_partial_preview(
                Some("帮我录音，怎么退"),
                "帮我录音，怎么退？现在"
            )
            .as_deref(),
            Some("帮我录音，怎么退？现在")
        );
    }

    fn embedded_audio_test_session(
        session_id: crate::coordinator_state::SessionId,
        consumer: Arc<dyn crate::recorder::AudioConsumer>,
    ) -> EmbeddedAudioDictationSession {
        EmbeddedAudioDictationSession {
            session_id,
            active_asr: "openai".into(),
            consumer,
            archive_pcm: Some(Vec::new()),
            streamed_pcm_bytes: 0,
            normalized_pcm_bytes: 0,
            streaming_pcm_buffer: Vec::new(),
            streaming_agc: EmbeddedStreamingAgcState::default(),
            device_ai_processing_started: false,
        }
    }

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
    fn embedded_pcm_visual_level_tracks_raw_voice_energy_without_asr_gain() {
        let silence = pcm_from_samples(&[0, 0, 0, 0]);
        let quiet_voice = pcm_from_samples(&[50, -50, 50, -50]);
        let ordinary_voice = pcm_from_samples(&[300, -300, 300, -300]);
        let loud_voice = pcm_from_samples(&[1_000, -1_000, 1_000, -1_000]);

        let silence_level = embedded_pcm_visual_level(&silence);
        let quiet_level = embedded_pcm_visual_level(&quiet_voice);
        let ordinary_level = embedded_pcm_visual_level(&ordinary_voice);
        let loud_level = embedded_pcm_visual_level(&loud_voice);

        assert_eq!(silence_level, 0.0);
        assert!(quiet_level > 0.012, "quiet_level={quiet_level}");
        assert!(
            ordinary_level > quiet_level,
            "{ordinary_level} <= {quiet_level}"
        );
        assert!(
            loud_level > ordinary_level,
            "{loud_level} <= {ordinary_level}"
        );
        assert_eq!(loud_level, 1.0);
    }

    fn samples_for_ms(ms: usize, sample: i16) -> Vec<i16> {
        vec![sample; 16_000 * ms / 1_000]
    }

    fn seed_cancelled_processing_session(
        coordinator: &Coordinator,
    ) -> crate::coordinator_state::SessionId {
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Processing;
            state.cancelled = true;
            state.focus_target = Some(42);
        }
        store_embedded_audio_stats(
            &coordinator.inner,
            crate::embedded_audio::SessionCollector::default().stats(),
        );
        session_id
    }

    fn assert_cancelled_processing_session_cleaned(coordinator: &Coordinator) {
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Idle);
            assert!(state.cancelled);
            assert_eq!(state.focus_target, None);
        }
        assert!(coordinator.inner.embedded_audio_stats.lock().is_none());
    }

    #[test]
    fn finish_pipeline_error_after_processing_cancel_cleans_without_error_finish() {
        let coordinator = Coordinator::new();
        let session_id = seed_cancelled_processing_session(&coordinator);

        let finished_as_error =
            finish_dictation_pipeline_error(&coordinator.inner, session_id, "识别失败".to_string());

        assert!(!finished_as_error);
        assert_cancelled_processing_session_cleaned(&coordinator);
    }

    #[test]
    fn finish_timeout_after_processing_cancel_cleans_without_error_finish() {
        let coordinator = Coordinator::new();
        let session_id = seed_cancelled_processing_session(&coordinator);

        let finished_as_error =
            finish_dictation_timeout(&coordinator.inner, session_id, "识别超时".to_string());

        assert!(!finished_as_error);
        assert_cancelled_processing_session_cleaned(&coordinator);
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
    fn cancel_session_requests_embedded_ble_capture_cancel_even_when_idle() {
        let coordinator = Coordinator::new();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
        coordinator.inner.state.lock().phase = SessionPhase::Idle;

        cancel_session(&coordinator.inner);

        assert!(cancel_flag.load(Ordering::SeqCst));
        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history
            .iter()
            .any(|record| record.command == EmbeddedBleSessionActorCommand::CancelCommand));
    }

    #[test]
    fn idle_cancel_without_capture_flag_does_not_route_by_default_embedded_pref() {
        let coordinator = Coordinator::new();
        coordinator.inner.state.lock().phase = SessionPhase::Idle;

        cancel_session(&coordinator.inner);

        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history.is_empty());
    }

    #[test]
    fn capsule_cancel_routes_by_embedded_ble_preference_without_capture_flag() {
        let coordinator = Coordinator::new();
        {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        cancel_session(&coordinator.inner);

        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history
            .iter()
            .any(|record| record.command == EmbeddedBleSessionActorCommand::CancelCommand));
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
    fn background_listener_keeps_notify_ready_after_completed_pipeline_failure() {
        let coordinator = Coordinator::new();
        let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
        mark_embedded_ble_listener_ready(&coordinator.inner, &active);
        let mut streaming = EmbeddedStreamingDictation::background_listener();

        assert!(streaming.keep_notify_ready_after_completed_pipeline_error(
            &coordinator.inner,
            "ASR final failed after completed BLE audio session"
        ));

        assert!(streaming.terminal_received);
        assert!(!active.load(Ordering::SeqCst));
        assert!(embedded_ble_listener_capture_ready(&coordinator.inner));
        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history.iter().any(|record| {
            record.command == EmbeddedBleSessionActorCommand::ActorRestart
                && record.detail.contains("pipeline error")
        }));
    }

    #[test]
    fn session_actor_commands_apply_asr_cancel_timeout_and_empty_final_behavior() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        let cancel_flag = Arc::new(AtomicBool::new(false));
        register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);

        update_embedded_audio_partial_preview(&coordinator.inner, session_id, "partial".into());
        assert_eq!(
            current_embedded_audio_partial_preview(&coordinator.inner).as_deref(),
            Some("partial")
        );
        cancel_session(&coordinator.inner);
        assert!(cancel_flag.load(Ordering::SeqCst));
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Idle);
            assert!(state.cancelled);
        }

        let timeout_session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = timeout_session_id;
            state.phase = SessionPhase::Processing;
            state.cancelled = false;
        }
        store_embedded_audio_stats(
            &coordinator.inner,
            crate::embedded_audio::SessionCollector::default().stats(),
        );
        assert!(publish_embedded_ble_asr_final(
            &coordinator.inner,
            timeout_session_id,
            true,
            Some("没有识别到语音".to_string())
        ));
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Idle);
        }
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = timeout_session_id;
            state.phase = SessionPhase::Processing;
            state.cancelled = false;
        }
        finish_dictation_timeout(
            &coordinator.inner,
            timeout_session_id,
            "识别超时".to_string(),
        );
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Idle);
        }

        let history = embedded_ble_session_actor_history(&coordinator.inner);
        let commands: Vec<_> = history.iter().map(|record| record.command).collect();
        assert!(commands.contains(&EmbeddedBleSessionActorCommand::AsrPartial));
        assert!(commands.contains(&EmbeddedBleSessionActorCommand::CancelCommand));
        assert!(commands.contains(&EmbeddedBleSessionActorCommand::AsrFinal));
        assert!(commands.contains(&EmbeddedBleSessionActorCommand::Timeout));
        assert!(history.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    }

    #[tokio::test]
    async fn session_actor_ble_packet_command_feeds_pcm_through_single_handler() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        let consumer = Arc::new(CountingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
        let mut streaming = EmbeddedStreamingDictation::default();
        streaming.embedded_session_id = Some(77);
        streaming.session = Some(embedded_audio_test_session(
            session_id,
            consumer_for_session,
        ));
        let pcm = pcm_from_samples(&samples_for_ms(100, 3_000));

        let complete = streaming
            .handle_ble_packet_actor_command(
                &coordinator.inner,
                crate::embedded_audio::StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                    session_id: 77,
                    packet_sequence: 0,
                    pcm: pcm.clone(),
                    after_stop_boundary: false,
                }),
            )
            .await
            .expect("BLE packet actor command handles PCM");

        assert!(!complete);
        assert_eq!(consumer.bytes.load(Ordering::SeqCst), pcm.len());
        assert_eq!(
            streaming
                .session
                .as_ref()
                .expect("streaming session remains active")
                .streamed_pcm_bytes,
            pcm.len()
        );
        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history
            .iter()
            .any(|record| record.command == EmbeddedBleSessionActorCommand::BlePacket));
        assert!(history.iter().any(|record| {
            record.command == EmbeddedBleSessionActorCommand::BlePacket
                && record.detail.contains("pcm_capsule")
        }));
    }

    #[test]
    fn embedded_audio_stop_feedback_is_ble_stop_boundary() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        assert!(emit_embedded_audio_transcribing_if_active(
            &coordinator.inner,
            session_id,
            Some("partial preview".to_string()),
        ));
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Listening);
        }

        coordinator.inner.state.lock().phase = SessionPhase::Processing;
        assert!(!emit_embedded_audio_transcribing_if_active(
            &coordinator.inner,
            session_id,
            Some("late preview".to_string()),
        ));
    }

    #[test]
    fn key_stop_feedback_latches_transcribing_without_processing_phase() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        let mut prefs = crate::types::UserPreferences::default();
        prefs.dictation_input_source = DictationInputSource::Microphone;
        coordinator.inner.prefs.replace_for_tests(prefs);
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        assert!(request_embedded_audio_stop_feedback(
            &coordinator.inner,
            "unit_test_stop_feedback"
        ));
        assert!(embedded_audio_stop_feedback_latched(&coordinator.inner));
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Listening);
        }
    }

    #[tokio::test]
    async fn host_stop_request_to_firmware_latches_feedback_without_local_finish() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        let handled = request_embedded_ble_recording_stop_from_host(
            &coordinator.inner,
            "unit_test_host_stop",
        )
        .await
        .expect("test stop request does not touch BLE transport");

        assert!(handled);
        assert!(!cancel_flag.load(Ordering::SeqCst));
        assert!(embedded_audio_stop_feedback_latched(&coordinator.inner));
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Listening);
        }
        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history.iter().any(|record| {
            record.command == EmbeddedBleSessionActorCommand::StopCommand
                && record.detail.contains("unit_test_host_stop")
        }));
    }

    #[test]
    fn embedded_audio_session_attaches_to_host_starting_session() {
        let coordinator = Coordinator::new();
        let mut prefs = coordinator.inner.prefs.get();
        prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
        coordinator.inner.prefs.replace_for_tests(prefs);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Starting;
            state.cancelled = false;
        }

        let attached = begin_embedded_audio_dictation_session_id(&coordinator.inner)
            .expect("BLE start packet should attach to the host-started session");

        assert_eq!(attached, session_id);
        let state = coordinator.inner.state.lock();
        assert_eq!(state.session_id, session_id);
        assert_eq!(state.phase, SessionPhase::Starting);
    }

    #[tokio::test]
    async fn host_stop_request_routes_by_embedded_ble_preference_without_capture_flag() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        let handled = request_embedded_ble_recording_stop_from_host(
            &coordinator.inner,
            "unit_test_host_stop_pref",
        )
        .await
        .expect("test stop request does not touch BLE transport");

        assert!(handled);
        assert!(embedded_audio_stop_feedback_latched(&coordinator.inner));
        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Listening);
        }
        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history.iter().any(|record| {
            record.command == EmbeddedBleSessionActorCommand::StopCommand
                && record.detail.contains("unit_test_host_stop_pref")
        }));
    }

    #[tokio::test]
    async fn session_actor_stop_command_owns_embedded_ble_stop_transition() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        end_embedded_ble_session(&coordinator.inner, true, "unit test stop command")
            .await
            .expect("stop command completes without ASR resource");

        {
            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, SessionPhase::Idle);
        }
        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history.iter().any(|record| {
            record.command == EmbeddedBleSessionActorCommand::StopCommand
                && record.detail.contains("unit test stop command")
        }));
    }

    #[test]
    fn session_actor_restart_history_covers_rapid_repeated_short_sessions() {
        let coordinator = Coordinator::new();
        let mut streaming = EmbeddedStreamingDictation::background_listener();

        assert!(streaming.keep_notify_ready_after_completed_pipeline_error(
            &coordinator.inner,
            "first short session ASR empty result"
        ));
        streaming.reset_for_next_session();
        assert!(streaming.keep_notify_ready_after_completed_pipeline_error(
            &coordinator.inner,
            "second short session polish failure"
        ));

        let restarts: Vec<_> = embedded_ble_session_actor_history(&coordinator.inner)
            .into_iter()
            .filter(|record| record.command == EmbeddedBleSessionActorCommand::ActorRestart)
            .collect();
        assert_eq!(restarts.len(), 2);
        assert!(restarts[0].seq < restarts[1].seq);
    }

    #[test]
    fn notify_cleanup_delay_records_listener_actor_command() {
        let coordinator = Coordinator::new();
        let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
        mark_embedded_ble_listener_ready(&coordinator.inner, &active);

        cancel_embedded_ble_listener_capture(
            &coordinator.inner,
            "notify cleanup delay test",
            false,
        );

        assert!(active.load(Ordering::SeqCst));
        let history = embedded_ble_session_actor_history(&coordinator.inner);
        assert!(history.iter().any(|record| {
            record.command == EmbeddedBleSessionActorCommand::NotifyCleanupDelay
                && record.detail.contains("notify cleanup delay test")
        }));
    }

    #[test]
    fn session_actor_diagnostics_expose_ordered_safe_event_context() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();

        record_embedded_ble_session_actor_command(
            &coordinator.inner,
            EmbeddedBleSessionActorCommand::AsrPartial,
            Some(session_id),
            "chars=12",
        );
        record_embedded_ble_session_actor_command(
            &coordinator.inner,
            EmbeddedBleSessionActorCommand::AsrFinal,
            Some(session_id),
            "transcript_empty=false",
        );

        let diagnostics = coordinator.embedded_ble_session_actor_diagnostics();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].seq, 1);
        assert_eq!(diagnostics[0].command, "asr_partial");
        assert_eq!(diagnostics[0].session_id, Some(session_id.to_string()));
        assert_eq!(diagnostics[0].detail, "chars=12");
        assert_eq!(diagnostics[1].seq, 2);
        assert_eq!(diagnostics[1].command, "asr_final");
        assert_eq!(diagnostics[1].detail, "transcript_empty=false");
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
    fn embedded_streaming_pcm_after_cancel_is_not_fed_to_asr() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Idle;
            state.cancelled = true;
        }
        let consumer = Arc::new(CountingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut session = embedded_audio_test_session(session_id, consumer_for_session);
        let pcm = pcm_from_samples(&samples_for_ms(100, 3_000));

        session
            .consume_streaming_pcm(&coordinator.inner, &pcm)
            .expect("cancelled PCM is ignored without error");

        assert_eq!(session.streamed_pcm_bytes, 0);
        assert_eq!(session.normalized_pcm_bytes, 0);
        assert!(!session.device_ai_processing_started);
        assert!(session.archive_pcm.as_ref().expect("archive").is_empty());
        assert_eq!(consumer.bytes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn embedded_streaming_pcm_for_active_session_feeds_asr_without_early_ai_led() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        let consumer = Arc::new(CountingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut session = embedded_audio_test_session(session_id, consumer_for_session);
        let pcm = pcm_from_samples(&samples_for_ms(100, 3_000));

        session
            .consume_streaming_pcm(&coordinator.inner, &pcm)
            .expect("active PCM is accepted");

        assert_eq!(session.streamed_pcm_bytes, pcm.len());
        assert_eq!(session.normalized_pcm_bytes, pcm.len());
        assert!(!session.device_ai_processing_started);
        assert_eq!(session.archive_pcm.as_ref().expect("archive"), &pcm);
        assert_eq!(consumer.bytes.load(Ordering::SeqCst), pcm.len());
    }

    #[test]
    fn embedded_streaming_pcm_combines_short_ble_packets_before_asr() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        let consumer = Arc::new(CapturingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut session = embedded_audio_test_session(session_id, consumer_for_session);
        let first_packet = pcm_from_samples(&samples_for_ms(40, 3_000));
        let second_packet = pcm_from_samples(&samples_for_ms(60, 3_000));
        let mut expected_pcm = first_packet.clone();
        expected_pcm.extend_from_slice(&second_packet);

        session
            .consume_streaming_pcm(&coordinator.inner, &first_packet)
            .expect("first short packet is accepted");
        assert!(consumer.chunks.lock().expect("capture lock").is_empty());
        assert_eq!(session.normalized_pcm_bytes, 0);

        session
            .consume_streaming_pcm(&coordinator.inner, &second_packet)
            .expect("second short packet is accepted");

        let chunks = consumer.chunks.lock().expect("capture lock");
        assert_eq!(chunks.as_slice(), [expected_pcm]);
        assert_eq!(session.streamed_pcm_bytes, EMBEDDED_AUDIO_FEED_CHUNK_BYTES);
        assert_eq!(
            session.normalized_pcm_bytes,
            EMBEDDED_AUDIO_FEED_CHUNK_BYTES
        );
        assert!(session.streaming_pcm_buffer.is_empty());
    }

    #[test]
    fn embedded_streaming_pcm_flushes_final_partial_block_once() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        let consumer = Arc::new(CapturingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut session = embedded_audio_test_session(session_id, consumer_for_session);
        let tail_pcm = pcm_from_samples(&samples_for_ms(50, 3_000));

        session
            .consume_streaming_pcm(&coordinator.inner, &tail_pcm)
            .expect("partial tail is accepted");
        assert!(consumer.chunks.lock().expect("capture lock").is_empty());

        session.flush_streaming_pcm();
        session.flush_streaming_pcm();

        let chunks = consumer.chunks.lock().expect("capture lock");
        assert_eq!(chunks.as_slice(), [tail_pcm]);
        assert_eq!(
            session.normalized_pcm_bytes,
            EMBEDDED_AUDIO_FEED_CHUNK_BYTES / 2
        );
        assert!(session.streaming_pcm_buffer.is_empty());
    }

    #[test]
    fn volcengine_streaming_agc_resolves_one_provider_block_not_each_ble_packet() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        let consumer = Arc::new(CountingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut session = embedded_audio_test_session(session_id, consumer_for_session);
        session.active_asr = "volcengine".into();
        let packet = pcm_from_samples(&samples_for_ms(20, 320));

        for _ in 0..4 {
            session
                .consume_streaming_pcm(&coordinator.inner, &packet)
                .expect("short voiced packet is accepted");
        }
        assert_eq!(consumer.bytes.load(Ordering::SeqCst), 0);
        assert_eq!(session.streaming_agc.voiced_chunks, 0);

        session
            .consume_streaming_pcm(&coordinator.inner, &packet)
            .expect("provider block is accepted");
        assert_eq!(
            consumer.bytes.load(Ordering::SeqCst),
            EMBEDDED_AUDIO_FEED_CHUNK_BYTES
        );
        assert_eq!(session.streaming_agc.voiced_chunks, 1);
        assert_eq!(session.streaming_agc.first_voiced_pcm_ms, Some(0));
    }

    #[test]
    fn embedded_streaming_reset_prepares_background_listener_for_next_session() {
        let mut streaming = EmbeddedStreamingDictation::default();
        let pcm = pcm_from_samples(&[100, -100]);

        streaming
            .collector
            .handle_notification(&build_session_start_notification(7))
            .expect("start notification");
        streaming
            .collector
            .handle_notification(
                &build_audio_data_notification(7, 0, &pcm).expect("audio notification"),
            )
            .expect("audio notification");
        streaming
            .collector
            .handle_notification(&build_session_stop_notification(7, 1))
            .expect("stop notification");
        streaming.terminal_received = true;

        let result = streaming
            .submission_result()
            .expect("complete streaming result");
        assert_eq!(result.stats.session_id, Some(7));
        assert_eq!(result.stats.received_packet_count, 1);
        assert_eq!(result.reconstructed_pcm_bytes, pcm.len());

        streaming.reset_for_next_session();

        assert!(!streaming.terminal_received);
        assert!(streaming.session.is_none());
        assert!(streaming.embedded_session_id.is_none());
        assert!(streaming.pending_stop_expected_packet_count.is_none());
        assert_eq!(streaming.collector.inner().stats().session_id, None);
        assert!(streaming.submission_result().is_err());
    }

    #[test]
    fn embedded_streaming_background_listener_keeps_pipeline_error_policy_after_reset() {
        let mut streaming = EmbeddedStreamingDictation::background_listener();

        assert!(streaming.keep_listening_after_pipeline_errors);
        streaming.terminal_received = true;
        streaming.embedded_session_id = Some(42);

        streaming.reset_for_next_session();

        assert!(streaming.keep_listening_after_pipeline_errors);
        assert!(!streaming.terminal_received);
        assert!(streaming.embedded_session_id.is_none());
        assert!(!EmbeddedStreamingDictation::default().keep_listening_after_pipeline_errors);
    }

    #[test]
    fn embedded_streaming_tail_chunk_remains_asr_input_until_the_session_drains() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        let consumer = Arc::new(CapturingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut session = embedded_audio_test_session(session_id, consumer_for_session);
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
        assert!(embedded_streaming_chunk_is_asr_input(&after_stop));
        session
            .consume_streaming_pcm(&coordinator.inner, &after_stop.pcm)
            .expect("post-stop drain PCM is forwarded to ASR");
        session.flush_streaming_pcm();

        let chunks = consumer.chunks.lock().expect("capture lock");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), after_stop.pcm.len());
    }

    #[test]
    fn embedded_ble_pcm_event_trace_is_sampled() {
        let first = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
            session_id: 1,
            packet_sequence: 0,
            pcm: vec![1, 2],
            after_stop_boundary: false,
        });
        let middle = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
            session_id: 1,
            packet_sequence: 17,
            pcm: vec![1, 2],
            after_stop_boundary: false,
        });
        let sample = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
            session_id: 1,
            packet_sequence: 50,
            pcm: vec![1, 2],
            after_stop_boundary: false,
        });
        let after_stop = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
            session_id: 1,
            packet_sequence: 51,
            pcm: vec![1, 2],
            after_stop_boundary: true,
        });

        assert!(embedded_ble_session_event_should_trace(&first));
        assert!(!embedded_ble_session_event_should_trace(&middle));
        assert!(embedded_ble_session_event_should_trace(&sample));
        assert!(embedded_ble_session_event_should_trace(&after_stop));
        assert!(embedded_ble_session_event_should_trace(
            &StreamingSessionEvent::Stopped {
                session_id: 1,
                expected_packet_count: 52,
                origin: crate::embedded_audio::SessionStopOrigin::User,
            }
        ));
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
    fn raw_mode_without_llm_uses_passthrough_instead_of_streaming_polish() {
        assert!(!streaming_insert_eligible(
            true,
            false,
            PolishMode::Raw,
            false,
            false
        ));
        assert!(streaming_insert_eligible(
            true,
            false,
            PolishMode::Raw,
            true,
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
    fn automatic_speaker_candidate_reaches_asr_only_after_verified_match() {
        assert!(super::speaker_candidate_may_reach_asr(false, None));
        assert!(super::speaker_candidate_may_reach_asr(true, Some(true)));
        assert!(!super::speaker_candidate_may_reach_asr(true, Some(false)));
        assert!(!super::speaker_candidate_may_reach_asr(true, None));
    }

    #[test]
    fn phrase_hit_waits_for_real_owner_audio_without_requiring_a_pause() {
        assert!(!super::owner_verification_window_ready(
            super::OWNER_VERIFICATION_START_BYTES - 2
        ));
        assert!(super::owner_verification_window_ready(
            super::OWNER_VERIFICATION_START_BYTES
        ));
        assert_eq!(super::next_owner_verification_retry_ms(1_100), Some(1_800));
        assert_eq!(super::next_owner_verification_retry_ms(1_800), Some(2_400));
        assert_eq!(super::next_owner_verification_retry_ms(2_400), None);
    }

    #[test]
    fn local_confirmation_adds_context_with_a_strict_attempt_cap() {
        assert_eq!(
            super::next_local_confirmation_snapshot_bytes(0),
            Some(1_800 * 32)
        );
        assert_eq!(
            super::next_local_confirmation_snapshot_bytes(1),
            Some(2_400 * 32)
        );
        assert_eq!(
            super::next_local_confirmation_snapshot_bytes(2),
            Some(3_000 * 32)
        );
        assert_eq!(super::next_local_confirmation_snapshot_bytes(3), None);
    }

    #[test]
    fn automatic_start_never_bypasses_hidden_candidate_gate() {
        use crate::embedded_audio::SessionStartOrigin;

        assert_eq!(
            super::buffered_speaker_candidate_kind(SessionStartOrigin::User, false, false),
            None
        );
        assert_eq!(
            super::buffered_speaker_candidate_kind(
                SessionStartOrigin::VoiceActivation,
                false,
                true
            ),
            Some(super::BufferedSpeakerCandidateKind::Verification)
        );
        assert_eq!(
            super::buffered_speaker_candidate_kind(
                SessionStartOrigin::VoiceActivation,
                false,
                false
            ),
            Some(super::BufferedSpeakerCandidateKind::Rejected)
        );
        assert_eq!(
            super::buffered_speaker_candidate_kind(SessionStartOrigin::Unknown(9), false, true),
            Some(super::BufferedSpeakerCandidateKind::Rejected)
        );

        let source = include_str!("dictation.rs");
        let start = source
            .find("async fn try_release_automatic_candidate")
            .expect("live automatic gate should exist");
        let end = source[start..]
            .find("async fn finish_completed_streaming_session")
            .map(|offset| start + offset)
            .expect("live automatic gate boundary");
        let body = &source[start..end];
        assert!(body.contains("detector.accept_pcm(&new_pcm)"));
        assert!(body.contains("candidate.kws_fed_bytes = candidate.pcm.len()"));
        assert!(body.contains("crate::speaker_verification::verify(&pcm)"));
        assert!(
            body.find("detector.accept_pcm(&new_pcm)")
                < body.find("crate::speaker_verification::verify(&pcm)")
        );
        assert!(
            body.find("let recording_control_task")
                < body.find("begin_embedded_audio_dictation_session")
        );
        assert!(
            body.find("begin_embedded_audio_dictation_session")
                < body.find("recording_control_task.await")
        );
        assert!(body.contains("latency_target_ms=1200"));
        assert!(body.contains("latency_ceiling_ms=1500"));
    }

    #[test]
    fn physical_hidden_candidate_promotion_discards_pre_press_pcm() {
        let mut pcm = vec![1, 2, 3, 4, 5, 6];
        assert_eq!(super::discard_pre_press_candidate_pcm(&mut pcm), 6);
        assert!(pcm.is_empty());

        let source = include_str!("dictation.rs");
        let start = source
            .find("async fn promote_hidden_candidate_if_requested")
            .expect("physical hidden-candidate promotion should exist");
        let end = source[start..]
            .find("async fn try_release_automatic_candidate")
            .map(|offset| start + offset)
            .expect("physical promotion helper boundary");
        let body = &source[start..end];
        assert!(body.contains("discard_pre_press_candidate_pcm(&mut candidate.pcm)"));
        assert!(!body.contains("session.consume_streaming_pcm"));
    }

    #[test]
    fn device_processing_completion_requires_a_matching_start() {
        assert!(!super::device_ai_processing_completion_allowed(
            false, false
        ));
        assert!(super::device_ai_processing_completion_allowed(true, false));
        assert!(!super::device_ai_processing_completion_allowed(true, true));
    }

    #[test]
    fn hidden_candidate_rejection_has_no_processing_led_command() {
        let source = include_str!("dictation.rs");
        let start = source
            .find("fn reject_hidden_automatic_candidate")
            .expect("hidden rejection helper should exist");
        let end = source[start..]
            .find("fn complete_voiceprint_enrollment_candidate")
            .map(|offset| start + offset)
            .expect("voiceprint enrollment helper should follow rejection helper");
        let body = &source[start..end];

        assert!(!body.contains("send_recording_processing_"));
        assert!(body.contains("rejected silently"));

        let start = source
            .find("async fn finish_end_session_after_stop_transition")
            .expect("stop pipeline should exist");
        let end = source[start..]
            .find("pub(super) fn dictation_error_code")
            .map(|offset| start + offset)
            .expect("stop pipeline boundary should exist");
        let body = &source[start..end];
        let processing_start = body
            .find("dictation_transcribing_processing_start")
            .expect("processing LED should start with the transcribing phase");
        let final_result_wait = body
            .find("asr.await_final_result()")
            .expect("streaming ASR should await its final result");

        assert!(processing_start < final_result_wait);
        assert!(!body.contains("dictation_text_ready_processing_start"));
    }

    #[test]
    fn clipboard_retention_takes_precedence_over_restore() {
        let mut prefs = UserPreferences::default();
        prefs.restore_clipboard_after_paste = true;
        prefs.copy_dictation_to_clipboard = true;
        assert!(!should_restore_clipboard_after_dictation(&prefs, true));
        assert!(should_restore_clipboard_after_dictation(&prefs, false));

        prefs.copy_dictation_to_clipboard = false;
        assert!(should_restore_clipboard_after_dictation(&prefs, false));
    }

    #[test]
    fn voice_activation_stop_never_counts_as_user_initiated() {
        assert!(!embedded_audio_stop_is_user_initiated(Some(
            crate::embedded_audio::SessionStopOrigin::VoiceActivation
        )));
        assert!(embedded_audio_stop_is_user_initiated(Some(
            crate::embedded_audio::SessionStopOrigin::User
        )));
        assert!(embedded_audio_stop_is_user_initiated(None));
    }

    #[test]
    fn post_dictation_key_requires_a_successful_plain_dictation_insert() {
        let enter = should_send_post_dictation_key(
            true,
            PostDictationKey::Enter,
            InsertStatus::Inserted,
            true,
            true,
            true,
            false,
        )
        .expect("inserted dictation should submit");
        assert_eq!(enter.primary, "Enter");
        assert!(enter.modifiers.is_empty());

        let ctrl_enter = should_send_post_dictation_key(
            true,
            PostDictationKey::CtrlEnter,
            InsertStatus::Inserted,
            true,
            true,
            true,
            false,
        )
        .expect("confirmed dictation should submit");
        assert_eq!(ctrl_enter.primary, "Enter");
        assert_eq!(ctrl_enter.modifiers, ["ctrl"]);

        for status in [
            InsertStatus::PasteSent,
            InsertStatus::CopiedFallback,
            InsertStatus::Failed,
        ] {
            assert!(should_send_post_dictation_key(
                true,
                PostDictationKey::Enter,
                status,
                true,
                true,
                true,
                false,
            )
            .is_none());
        }
        assert!(should_send_post_dictation_key(
            false,
            PostDictationKey::Enter,
            InsertStatus::Inserted,
            true,
            true,
            true,
            false,
        )
        .is_none());
        for denied_context in 0..3 {
            let (nonempty, target_restored, clipboard_satisfied) = match denied_context {
                0 => (false, true, true),
                1 => (true, false, true),
                _ => (true, true, false),
            };
            assert!(should_send_post_dictation_key(
                true,
                PostDictationKey::Enter,
                InsertStatus::Inserted,
                nonempty,
                target_restored,
                clipboard_satisfied,
                false,
            )
            .is_none());
        }
        assert!(should_send_post_dictation_key(
            true,
            PostDictationKey::Enter,
            InsertStatus::Inserted,
            true,
            true,
            true,
            true,
        )
        .is_none());
    }

    #[test]
    fn post_dictation_key_claim_is_once_per_session() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Inserting;
        }

        assert!(claim_post_dictation_key(&coordinator.inner, session_id));
        assert!(!claim_post_dictation_key(&coordinator.inner, session_id));
        assert!(!claim_post_dictation_key(
            &coordinator.inner,
            new_session_id()
        ));
    }

    #[test]
    fn default_done_message_treats_raw_insert_after_polish_failure_as_successful_fallback() {
        assert_eq!(
            default_done_message(InsertStatus::PasteSent, false),
            Some("已尝试粘贴".to_string())
        );
        assert_eq!(default_done_message(InsertStatus::Inserted, true), None);
        assert_eq!(
            default_done_message(InsertStatus::PasteSent, true),
            Some("已尝试粘贴原文".to_string())
        );
        let copied_message = if cfg!(target_os = "windows") {
            "已复制原文，请 Ctrl+V"
        } else {
            "已复制原文，请粘贴"
        };
        assert_eq!(
            default_done_message(InsertStatus::CopiedFallback, true),
            Some(copied_message.to_string())
        );
        assert_eq!(
            default_done_message(InsertStatus::Failed, true),
            Some("润色不可用，插入失败".to_string())
        );
    }

    #[test]
    fn device_processing_treats_raw_insert_after_polish_failure_as_success() {
        assert!(device_processing_final_succeeded(
            InsertStatus::Inserted,
            Some("polishFailed")
        ));
        assert!(device_processing_final_succeeded(
            InsertStatus::PasteSent,
            Some("polishFailed")
        ));
        assert!(device_processing_final_succeeded(
            InsertStatus::CopiedFallback,
            Some("polishFailed")
        ));
        assert!(!device_processing_final_succeeded(
            InsertStatus::Failed,
            Some("polishFailed")
        ));
        assert!(!device_processing_final_succeeded(
            InsertStatus::Inserted,
            Some("windowsImeTsfRequired")
        ));
    }

    #[test]
    fn device_processing_completion_delay_keeps_ai_led_visible() {
        let started_at = Instant::now();

        assert_eq!(
            device_ai_processing_completion_delay(Some(started_at), started_at),
            Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS)
        );
        assert_eq!(
            device_ai_processing_completion_delay(
                Some(started_at),
                started_at + Duration::from_millis(400),
            ),
            Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS - 400)
        );
        assert_eq!(
            device_ai_processing_completion_delay(
                Some(started_at),
                started_at + Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS + 1),
            ),
            Duration::from_millis(0)
        );
        assert_eq!(
            device_ai_processing_completion_delay(None, started_at),
            Duration::from_millis(0)
        );
    }

    #[test]
    fn device_processing_max_visible_timeout_is_bounded() {
        assert_eq!(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS, 5_000);
        assert!(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS > DEVICE_AI_PROCESSING_MIN_VISIBLE_MS);
        assert!(
            Duration::from_millis(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS) <= Duration::from_secs(5)
        );
    }

    #[test]
    fn embedded_ble_processing_sync_can_be_disabled_by_env() {
        let previous = std::env::var_os(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV);
        std::env::set_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, "1");
        assert!(embedded_ble_processing_sync_disabled());
        std::env::set_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, "false");
        assert!(!embedded_ble_processing_sync_disabled());
        match previous {
            Some(value) => std::env::set_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, value),
            None => std::env::remove_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV),
        }
    }

    #[test]
    fn unit_tests_never_send_device_ai_processing_side_effects() {
        assert!(!device_ai_processing_io_allowed());
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
    fn volcengine_streaming_agc_forwards_quiet_pcm_without_buffering() {
        let quiet = pcm_from_samples(&vec![12i16; 320]);
        let mut agc = EmbeddedStreamingAgcState::default();

        let (forwarded, stats) = normalize_embedded_streaming_pcm_for_asr(&quiet, &mut agc);

        assert_eq!(forwarded, quiet);
        assert_eq!(stats.gain, 1.0);
        assert_eq!(agc.quiet_chunks, 1);
        assert_eq!(agc.voiced_chunks, 0);
    }

    #[test]
    fn volcengine_streaming_agc_ignores_quiet_start_and_boosts_first_voice_immediately() {
        let quiet = pcm_from_samples(&vec![12i16; 320]);
        let voice = pcm_from_samples(&vec![320i16; 320]);
        let mut agc = EmbeddedStreamingAgcState::default();

        let (quiet_forwarded, _) = normalize_embedded_streaming_pcm_for_asr(&quiet, &mut agc);
        let (voice_forwarded, voice_stats) =
            normalize_embedded_streaming_pcm_for_asr(&voice, &mut agc);

        assert_eq!(quiet_forwarded, quiet);
        assert_eq!(voice_forwarded.len(), voice.len());
        assert!(voice_stats.gain > 1.0, "gain={}", voice_stats.gain);
        assert!(
            embedded_pcm_rms_and_peak(&voice_forwarded).0 > embedded_pcm_rms_and_peak(&voice).0
        );
        assert_eq!(agc.quiet_chunks, 1);
        assert_eq!(agc.voiced_chunks, 1);
        assert_eq!(agc.first_gain, Some(voice_stats.gain));
        assert_eq!(agc.max_gain, voice_stats.gain);
        assert_eq!(agc.gain_update_count, 1);
        assert_eq!(agc.clipped_samples, voice_stats.clipped_samples);
    }

    #[test]
    fn volcengine_streaming_agc_uses_full_bounded_gain_for_quiet_voiced_input() {
        let quiet_voice = pcm_from_samples(&vec![120i16; 1_600]);
        let mut agc = EmbeddedStreamingAgcState::default();

        let (_, stats) = normalize_embedded_streaming_pcm_for_asr(&quiet_voice, &mut agc);

        assert_eq!(stats.gain, EMBEDDED_AUDIO_MAX_GAIN);
        assert_eq!(agc.gain, EMBEDDED_AUDIO_MAX_GAIN);
        assert_eq!(agc.gain_update_count, 1);
    }

    #[test]
    fn volcengine_streaming_agc_amplifies_quiet_blocks_after_session_calibration() {
        let calibration_voice = pcm_from_samples(&vec![120i16; 1_600]);
        let quiet = pcm_from_samples(&vec![12i16; 1_600]);
        let mut agc = EmbeddedStreamingAgcState::default();

        normalize_embedded_streaming_pcm_for_asr(&calibration_voice, &mut agc);
        let (normalized_quiet, quiet_stats) =
            normalize_embedded_streaming_pcm_for_asr(&quiet, &mut agc);

        assert_eq!(quiet_stats.gain, EMBEDDED_AUDIO_MAX_GAIN);
        assert_ne!(normalized_quiet, quiet);
        assert_eq!(agc.quiet_chunks, 1);
        assert_eq!(agc.gain_update_count, 1);
    }

    #[test]
    fn volcengine_streaming_agc_does_not_calibrate_from_a_sparse_clipped_impulse() {
        let mut samples = vec![0i16; 1_600];
        samples[0] = i16::MIN;
        let pcm = pcm_from_samples(&samples);
        let mut agc = EmbeddedStreamingAgcState::default();

        let (normalized, stats) = normalize_embedded_streaming_pcm_for_asr(&pcm, &mut agc);

        assert_eq!(normalized, pcm);
        assert!(stats.rms_before > EMBEDDED_AUDIO_STREAMING_SPEECH_RMS);
        assert_eq!(agc.voiced_chunks, 0);
        assert_eq!(agc.quiet_chunks, 1);
        assert!(!agc.gain_calibrated);
    }

    #[test]
    fn volcengine_streaming_agc_preserves_spoken_gain_despite_a_sparse_clipped_impulse() {
        let mut samples = vec![180i16; 1_600];
        samples[0] = i16::MIN;
        let pcm = pcm_from_samples(&samples);
        let mut agc = EmbeddedStreamingAgcState::default();

        let (normalized, stats) = normalize_embedded_streaming_pcm_for_asr(&pcm, &mut agc);
        let (normalized_rms, _) = embedded_pcm_rms_and_peak(&normalized);

        assert!(stats.gain > 8.0, "gain={}", stats.gain);
        assert!(stats.clipped_samples >= 1);
        assert!(normalized_rms > EMBEDDED_AUDIO_TARGET_RMS * 0.8);
        assert_eq!(agc.first_gain, Some(stats.gain));
        assert!(agc.gain_calibrated);
    }

    #[test]
    fn volcengine_streaming_agc_limits_only_the_later_over_peak_block() {
        let calibration_voice = pcm_from_samples(&vec![500i16; 1_600]);
        let ordinary_voice = pcm_from_samples(&vec![900i16; 1_600]);
        let loud_voice = pcm_from_samples(&vec![16_000i16; 1_600]);
        let later_moderate_voice = pcm_from_samples(&vec![600i16; 1_600]);
        let mut agc = EmbeddedStreamingAgcState::default();

        let (_, first_stats) =
            normalize_embedded_streaming_pcm_for_asr(&calibration_voice, &mut agc);
        let calibrated_gain = first_stats.gain;
        assert!(calibrated_gain > 1.0);

        let (_, ordinary_stats) =
            normalize_embedded_streaming_pcm_for_asr(&ordinary_voice, &mut agc);
        assert_eq!(ordinary_stats.gain, calibrated_gain);

        let (_, loud_stats) = normalize_embedded_streaming_pcm_for_asr(&loud_voice, &mut agc);
        assert!(loud_stats.gain < calibrated_gain);
        assert!(loud_stats.gain >= 1.0);
        assert_eq!(loud_stats.clipped_samples, 0);

        let (_, later_stats) =
            normalize_embedded_streaming_pcm_for_asr(&later_moderate_voice, &mut agc);
        assert_eq!(later_stats.gain, calibrated_gain);
        assert_eq!(agc.gain_update_count, 1);
    }

    #[test]
    fn volcengine_streaming_agc_raises_for_later_quiet_confirmed_speech() {
        let calibration_voice = pcm_from_samples(&vec![700i16; 1_600]);
        let later_quiet_voice = pcm_from_samples(&vec![180i16; 1_600]);
        let mut agc = EmbeddedStreamingAgcState::default();

        let (_, first_stats) =
            normalize_embedded_streaming_pcm_for_asr(&calibration_voice, &mut agc);
        let (_, later_stats) =
            normalize_embedded_streaming_pcm_for_asr(&later_quiet_voice, &mut agc);

        assert!(later_stats.gain > first_stats.gain);
        assert_eq!(agc.gain, later_stats.gain);
        assert_eq!(agc.gain_update_count, 2);
    }

    #[test]
    fn volcengine_preview_and_final_share_the_authoritative_session() {
        let source = include_str!("dictation.rs");
        assert!(source.contains(
            "authoritative optimized-bidirectional ASR ready; preview and final share one provider session"
        ));
        assert!(source.contains("set_volcengine_preview_callbacks"));
        assert!(source.contains("asr.set_partial_transcript_callback"));
        assert!(source.contains("asr.set_final_intermediate_transcript_callback"));
        let builder_name = ["build", "_volcengine_asr("].concat();
        assert_eq!(source.matches(&builder_name).count(), 3);
    }
}
