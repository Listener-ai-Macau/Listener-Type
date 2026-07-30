use super::{
    append_typed_prefix, begin_embedded_audio_dictation_session_id,
    cancel_embedded_ble_listener_capture, cancel_session, claim_post_dictation_key,
    clear_embedded_ble_cancel_flag, current_embedded_audio_partial_preview, default_done_message,
    device_ai_processing_completion_delay, device_ai_processing_io_allowed,
    device_processing_final_succeeded, dictation_asr_engine_backend_id,
    dictation_asr_quality_warning, dictation_asr_uses_core_accurate_engine, dictation_error_code,
    embedded_audio_stop_feedback_latched, embedded_audio_stop_is_user_initiated,
    embedded_ble_listener_capture_ready, embedded_ble_processing_sync_disabled,
    embedded_ble_session_actor_history, embedded_ble_session_event_should_trace,
    embedded_ble_stream_idle_timeout, embedded_pcm_rms_and_peak, embedded_pcm_visual_level,
    embedded_streaming_chunk_is_asr_input, emit_embedded_audio_transcribing_if_active,
    end_embedded_ble_session, finalize_polished_text, finish_dictation_pipeline_error,
    finish_dictation_timeout, install_embedded_ble_listener_cancel,
    mark_embedded_ble_listener_ready, normalize_embedded_pcm_for_asr,
    normalize_embedded_streaming_pcm_for_asr, preserve_recording_transcript,
    provider_preview_change, publish_embedded_ble_asr_final,
    record_embedded_ble_session_actor_command, register_embedded_ble_cancel_flag,
    remove_standalone_dictation_fillers, request_embedded_audio_stop_feedback,
    request_embedded_ble_recording_stop_from_host, should_restore_clipboard_after_dictation,
    should_send_post_dictation_key, stabilize_embedded_audio_final_supplemental_preview,
    stabilize_embedded_audio_partial_preview, store_embedded_audio_stats,
    streaming_insert_eligible, update_embedded_audio_partial_preview, wayland_done_message,
    EmbeddedAudioDictationSession, EmbeddedBleSessionActorCommand, EmbeddedStreamingAgcState,
    EmbeddedStreamingDictation, DEVICE_AI_PROCESSING_MAX_VISIBLE_MS,
    DEVICE_AI_PROCESSING_MIN_VISIBLE_MS, EMBEDDED_AUDIO_FEED_CHUNK_BYTES, EMBEDDED_AUDIO_MAX_GAIN,
    EMBEDDED_AUDIO_STREAMING_SPEECH_RMS, EMBEDDED_AUDIO_TARGET_RMS,
    EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS,
    LOCAL_CONFIRMATION_START_BYTES, LOCAL_CONFIRMATION_START_MS,
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
    assert_eq!(LOCAL_CONFIRMATION_START_MS, 1_000);
    assert_eq!(LOCAL_CONFIRMATION_START_BYTES / 32, 1_000);
}

#[test]
fn wake_phrase_tail_latency_excludes_deliberate_phrase_duration() {
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(3.040, 3_216), 176);
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(0.900, 1_032), 132);
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(3.500, 3_200), 0);
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(f32::NAN, 900), 900);
}

#[test]
fn wake_diagnostic_retention_removes_expired_then_oldest_for_bytes() {
    let now = std::time::UNIX_EPOCH + Duration::from_secs(10 * 24 * 60 * 60);
    let entries = vec![
        super::WakeDiagnosticRetentionEntry {
            path: "expired.wav".into(),
            modified: std::time::UNIX_EPOCH,
            bytes: 1,
        },
        super::WakeDiagnosticRetentionEntry {
            path: "older.wav".into(),
            modified: now - Duration::from_secs(60),
            bytes: 20 * 1024 * 1024,
        },
        super::WakeDiagnosticRetentionEntry {
            path: "newer.wav".into(),
            modified: now - Duration::from_secs(30),
            bytes: 20 * 1024 * 1024,
        },
    ];

    let removals = super::wake_diagnostic_retention_plan(
        entries,
        now,
        Duration::from_secs(7 * 24 * 60 * 60),
        128,
        32 * 1024 * 1024,
    );
    assert_eq!(
        removals,
        vec![
            std::path::PathBuf::from("expired.wav"),
            std::path::PathBuf::from("older.wav")
        ]
    );
}

#[test]
fn wake_diagnostic_cleanup_caps_matching_files_and_keeps_unrelated_files() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "listener-wake-retention-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("create retention fixture");
    for index in 0..130 {
        std::fs::write(
            directory.join(format!("wake-candidate-{index:03}.wav")),
            [index as u8],
        )
        .expect("write matching fixture");
    }
    let unrelated = directory.join("operator-note.txt");
    std::fs::write(&unrelated, b"keep").expect("write unrelated fixture");

    let removed = super::prune_default_wake_diagnostics(&directory).expect("prune fixtures");
    let remaining_wavs = std::fs::read_dir(&directory)
        .expect("read retention fixture")
        .filter_map(Result::ok)
        .filter(|item| {
            item.path()
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("wav"))
        })
        .count();
    assert_eq!(removed, 2);
    assert_eq!(remaining_wavs, 128);
    assert!(unrelated.exists());

    std::fs::remove_dir_all(&directory).expect("remove retention fixture");
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
fn recording_transcript_preserves_wake_phrase_as_ordinary_speech() {
    assert_eq!(
        preserve_recording_transcript("开始录音，今天要说的是正文。"),
        "开始录音，今天要说的是正文。"
    );
    assert_eq!(
        preserve_recording_transcript("正常语句里开始录音只是普通内容。"),
        "正常语句里开始录音只是普通内容。"
    );
}

#[test]
fn automatic_wake_guard_removes_only_the_activation_prefix() {
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "开始录音。现在开始录音又开始不灵敏。",
            "开始录音",
            false,
        ),
        "现在开始录音又开始不灵敏。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "正常语句里开始录音只是普通内容。",
            "开始录音",
            false,
        ),
        "正常语句里开始录音只是普通内容。"
    );
}

#[test]
fn automatic_wake_guard_hides_partial_prefix_and_bounded_tail() {
    assert_eq!(
        super::strip_automatic_activation_prefix("开始录", "开始录音", true),
        ""
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("录音，正文开始。", "开始录音", false),
        "正文开始。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("音频测试", "开始录音", false),
        "音频测试"
    );
}

#[test]
fn remove_standalone_dictation_fillers_also_strips_inlined_chinese_fillers() {
    // 中文 ASR 常输出无标点的连续文本,语气词粘连在正文里——standalone 删不掉,
    // 这是用户觉得"开关没用"的根因。这里验证粘连的嗯/呃/唔会被剥离。
    assert_eq!(
        remove_standalone_dictation_fillers("今天嗯去测试"),
        "今天去测试"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("那个嗯文件"),
        "那个文件"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("呃我不知道"),
        "我不知道"
    );
    assert_eq!(remove_standalone_dictation_fillers("今天嗯嗯去"), "今天去");
    // 句首/句尾的粘连语气词也要去掉
    assert_eq!(
        remove_standalone_dictation_fillers("嗯今天嗯去嗯"),
        "今天去"
    );
    // 额有实义(额外/金额/名额),不剥离——只删被标点分隔的独立"额"
    assert_eq!(
        remove_standalone_dictation_fillers("金额是一百"),
        "金额是一百"
    );
    assert_eq!(remove_standalone_dictation_fillers("额外版本"), "额外版本");
    // 被标点分隔的独立语气词仍由 standalone 正常删除,不回归
    assert_eq!(
        remove_standalone_dictation_fillers("嗯，呃，今天测试。"),
        "今天测试。"
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
        stabilize_embedded_audio_final_supplemental_preview(Some("灵敏"), "预览灵敏稳定才算通过"),
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
fn embedded_audio_final_supplement_repairs_observed_early_cjk_rewrite_without_unrelated_takeover() {
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
        proactive_stop_body_started: false,
        proactive_stop_silence_ms: 0,
        proactive_stop_dispatched: false,
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
fn cancel_session_does_not_stop_background_listener_cancel_handle() {
    // Owner stability bug: capsule cancel used the continuous capture cancel flag,
    // so TYPE:READY was torn down (TYPE:BYE + CCCD off) on every Esc/cancel click.
    // Continuous background registers a separate session-abort flag; the listener
    // cancel handle must stay clear so notify remains open.
    let coordinator = Coordinator::new();
    let listener_cancel = Arc::new(AtomicBool::new(false));
    let session_abort = Arc::new(AtomicBool::new(false));
    {
        *coordinator.inner.embedded_ble_listener_cancel.lock() = Some(Arc::clone(&listener_cancel));
        coordinator
            .inner
            .embedded_ble_listener_ready
            .store(true, Ordering::SeqCst);
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &session_abort);
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    cancel_session(&coordinator.inner);

    assert!(
        session_abort.load(Ordering::SeqCst),
        "session soft-abort must still be requested for in-flight stream cleanup"
    );
    assert!(
        !listener_cancel.load(Ordering::SeqCst),
        "continuous background notify cancel handle must not be set by dictation cancel"
    );
    assert!(
        coordinator
            .inner
            .embedded_ble_listener_ready
            .load(Ordering::SeqCst),
        "cancel must not clear TYPE:READY flag; stream soft-abort keeps notify live"
    );
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

    let handled =
        request_embedded_ble_recording_stop_from_host(&coordinator.inner, "unit_test_host_stop")
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

    cancel_embedded_ble_listener_capture(&coordinator.inner, "notify cleanup delay test", false);

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
fn proactive_stop_accumulates_trailing_silence_only_after_body_started() {
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

    // Leading silence before the body must not arm the proactive stop.
    let silence = pcm_from_samples(&samples_for_ms(200, 0));
    session
        .consume_streaming_pcm(&coordinator.inner, &silence)
        .expect("leading silence accepted");
    assert!(!session.proactive_stop_body_started);
    assert_eq!(session.proactive_stop_silence_ms, 0);

    // A voiced block starts the body and zeroes trailing silence.
    let voiced = pcm_from_samples(&samples_for_ms(100, 3_000));
    session
        .consume_streaming_pcm(&coordinator.inner, &voiced)
        .expect("voiced body accepted");
    assert!(session.proactive_stop_body_started);
    assert_eq!(session.proactive_stop_silence_ms, 0);

    // Trailing silence accumulates only after the body has started, but a
    // single short gap must not yet cross the proactive-stop threshold.
    session
        .consume_streaming_pcm(&coordinator.inner, &silence)
        .expect("trailing silence accepted");
    assert!(session.proactive_stop_silence_ms > 0);
    assert!(
        session.proactive_stop_silence_ms < EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS,
        "a single 200ms gap should not yet cross the 1.2s threshold"
    );

    // Resuming speech resets the trailing-silence accumulator.
    session
        .consume_streaming_pcm(&coordinator.inner, &voiced)
        .expect("resume body accepted");
    assert_eq!(session.proactive_stop_silence_ms, 0);
    // The dispatcher lives in the packet handler; the session only exposes readiness.
    assert!(!session.proactive_stop_dispatched);
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
    // No enrolled voiceprint: phrase hit is enough (product contract). The 1.1s
    // owner window only applies when a template is enrolled for embedding quality.
    if crate::speaker_verification::is_enrolled() {
        assert!(!super::owner_verification_window_ready(
            super::OWNER_VERIFICATION_START_BYTES - 2
        ));
        assert!(super::owner_verification_window_ready(
            super::OWNER_VERIFICATION_START_BYTES
        ));
    } else {
        assert!(super::owner_verification_window_ready(0));
        assert!(super::owner_verification_window_ready(
            super::OWNER_VERIFICATION_START_BYTES - 2
        ));
    }
    assert_eq!(super::next_owner_verification_retry_ms(1_100), Some(1_800));
    assert_eq!(super::next_owner_verification_retry_ms(1_800), Some(2_400));
    assert_eq!(super::next_owner_verification_retry_ms(2_400), None);
}

#[test]
fn local_confirmation_adds_context_with_a_strict_attempt_cap() {
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(0),
        Some(1_000 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(1),
        Some(1_400 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(2),
        Some(1_800 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(3),
        Some(2_400 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(4),
        Some(3_000 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(5),
        Some(5_000 * 32)
    );
    assert_eq!(super::next_local_confirmation_snapshot_bytes(6), None);
}

#[test]
fn device_key_start_takeover_pending_before_hidden_active() {
    let polish = include_str!("dictation_wake_polish.rs");
    assert!(
        polish.contains("DEVICE_KEY_DICTATION_TAKEOVER_PENDING")
            && polish.contains("note_device_key_dictation_start_intent")
            && polish.contains("device-key takeover pending applied as promotion"),
        "device-key Start must sticky-promote when the hidden VA candidate is not ACTIVE yet"
    );
    let hotkey = include_str!("hotkey_device_runtime.rs");
    assert!(
        hotkey.contains("note_device_key_dictation_start_intent()"),
        "device-key dictation Start must call note_device_key_dictation_start_intent"
    );
    // Promote stays internal (ACTIVATE vs TOGGLE). Capsule copy must not say "接管"
    // — owners treat that as a product bug when voice-auto-start only had a hidden buffer.
    assert!(
        !hotkey.contains("\"正在接管当前录音...\""),
        "device-key Start capsule must not emit the legacy taking-over status string"
    );
    assert!(
        hotkey.contains("\"正在启动 Listener 录音...\""),
        "device-key Start capsule must use normal start copy even when promoting hidden VA"
    );
}

#[test]
fn hidden_candidate_marked_active_before_detector_init() {
    // Device-key promote depends on ACTIVE during StreamingDetector::new (~2s).
    let stream = include_str!("dictation_embedded_stream.rs");
    let begin = stream
        .find("async fn begin_candidate_or_session")
        .expect("begin_candidate_or_session");
    let body = &stream[begin..];
    let mark = body
        .find("mark_hidden_automatic_candidate_active()")
        .expect("must mark hidden ACTIVE for Verification");
    let detector = body
        .find("StreamingDetector::new(&phrase)")
        .expect("detector init");
    assert!(
        mark < detector,
        "mark_hidden_automatic_candidate_active must run before StreamingDetector::new so EC11 Start can promote instead of toggle-stop"
    );
    assert!(
        body.contains("detector_deferred") && body.contains("wake_detector_init"),
        "detector init must be deferred so PCM buffers during StreamingDetector::new"
    );
    let stream_all = include_str!("dictation_embedded_stream.rs");
    let dictation = include_str!("dictation.rs");
    assert!(
        stream_all.contains("show_early_wake_recording_capsule")
            && dictation.contains("local full-phrase confirmed")
            && stream_all.contains("stage2 timeout fail-open KeywordModel")
            && stream_all.contains("stage2 timeout held after explicit Absent")
            && stream_all.contains("terminal stage2 unavailable held after explicit Absent")
            && stream_all.contains("terminal stage2 task failure held after explicit Absent")
            && stream_all.contains("stage2 Absent reject")
            && stream_all.contains("KWS_SECONDARY_CONFIRM_BUDGET_MS"),
        "XiaoAi-style: stage2 Present/timeout fallback; explicit Absent remains authoritative through terminal confirmation"
    );
}

#[test]
fn orphan_pcm_without_explicit_start_must_not_open_dictation() {
    // Type restart / notify reopen can receive mid-stream PCM (no SessionStart).
    // That must not open a Recording capsule (phantom dictation).
    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("ignoring orphan embedded PCM without explicit start")
            && stream.contains("if self.session.is_none()")
            && stream.contains("no phantom recording on reconnect"),
        "PcmChunk path must drop orphan audio until explicit SessionStart creates session/candidate"
    );
    // The guard must sit before begin_session_if_needed on the no-candidate branch.
    let pcm_arm = stream
        .find("StreamingSessionEvent::PcmChunk(chunk)")
        .expect("pcm arm");
    let orphan = stream[pcm_arm..]
        .find("ignoring orphan embedded PCM without explicit start")
        .expect("orphan guard")
        + pcm_arm;
    let begin = stream[pcm_arm..]
        .find("self.begin_session_if_needed(inner, chunk.session_id)")
        .expect("begin_session_if_needed on pcm path")
        + pcm_arm;
    assert!(
        orphan < begin,
        "orphan PCM guard must run before begin_session_if_needed"
    );
}

#[test]
fn automatic_start_never_bypasses_hidden_candidate_gate() {
    use crate::embedded_audio::SessionStartOrigin;

    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::User, false, false),
        None
    );
    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::VoiceActivation, false, true),
        Some(super::BufferedSpeakerCandidateKind::Verification)
    );
    // Deleting the voiceprint must not disable automatic wake: still enter the
    // verification gate path; speaker_verification::verify open-gates when empty.
    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::VoiceActivation, false, false),
        Some(super::BufferedSpeakerCandidateKind::Verification)
    );
    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::Unknown(9), false, true),
        Some(super::BufferedSpeakerCandidateKind::Rejected)
    );

    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs")
    );
    assert!(
        source.contains("if !crate::speaker_verification::is_enrolled()")
            && source.contains("fn owner_verification_window_ready"),
        "no-voiceprint path must skip the owner speech window delay"
    );
    // Hidden ACTIVE must be marked before StreamingDetector::new (~1–2s init)
    // so device-key Start promotes instead of toggle-stop during that window.
    let mark_hidden = source
        .find("mark_hidden_automatic_candidate_active()")
        .expect("hidden automatic candidate must be marked active");
    let wake_init = source[mark_hidden..]
        .find("let wake_detector_init =")
        .map(|offset| mark_hidden + offset)
        .expect("wake detector init after hidden ACTIVE mark");
    let wake_window = &source[wake_init..wake_init + 1200.min(source.len() - wake_init)];
    assert!(
        wake_window.contains("StreamingDetector::new(&phrase)")
            && !wake_window.contains("StreamingDetector::new_strict"),
        "primary automatic wake must use StreamingDetector::new (sensitive), not new_strict"
    );
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

    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs")
    );
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
    // 候选必须先到再消费 promotion，否则第一次物理键按下会把 promotion 吃掉却开不了录音。
    let candidate_take = body
        .find("self.speaker_candidate.take()")
        .expect("promotion must take speaker candidate first");
    let promotion_take = body
        .find("take_hidden_automatic_candidate_promotion()")
        .expect("promotion must consume the promotion flag");
    assert!(
        candidate_take < promotion_take,
        "candidate must arrive before promotion is consumed"
    );
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
    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs")
    );
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
    assert!(Duration::from_millis(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS) <= Duration::from_secs(5));
}

#[test]
fn kws_hit_schedules_immediate_local_confirmation() {
    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("kws_prompted_local_confirm")
            && stream.contains("kws_immediate")
            && stream.contains("kws_retry")
            && stream.contains("KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_BYTES")
            && stream.contains("kws_local_absent_count")
            && stream.contains("kws_first_hit_at")
            && stream.contains("stage1 KWS hit")
            && stream.contains("stage2 timeout fail-open KeywordModel")
            && stream.contains("stage2 timeout held after explicit Absent")
            && stream.contains("stage2 Absent reject")
            && !stream.contains("KWS provisional accept after local Absent"),
        "XiaoAi-style cascade: stage1 KWS -> stage2 local; Absent blocks timeout fail-open"
    );
    let polish = include_str!("dictation_wake_polish.rs");
    assert!(
        polish.contains("kws_prompted_local_confirm")
            && polish.contains("KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS: usize = 800")
            && polish.contains("KWS_LOCAL_CONFIRM_RETRY_MS: usize = 400")
            && polish.contains("KWS_SECONDARY_CONFIRM_BUDGET_MS: u64 = 900")
            && polish.contains("KWS_SECONDARY_ABSENT_REJECT_COUNT: u8 = 2")
            && polish.contains("gain_normalized_pcm16"),
        "secondary budget 900ms + 2 Absent rejects + gain-boosted local ASR"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn explicit_absent_blocks_keyword_only_secondary_fallback() {
    assert!(super::secondary_fallback_can_accept_keyword(true, 0));
    assert!(!super::secondary_fallback_can_accept_keyword(true, 1));
    assert!(!super::secondary_fallback_can_accept_keyword(true, 2));
    assert!(!super::secondary_fallback_can_accept_keyword(false, 0));
}

#[cfg(target_os = "windows")]
#[test]
fn busy_local_wake_helper_is_retried_without_queue_or_keyword_fallback() {
    assert!(crate::asr::local::wake_helper::is_busy_error(
        "local wake confirmation failed: local_wake_helper_busy"
    ));
    assert!(!crate::asr::local::wake_helper::is_busy_error(
        "local wake helper did not start"
    ));

    let helper = include_str!("../asr/local/wake_helper.rs");
    assert!(
        helper.contains(".process\n                .try_lock()")
            && !helper.contains("let mut process_slot = self.process.lock();"),
        "local confirmations must be single-flight and non-queueing"
    );

    let stream = include_str!("dictation_embedded_stream.rs");
    let busy_branch = stream
        .find("is_busy_error(&err)")
        .expect("busy helper branch must exist");
    let unavailable_fallback = stream[busy_branch..]
        .find("secondary_fallback_can_accept_keyword(")
        .expect("ordinary helper failure fallback must remain");
    let busy_retry = stream[busy_branch..]
        .find("stage2 local confirm busy; retrying without queue")
        .expect("busy helper retry log must exist");
    assert!(
        busy_retry < unavailable_fallback,
        "busy backpressure must return before keyword-only fallback"
    );
}

#[test]
fn terminal_offline_recall_releases_the_actor_after_repeated_local_absence() {
    assert!(!super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES - 2,
        0
    ));
    assert!(super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        0
    ));
    assert!(super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        1
    ));
    assert!(!super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        super::TERMINAL_OFFLINE_SKIP_ABSENT_COUNT
    ));

    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("Duration::from_millis(TERMINAL_OFFLINE_RECALL_BUDGET_MS)")
            && stream.contains("terminal offline recall released actor after bounded wait")
            && stream.contains("candidate.local_absent_count.saturating_add(1)"),
        "terminal offline recovery must retain a bounded fallback without blocking later BLE input"
    );
    assert_eq!(super::TERMINAL_OFFLINE_RECALL_BUDGET_MS, 500);
}

#[test]
fn automatic_wake_discards_pre_wake_pcm_for_local_transcript() {
    // Regression: LocalTranscript forced post_wake_offset=0 and kept pre-wake speech.
    assert_eq!(super::post_wake_pcm_offset_bytes(0.0, 32_000), 0);
    assert_eq!(
        super::post_wake_pcm_offset_bytes(10.24, 400_000),
        ((10.24_f32 + 0.12) * 32_000.0) as usize
    );
    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("post_wake_pcm_offset_bytes(wake_match.end_seconds")
            && !stream.contains(
                "phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {\n                    ((wake_match.end_seconds"
            ),
        "LocalTranscript and KeywordModel must share post-wake PCM drain"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn exact_phrase_only_local_confirmation_refines_late_keyword_boundary() {
    let confirmation = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 4,
        inference_ms: 100,
        snapshot_pcm_ms: 2_775,
        recovered_keyword_end_seconds: None,
    };
    let refined = super::refined_wake_end_seconds(1.915, &confirmation, 4);
    assert!((refined - 2.655).abs() < 0.001);
    assert_eq!(super::post_wake_pcm_offset_bytes(refined, 98_400), 88_800);

    let with_body = super::LocalWakeConfirmation {
        transcript_chars: 11,
        ..confirmation
    };
    assert_eq!(super::refined_wake_end_seconds(0.685, &with_body, 4), 0.685);
}

#[cfg(target_os = "windows")]
#[test]
fn local_only_second_chance_requires_a_start_aligned_phrase() {
    use crate::wake_phrase::LocalPhraseRelation;

    assert!(super::local_confirmation_can_activate(
        false,
        LocalPhraseRelation::ExactStart
    ));
    assert!(super::local_confirmation_can_activate(
        false,
        LocalPhraseRelation::PhoneticStart
    ));
    assert!(!super::local_confirmation_can_activate(
        false,
        LocalPhraseRelation::PresentLater
    ));
    assert!(super::local_confirmation_can_activate(
        true,
        LocalPhraseRelation::PresentLater
    ));
}

#[test]
fn kws_local_confirmation_uses_only_an_aligned_five_second_tail() {
    let short = vec![7u8; super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES - 2];
    assert_eq!(super::local_confirmation_pcm(&short, true), short);

    let long: Vec<u8> = (0..super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES + 102)
        .map(|index| (index % 251) as u8)
        .collect();
    let bounded = super::local_confirmation_pcm(&long, true);
    assert_eq!(bounded.len(), super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES);
    assert_eq!(
        bounded,
        long[long.len() - super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES..]
    );
    assert_eq!(
        super::local_confirmation_pcm(&long, false),
        long,
        "exploratory local confirmation keeps the full candidate"
    );
}

#[test]
fn kws_phrase_focus_tail_is_shorter_than_five_second_cap() {
    let long: Vec<u8> = (0..super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES + 200)
        .map(|index| (index % 251) as u8)
        .collect();
    let focus = super::kws_phrase_focus_pcm(&long);
    assert_eq!(focus.len(), super::KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES);
    assert!(focus.len() < super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES);
    assert_eq!(
        focus,
        long[long.len() - super::KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES..]
    );
    let short = vec![9u8; 800];
    assert_eq!(super::kws_phrase_focus_pcm(&short), short);
}

#[test]
fn kws_absent_hard_reject_waits_for_post_hit_phrase_horizon() {
    // First hit at 1920 ms (session-288 style): Absents before +1 s must not
    // burn the two-Absent reject budget; later Absents remain authoritative.
    assert!(!super::kws_absent_counts_toward_reject(Some(1_920), 1_800));
    assert!(!super::kws_absent_counts_toward_reject(Some(1_920), 2_040));
    assert!(!super::kws_absent_counts_toward_reject(Some(1_920), 2_900));
    assert!(super::kws_absent_counts_toward_reject(Some(1_920), 2_920));
    assert!(super::kws_absent_counts_toward_reject(Some(1_920), 3_900));
    assert!(
        super::kws_absent_counts_toward_reject(None, 800),
        "without a recorded hit, Absent evidence stays authoritative"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn local_only_start_phrase_has_a_bounded_nonzero_audio_endpoint() {
    let confirmation = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 9,
        inference_ms: 100,
        snapshot_pcm_ms: 1_800,
        recovered_keyword_end_seconds: None,
    };
    let estimated = super::refined_wake_end_seconds(0.0, &confirmation, 4);
    assert!((estimated - 0.8).abs() < 0.001);

    let recovered = super::LocalWakeConfirmation {
        recovered_keyword_end_seconds: Some(0.72),
        ..confirmation
    };
    assert!((super::refined_wake_end_seconds(0.0, &recovered, 4) - 0.72).abs() < 0.001);

    let later_keyword_occurrence = super::LocalWakeConfirmation {
        recovered_keyword_end_seconds: Some(1.84),
        ..confirmation
    };
    assert!(
        (super::refined_wake_end_seconds(0.0, &later_keyword_occurrence, 4) - 0.8).abs() < 0.001
    );

    let later = super::LocalWakeConfirmation {
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::PresentLater,
        recovered_keyword_end_seconds: None,
        ..confirmation
    };
    assert_eq!(super::refined_wake_end_seconds(0.0, &later, 4), 0.0);
}

#[test]
fn device_processing_max_visible_timeout_stops_without_done() {
    // Long ASR/polish: max-visible must only clear purple AI. PROCESSING:DONE is the
    // green OK flash and must fire once at real completion — not again at the 5s cap.
    let source = include_str!("dictation_device_ai.rs");
    let begin = source
        .find("fn schedule_device_ai_processing_max_visible_timeout")
        .expect("max-visible scheduler");
    let body = &source[begin..];
    let end = body[1..].find("\nfn ").map(|i| i + 1).unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("send_recording_processing_state(false"),
        "max-visible must STOP AI LED"
    );
    assert!(
        !body.contains("send_recording_processing_done"),
        "max-visible must not send DONE (avoids intermittent double green OK)"
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
    let (voice_forwarded, voice_stats) = normalize_embedded_streaming_pcm_for_asr(&voice, &mut agc);

    assert_eq!(quiet_forwarded, quiet);
    assert_eq!(voice_forwarded.len(), voice.len());
    assert!(voice_stats.gain > 1.0, "gain={}", voice_stats.gain);
    assert!(embedded_pcm_rms_and_peak(&voice_forwarded).0 > embedded_pcm_rms_and_peak(&voice).0);
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

    let (_, first_stats) = normalize_embedded_streaming_pcm_for_asr(&calibration_voice, &mut agc);
    let calibrated_gain = first_stats.gain;
    assert!(calibrated_gain > 1.0);

    let (_, ordinary_stats) = normalize_embedded_streaming_pcm_for_asr(&ordinary_voice, &mut agc);
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

    let (_, first_stats) = normalize_embedded_streaming_pcm_for_asr(&calibration_voice, &mut agc);
    let (_, later_stats) = normalize_embedded_streaming_pcm_for_asr(&later_quiet_voice, &mut agc);

    assert!(later_stats.gain > first_stats.gain);
    assert_eq!(agc.gain, later_stats.gain);
    assert_eq!(agc.gain_update_count, 2);
}

#[test]
fn volcengine_preview_and_final_share_the_authoritative_session() {
    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs")
    );
    assert!(source.contains(
        "authoritative optimized-bidirectional ASR ready; preview and final share one provider session"
    ));
    assert!(source.contains("set_volcengine_preview_callbacks"));
    assert!(source.contains("asr.set_partial_transcript_callback"));
    assert!(source.contains("asr.set_final_intermediate_transcript_callback"));
    let builder_name = ["build", "_volcengine_asr("].concat();
    assert_eq!(source.matches(&builder_name).count(), 3);
}
