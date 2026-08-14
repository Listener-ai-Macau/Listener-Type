use super::{
    acknowledge_automatic_wake_capsule_visible, append_typed_prefix, arm_automatic_wake_text_guard,
    automatic_wake_body_started, automatic_wake_initial_body_wait_active,
    automatic_wake_session_active, begin_embedded_audio_dictation_session_id,
    cancel_embedded_ble_listener_capture, cancel_session, claim_post_dictation_key,
    clear_automatic_wake_text_guard, clear_embedded_ble_cancel_flag,
    current_embedded_audio_partial_preview, default_done_message,
    device_ai_processing_completion_delay, device_ai_processing_io_allowed,
    device_processing_final_succeeded, dictation_asr_engine_backend_id,
    dictation_asr_quality_warning, dictation_asr_uses_core_accurate_engine, dictation_error_code,
    drive_polish_prefetch, embedded_audio_stop_feedback_latched,
    embedded_audio_stop_is_user_initiated, embedded_ble_listener_capture_ready,
    embedded_ble_processing_sync_disabled, embedded_ble_session_actor_history,
    embedded_ble_session_event_should_trace, embedded_ble_stream_idle_timeout,
    embedded_pcm_capsule_level, embedded_pcm_rms_and_peak, embedded_pcm_visual_level,
    embedded_streaming_chunk_is_asr_input, emit_embedded_audio_transcribing_if_active,
    end_embedded_ble_session, filter_automatic_wake_text, finalize_polished_text,
    finish_dictation_pipeline_error, finish_dictation_timeout,
    install_embedded_ble_listener_cancel, mark_embedded_ble_listener_ready,
    normalize_embedded_pcm_for_asr, normalize_embedded_streaming_pcm_for_asr,
    polish_prefetch_adoptable, preserve_recording_transcript, provider_preview_change,
    publish_embedded_ble_asr_final, record_embedded_ble_session_actor_command,
    register_embedded_ble_cancel_flag, remove_standalone_dictation_fillers,
    request_embedded_audio_stop_feedback, request_embedded_ble_recording_stop_from_host,
    should_restore_clipboard_after_dictation, should_send_post_dictation_key,
    stabilize_embedded_audio_final_supplemental_preview, stabilize_embedded_audio_partial_preview,
    store_embedded_audio_stats, streaming_insert_eligible, update_embedded_audio_partial_preview,
    wayland_done_message, EmbeddedAudioDictationSession, EmbeddedBleSessionActorCommand,
    EmbeddedStreamingAgcState, EmbeddedStreamingDictation, DEVICE_AI_PROCESSING_MAX_VISIBLE_MS,
    DEVICE_AI_PROCESSING_MIN_VISIBLE_MS, EMBEDDED_AUDIO_FEED_CHUNK_BYTES,
    EMBEDDED_AUDIO_HOST_LIMITER_PEAK, EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV,
    EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS, LOCAL_CONFIRMATION_START_BYTES,
    LOCAL_CONFIRMATION_START_MS,
};
use crate::coordinator::Coordinator;
use crate::coordinator::{PolishPrefetch, PolishPrefetchBuf};
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

#[derive(Default)]
struct DeferredBridgeTestConsumer {
    pcm: Mutex<Vec<u8>>,
}

impl crate::asr::AudioConsumer for DeferredBridgeTestConsumer {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.pcm
            .lock()
            .expect("test pcm lock")
            .extend_from_slice(pcm);
    }
}

#[test]
fn deferred_asr_bridge_flushes_prefix_once_and_forwards_tail_in_order() {
    let bridge = super::DeferredAsrBridge::new();
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[1, 2, 3]);
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[4, 5]);

    let target = Arc::new(DeferredBridgeTestConsumer::default());
    let asr_target: Arc<dyn crate::asr::AudioConsumer> = target.clone();
    assert_eq!(bridge.attach(asr_target), 5);
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[6, 7]);

    assert_eq!(
        target.pcm.lock().expect("test pcm lock").as_slice(),
        &[1, 2, 3, 4, 5, 6, 7]
    );
}

#[test]
fn local_confirmation_waits_for_pre_roll_plus_speech_observation() {
    assert_eq!(LOCAL_CONFIRMATION_START_MS, 800);
    assert_eq!(LOCAL_CONFIRMATION_START_BYTES / 32, 800);
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
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "嗯，开始录音，多人识别现在只保留我说的话。",
            "开始录音",
            false,
        ),
        "多人识别现在只保留我说的话。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("嗯嗯开始录音。正文保持完整。", "开始录音", false,),
        "正文保持完整。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "额外版本，开始录音只是普通内容。",
            "开始录音",
            false,
        ),
        "额外版本，开始录音只是普通内容。"
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
        volcengine_asr: None,
        archive_pcm: Some(Vec::new()),
        streamed_pcm_bytes: 0,
        normalized_pcm_bytes: 0,
        streaming_pcm_buffer: Vec::new(),
        streaming_agc: EmbeddedStreamingAgcState::default(),
        local_speaker_tracker: None,
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

#[test]
fn embedded_pcm_capsule_level_prefers_firmware_raw_meter_and_keeps_legacy_fallback() {
    let processed_loud = pcm_from_samples(&[2_000, -2_000, 2_000, -2_000]);
    let processed_quiet = pcm_from_samples(&[20, -20, 20, -20]);

    let quiet_raw = embedded_pcm_capsule_level(&processed_loud, Some(7));
    let loud_raw = embedded_pcm_capsule_level(&processed_quiet, Some(83));
    assert!(
        (quiet_raw - 0.03496).abs() < 0.00001,
        "quiet_raw={quiet_raw}"
    );
    assert!((loud_raw - 0.28424).abs() < 0.00001, "loud_raw={loud_raw}");
    assert_eq!(
        embedded_pcm_capsule_level(&processed_quiet, None),
        embedded_pcm_visual_level(&processed_quiet)
    );

    let sweep = [1, 21, 99].map(|level| embedded_pcm_capsule_level(&processed_loud, Some(level)));
    assert!(sweep[0] < sweep[1] && sweep[1] < sweep[2]);
    assert!(
        sweep[2] < 0.34,
        "24 dB sweep must not saturate the capsule response: {sweep:?}"
    );
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
fn polish_prefetch_adoptable_only_on_exact_final_match_and_no_failure() {
    use std::collections::VecDeque;
    let make = |input: &str, result: Option<super::StreamingPolishOutcome>| PolishPrefetch {
        input: input.to_string(),
        buf: Arc::new(parking_lot::Mutex::new(PolishPrefetchBuf {
            chunks: VecDeque::new(),
            result,
        })),
        notify: Arc::new(tokio::sync::Notify::new()),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    assert!(polish_prefetch_adoptable(
        &make("整理后的正文", None),
        "整理后的正文"
    ));
    assert!(
        !polish_prefetch_adoptable(&make("整理后的正文", None), "整理后的正文，多了尾巴"),
        "final with extra tail must not adopt"
    );
    assert!(!polish_prefetch_adoptable(
        &make(
            "整理后的正文",
            Some(super::StreamingPolishOutcome::Failed("idle timeout".into()))
        ),
        "整理后的正文"
    ));
}

#[tokio::test]
async fn drive_polish_prefetch_replays_buffer_then_streams_live() {
    use std::collections::VecDeque;
    let prefetch = PolishPrefetch {
        input: "正文".to_string(),
        buf: Arc::new(parking_lot::Mutex::new(PolishPrefetchBuf {
            chunks: VecDeque::from(["你".to_string(), "你好".to_string()]),
            result: None,
        })),
        notify: Arc::new(tokio::sync::Notify::new()),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let buf = Arc::clone(&prefetch.buf);
    let notify = Arc::clone(&prefetch.notify);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let drive = tokio::spawn(drive_polish_prefetch(prefetch, tx));
    // 等驱动把两个缓冲 chunk 回放完，再补一个 live chunk + 结束。
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    buf.lock().chunks.push_back("你好世".to_string());
    notify.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    buf.lock().result = Some(super::StreamingPolishOutcome::Streamed("你好世界".into()));
    notify.notify_one();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), drive)
        .await
        .expect("drive must finish")
        .expect("drive task");
    let mut received = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        received.push(chunk);
    }
    assert_eq!(received, vec!["你", "你好", "你好世"]);
    match outcome {
        super::StreamingPolishOutcome::Streamed(text) => assert_eq!(text, "你好世界"),
        _ => panic!("expected Streamed outcome"),
    }
}

#[test]
fn repeated_idle_hotkey_cancels_are_deduped_at_the_bridge() {
    // 2026-08-07 storm: 1875 idle Esc/cancels, each bouncing the background
    // listener actor (stale session cancel flag gave every one real work).
    // The bridge suppresses repeat Idle cancels; a non-Idle cancel re-arms.
    let coordinator = Coordinator::new();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
    coordinator.inner.state.lock().phase = SessionPhase::Idle;

    let (tx, rx) = std::sync::mpsc::channel();
    let inner = std::sync::Arc::clone(&coordinator.inner);
    let handle = std::thread::spawn(move || crate::coordinator::hotkey_bridge_loop(inner, rx));
    tx.send(crate::hotkey::HotkeyEvent::Cancelled).unwrap();
    tx.send(crate::hotkey::HotkeyEvent::Cancelled).unwrap();
    drop(tx);
    handle.join().unwrap();

    let history = embedded_ble_session_actor_history(&coordinator.inner);
    let cancel_commands = history
        .iter()
        .filter(|record| record.command == EmbeddedBleSessionActorCommand::CancelCommand)
        .count();
    assert_eq!(cancel_commands, 1, "repeat Idle cancels must be deduped");
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
                raw_input_level_percent: Some(31),
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
        .consume_streaming_pcm(&coordinator.inner, &pcm, None)
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
        .consume_streaming_pcm(&coordinator.inner, &pcm, None)
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
        .consume_streaming_pcm(&coordinator.inner, &first_packet, None)
        .expect("first short packet is accepted");
    assert!(consumer.chunks.lock().expect("capture lock").is_empty());
    assert_eq!(session.normalized_pcm_bytes, 0);

    session
        .consume_streaming_pcm(&coordinator.inner, &second_packet, None)
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
        .consume_streaming_pcm(&coordinator.inner, &silence, None)
        .expect("leading silence accepted");
    assert!(!session.proactive_stop_body_started);
    assert_eq!(session.proactive_stop_silence_ms, 0);

    // A voiced block starts the body and zeroes trailing silence.
    let voiced = pcm_from_samples(&samples_for_ms(100, 3_000));
    session
        .consume_streaming_pcm(&coordinator.inner, &voiced, None)
        .expect("voiced body accepted");
    assert!(session.proactive_stop_body_started);
    assert_eq!(session.proactive_stop_silence_ms, 0);

    // Trailing silence accumulates only after the body has started, but a
    // single short gap must not yet cross the proactive-stop threshold.
    session
        .consume_streaming_pcm(&coordinator.inner, &silence, None)
        .expect("trailing silence accepted");
    assert!(session.proactive_stop_silence_ms > 0);
    assert!(
        session.proactive_stop_silence_ms < EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS,
        "a single 200ms gap should not yet cross the 1.2s threshold"
    );

    // Resuming speech resets the trailing-silence accumulator.
    session
        .consume_streaming_pcm(&coordinator.inner, &voiced, None)
        .expect("resume body accepted");
    assert_eq!(session.proactive_stop_silence_ms, 0);
    // The dispatcher lives in the packet handler; the session only exposes readiness.
    assert!(!session.proactive_stop_dispatched);
}

#[test]
fn finished_sentences_stay_snappy_incomplete_body_holds() {
    let base = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(1_500),
        provider_audio_duration_ms: Some(2_500),
        audio_duration_ms: Some(2_500),
        local_speech_end_ms: Some(1_500),
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(1_500),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    // Standard 1.0s is due at +1000; a wider 2.0s window is not yet due.
    assert!(super::target_speaker_endpoint_due(&base));
    assert!(!super::target_speaker_endpoint_due_with_timeout(
        &base, 2_000
    ));

    let wider_window_due = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(3_500),
        audio_duration_ms: Some(3_500),
        ..base
    };
    assert!(super::target_speaker_endpoint_due_with_timeout(
        &wider_window_due,
        2_000
    ));
    assert_eq!(
        super::target_speaker_inactive_stop_reason(1_000),
        "target_speaker_inactive_1000ms"
    );
    // Finished sentences with 。？！ stay snappy 1.0s (old "terminal→2s" made
    // every Chinese short utterance feel slow). Incomplete body holds 1.5s so
    // mid-thought pauses are not cut (installed "现在是进入" / "你帮").
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("用全刷。")),
        1_000
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("简单说一下。")),
        1_000
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("现在整体是一个什么进度？")),
        1_000
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("我先检查一下，然后。")),
        2_000,
        "an optimistic full stop must not hide a dangling continuation",
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("最后。")),
        2_000,
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("最后一句要完整。")),
        1_000,
        "ordinary words containing a connector are still complete",
    );
    assert!(super::preview_has_dangling_continuation(Some(
        "这部分已经完成，但是。"
    )));
    assert!(!super::preview_has_dangling_continuation(Some(
        "这部分已经完成。"
    )));
    assert!(super::preview_has_dangling_continuation(Some(
        "We can continue, and."
    )));
    assert!(!super::preview_has_dangling_continuation(Some(
        "This is a brand."
    )));
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你继续帮我看一下吧")),
        1_500
    );
    // 5 spoken chars → short-body ladder (≤4 is 2.5s).
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("现在是进入")),
        1_500
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("那你")),
        2_500
    );
    assert_eq!(
        super::target_speaker_inactive_stop_reason(1_500),
        "target_speaker_inactive_1500ms"
    );
    assert_eq!(
        super::target_speaker_inactive_stop_reason(2_500),
        "target_speaker_inactive_2500ms"
    );
    assert!(super::preview_ends_with_sentence_terminal(Some(
        "现在整体是一个什么进度？你跟我简单说一下。"
    )));
    assert!(!super::preview_ends_with_sentence_terminal(Some(
        "你继续帮我看一下吧。就是他进入"
    )));
}

#[test]
fn target_speaker_endpoint_requires_one_second_without_that_speaker() {
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(1_500),
        provider_audio_duration_ms: Some(2_499),
        audio_duration_ms: Some(2_499),
        local_speech_end_ms: Some(1_500),
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(1_500),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&update));

    let due = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(2_500),
        audio_duration_ms: Some(2_500),
        ..update.clone()
    };
    assert!(super::target_speaker_endpoint_due(&due));

    let pending = crate::asr::volcengine::TargetSpeakerUpdate {
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        ..due.clone()
    };
    assert!(!super::target_speaker_endpoint_due(&pending));

    let unresolved_recent_local = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(3_100),
        local_speech_end_ms: Some(3_000),
        stable_attributed_speech_end_ms: Some(1_500),
        ..due.clone()
    };
    assert!(!super::target_speaker_endpoint_due(
        &unresolved_recent_local
    ));

    // Confirmed other-speaker energy does not refresh the owner clock: once the
    // owner has been inactive for 1000 ms, auto-end proceeds while others talk.
    let unresolved_local_is_confidently_other_speaker =
        crate::asr::volcengine::TargetSpeakerUpdate {
            local_non_target_speech_end_ms: Some(3_000),
            ..unresolved_recent_local.clone()
        };
    assert!(super::target_speaker_endpoint_due(
        &unresolved_local_is_confidently_other_speaker
    ));

    let stale_non_target_classification = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(2_899),
        ..unresolved_recent_local.clone()
    };
    assert!(!super::target_speaker_endpoint_due(
        &stale_non_target_classification
    ));

    let unresolved_local_has_reached_its_own_one_second_endpoint =
        crate::asr::volcengine::TargetSpeakerUpdate {
            audio_duration_ms: Some(4_000),
            ..unresolved_recent_local
        };
    assert!(super::target_speaker_endpoint_due(
        &unresolved_local_has_reached_its_own_one_second_endpoint
    ));

    let no_identity = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_info_present: false,
        ..due
    };
    assert!(!super::target_speaker_endpoint_due(&no_identity));

    let local_wake_target = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(2_499),
        local_speech_end_ms: Some(1_500),
        local_target_speech_end_ms: Some(1_500),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    assert!(!super::target_speaker_endpoint_due(&local_wake_target));
    // Pending provisional body must never auto-end (installed mid-sentence cut
    // 19df34c4). Local wake authority alone is not enough while unattributed
    // speech is still streaming.
    let local_wake_still_pending = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(2_500),
        pending_unattributed_speech: true,
        ..local_wake_target.clone()
    };
    assert!(!super::target_speaker_endpoint_due(
        &local_wake_still_pending
    ));
    let local_wake_due = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(2_500),
        pending_unattributed_speech: false,
        ..local_wake_target
    };
    assert!(super::target_speaker_endpoint_due(&local_wake_due));
}

#[test]
fn target_speaker_endpoint_uses_newest_stable_attributed_boundary_after_diarization_flip() {
    // Installed session 909d8f82 reproduced a same-owner diarization flip:
    // target speaker stopped at 13772 ms, while the provider had already
    // stabilized a newer spoken tail through 15742 ms. Stopping at provider
    // audio 16300 ms therefore waited only 558 ms and truncated the owner.
    // Newest protection clock is stable_attributed 15742 ms; exact due is +1000.
    let one_ms_before = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(13_772),
        provider_audio_duration_ms: Some(16_741),
        audio_duration_ms: Some(16_800),
        local_speech_end_ms: Some(15_742),
        local_target_speech_end_ms: Some(15_200),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(15_742),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&one_ms_before));

    let exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(16_742),
        ..one_ms_before
    };
    assert!(super::target_speaker_endpoint_due(&exact_endpoint));
}

#[test]
fn confirmed_other_speaker_does_not_extend_endpoint_via_provider_attribution() {
    // Installed session 245: the owner ended at 10842 ms. A nearby speaker then
    // advanced stable attribution to 15132 ms and kept a provisional tail open.
    // Repeated strong local NonTarget evidence must keep both provider channels
    // from extending the owner's endpoint clock.
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(10_842),
        provider_audio_duration_ms: Some(15_600),
        audio_duration_ms: Some(15_700),
        local_speech_end_ms: Some(14_700),
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(14_600),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(15_132),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: true,
    };

    assert!(super::target_speaker_endpoint_due(&update));
    let without_local_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: None,
        ..update
    };
    assert!(!super::target_speaker_endpoint_due(&without_local_other));
}

#[test]
fn target_speaker_endpoint_waits_for_provider_coverage_before_stopping_quiet_tail() {
    let provider_is_behind = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(5_112),
        provider_audio_duration_ms: Some(6_200),
        audio_duration_ms: Some(6_900),
        local_speech_end_ms: Some(5_900),
        local_target_speech_end_ms: Some(5_700),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(5_112),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&provider_is_behind));

    let quiet_tail_arrives = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(6_442),
        provider_audio_duration_ms: Some(6_900),
        stable_attributed_speech_end_ms: Some(6_442),
        target_activity_advanced: true,
        ..provider_is_behind.clone()
    };
    assert!(!super::target_speaker_endpoint_due(&quiet_tail_arrives));

    let one_ms_before_exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(7_441),
        audio_duration_ms: Some(7_700),
        ..quiet_tail_arrives.clone()
    };
    assert!(!super::target_speaker_endpoint_due(
        &one_ms_before_exact_endpoint
    ));

    let exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(7_442),
        ..one_ms_before_exact_endpoint
    };
    assert!(super::target_speaker_endpoint_due(&exact_endpoint));
}

#[test]
fn target_speaker_endpoint_uses_local_clock_only_for_a_clean_provider_stall() {
    let one_ms_before = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(9_992),
        provider_audio_duration_ms: Some(10_400),
        audio_duration_ms: Some(10_991),
        local_speech_end_ms: Some(9_400),
        local_target_speech_end_ms: Some(9_400),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(9_992),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &one_ms_before,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &one_ms_before,
        true,
        1_000,
    ));

    let exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(10_992),
        ..one_ms_before.clone()
    };
    assert!(super::provider_stall_local_endpoint_due(
        &exact_endpoint,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &exact_endpoint,
        true,
        1_000,
    ));

    let ordinary_provider_lag = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(10_493),
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &ordinary_provider_lag,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &ordinary_provider_lag,
        true,
        1_000,
    ));

    let pending_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        pending_unattributed_speech: true,
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &pending_tail,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &pending_tail,
        true,
        1_000,
    ));

    // Ongoing unclassified local energy during a provider stall must NOT end
    // the session (installed mid-cut: ASR text froze while owner kept talking).
    let mid_sentence_local_energy = crate::asr::volcengine::TargetSpeakerUpdate {
        local_speech_end_ms: Some(10_400),
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &mid_sentence_local_energy,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &mid_sentence_local_energy,
        true,
        1_000,
    ));

    // Residual energy that has itself been quiet for the full endpoint interval
    // may still use stall fallback so room noise does not hold the session open.
    let residual_energy_now_quiet = crate::asr::volcengine::TargetSpeakerUpdate {
        local_speech_end_ms: Some(9_900),
        ..exact_endpoint.clone()
    };
    assert!(super::provider_stall_local_endpoint_due(
        &residual_energy_now_quiet,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &residual_energy_now_quiet,
        true,
        1_000,
    ));

    let confirmed_other_speaker = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(11_000),
        local_speech_end_ms: Some(10_500),
        local_non_target_speech_end_ms: Some(10_500),
        ..exact_endpoint.clone()
    };
    assert!(super::provider_stall_local_endpoint_due(
        &confirmed_other_speaker,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &confirmed_other_speaker,
        true,
        1_000,
    ));

    let newer_local_target_one_ms_before = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(4_572),
        provider_audio_duration_ms: Some(5_200),
        audio_duration_ms: Some(5_899),
        local_speech_end_ms: Some(4_900),
        local_target_speech_end_ms: Some(4_900),
        stable_attributed_speech_end_ms: Some(4_572),
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &newer_local_target_one_ms_before,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &newer_local_target_one_ms_before,
        true,
        1_000,
    ));

    let newer_local_target_exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(5_900),
        ..newer_local_target_one_ms_before
    };
    assert!(super::provider_stall_local_endpoint_due(
        &newer_local_target_exact_endpoint,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &newer_local_target_exact_endpoint,
        true,
        1_000,
    ));

    // Stalled provider with newer local target: cloud 4572, local target 4900,
    // provider frozen at 5700. Capture must reach local_target + 1000 ms.
    let installed_newer_target_stall = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(4_572),
        provider_audio_duration_ms: Some(5_700),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(5_200),
        local_target_speech_end_ms: Some(4_900),
        stable_attributed_speech_end_ms: Some(4_572),
        ..exact_endpoint
    };
    assert!(super::provider_stall_local_endpoint_due(
        &installed_newer_target_stall,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &installed_newer_target_stall,
        true,
        1_000,
    ));
}

#[test]
fn session_1378_uncertain_owner_tail_gets_bounded_resume_window() {
    // Installed session 1378: the owner was locally confirmed through 2100 ms,
    // later speech energy reached 3100 ms with only Uncertain classifications,
    // and the provider still attributed the same speaker through 3612 ms. The
    // old 1000 ms fallback stopped at local audio 5400 ms, cutting the resumed
    // half-sentence. Identity uncertainty gets 2000 ms, but confirmed other
    // speech keeps the normal 1000 ms endpoint.
    let uncertain_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_612),
        provider_audio_duration_ms: Some(4_100),
        audio_duration_ms: Some(5_400),
        local_speech_end_ms: Some(3_100),
        local_target_speech_end_ms: Some(2_100),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_612),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert_eq!(
        super::target_speaker_fusion_state(&uncertain_tail),
        super::TargetSpeakerFusionState::UncertainOwnerTail,
    );
    let timeout = super::target_speaker_endpoint_timeout_with_fusion(
        super::target_speaker_fusion_state(&uncertain_tail),
        1_000,
    );
    assert_eq!(timeout, 2_000);
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &uncertain_tail,
        true,
        timeout,
    ));

    let bounded_due = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(5_612),
        ..uncertain_tail.clone()
    };
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &bounded_due,
        true,
        timeout,
    ));

    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(3_100),
        ..uncertain_tail
    };
    assert_eq!(
        super::target_speaker_fusion_state(&confirmed_other),
        super::TargetSpeakerFusionState::ConfirmedOther,
    );
    assert_eq!(
        super::target_speaker_endpoint_timeout_with_fusion(
            super::target_speaker_fusion_state(&confirmed_other),
            1_000,
        ),
        1_000,
    );
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &confirmed_other,
        true,
        1_000,
    ));

    let provider_owner_advanced = crate::asr::volcengine::TargetSpeakerUpdate {
        target_activity_advanced: true,
        ..bounded_due
    };
    assert_eq!(
        super::target_speaker_fusion_state(&provider_owner_advanced),
        super::TargetSpeakerFusionState::OwnerContinuing,
        "cloud target progress is explicit owner-continuation evidence",
    );
}

#[test]
fn provider_stall_requires_real_time_without_provider_coverage_progress() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    let started = Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(4_572),
        provider_audio_duration_ms: Some(5_700),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(5_200),
        local_target_speech_end_ms: Some(4_900),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_572),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };

    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &update,
        started
    ));
    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &update,
        started + Duration::from_millis(999)
    ));
    assert!(super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &update,
        started + Duration::from_millis(1_000)
    ));

    let provider_advanced = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(5_701),
        ..update.clone()
    };
    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &provider_advanced,
        started + Duration::from_millis(1_001)
    ));
    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        new_session_id(),
        &update,
        started + Duration::from_secs(1)
    ));
}

#[test]
fn terminal_wake_continuation_is_bounded_session_matched_and_one_shot() {
    let coordinator = Coordinator::new();
    let started = Instant::now();
    let session_id = new_session_id();
    let wake_pcm = vec![7u8; 32_000];

    assert!(super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        wake_pcm.clone(),
        0.8,
        "开始录音".into(),
        started
    ));
    assert!(!super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        wake_pcm.clone(),
        0.8,
        "开始录音".into(),
        started + Duration::from_millis(1)
    ));
    assert!(super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        session_id,
        started + Duration::from_millis(2)
    ));
    assert!(coordinator
        .inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .is_some_and(|guard| guard.session_id == session_id));

    let continuation = super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        session_id,
        started + Duration::from_millis(3),
    )
    .expect("matching continuation");
    assert_eq!(continuation.wake_pcm, wake_pcm);
    assert_eq!(continuation.wake_phrase, "开始录音");
    assert!(super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        session_id,
        started + Duration::from_millis(4)
    )
    .is_none());
}

#[test]
fn terminal_wake_continuation_expiry_or_session_mismatch_cannot_leak() {
    let coordinator = Coordinator::new();
    let started = Instant::now();
    let session_id = new_session_id();
    assert!(super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        vec![1u8; 3_200],
        0.1,
        "开始录音".into(),
        started
    ));
    assert!(!super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        session_id,
        started + super::EMBEDDED_TERMINAL_WAKE_CONTINUATION_TTL
    ));
    assert!(coordinator
        .inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .is_none());

    let rebound_session_id = new_session_id();
    assert!(super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        vec![2u8; 3_200],
        0.1,
        "开始录音".into(),
        started + Duration::from_secs(7)
    ));
    assert!(super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        rebound_session_id,
        started + Duration::from_secs(7)
    ));
    assert!(super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        new_session_id(),
        started + Duration::from_secs(7)
    )
    .is_none());
    assert!(coordinator
        .inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .is_none());
    assert!(coordinator
        .inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .is_none());
}

#[test]
fn terminal_wake_body_guard_is_bound_before_recording_capsule_emit() {
    let source = include_str!("hotkey_device_runtime.rs");
    let start = source
        .find("async fn request_embedded_ble_recording_start_from_host")
        .expect("host start function");
    let body = &source[start..];
    let bind = body
        .find("bind_terminal_wake_continuation_session")
        .expect("terminal continuation bind");
    let emit = body
        .find("emit_capsule_for_session")
        .expect("recording capsule emit");
    assert!(bind < emit);
}

#[test]
fn target_speaker_endpoint_holds_after_one_transient_local_mismatch() {
    // Installed session 6ef0d4b8 reproduced this exact shape: cloud
    // diarization had stabilized only the wake phrase while the debounced local
    // identity still owned the continuing body through 5.2 seconds.
    let transient_mismatch = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(880),
        provider_audio_duration_ms: Some(5_500),
        audio_duration_ms: Some(5_900),
        local_speech_end_ms: Some(5_200),
        local_target_speech_end_ms: Some(5_200),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_482),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&transient_mismatch));

    // Target clock max(880, 4482, 5200)=5200; other-speaker energy does not
    // extend it. Exact due is 5200 + 1000 = 6200.
    let confirmed_other_speaker_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(6_200),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(5_600),
        local_non_target_speech_end_ms: Some(5_600),
        ..transient_mismatch
    };
    assert!(super::target_speaker_endpoint_due(
        &confirmed_other_speaker_tail
    ));
}

#[test]
fn target_speaker_endpoint_holds_during_owner_identity_recovery() {
    // Installed session 53a3f755 reproduced this shape. The provider had
    // already heard the continuing tail while the first recovering Target
    // window was still waiting for its second hysteresis confirmation.
    let first_recovering_target = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_152),
        provider_audio_duration_ms: Some(4_400),
        audio_duration_ms: Some(4_500),
        local_speech_end_ms: Some(4_500),
        local_target_speech_end_ms: Some(3_300),
        local_non_target_speech_end_ms: Some(4_100),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_152),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: true,
    };

    assert!(!super::target_speaker_endpoint_due(
        &first_recovering_target
    ));

    let owner_restored = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_600),
        audio_duration_ms: Some(4_900),
        local_speech_end_ms: Some(4_900),
        local_target_speech_end_ms: Some(4_900),
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        ..first_recovering_target
    };
    assert!(!super::target_speaker_endpoint_due(&owner_restored));
}

#[test]
fn target_speaker_endpoint_waits_for_startup_body_calibration() {
    let unresolved_body = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(1_442),
        provider_audio_duration_ms: Some(2_900),
        audio_duration_ms: Some(3_100),
        local_speech_end_ms: Some(3_100),
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(3_100),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(1_442),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&unresolved_body));

    // Once the provider has converged and there is still only confirmed
    // non-target body speech, the wake speaker's exact endpoint remains bounded.
    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(3_100),
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        ..unresolved_body
    };
    assert!(super::target_speaker_endpoint_due(&confirmed_other));
}

#[test]
fn incomplete_body_preview_uses_fifteen_hundred_ms_endpoint() {
    // Very short incomplete body holds longer (installed "那你" mid-cut).
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你帮")),
        2_500
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("那你")),
        2_500
    );
    assert_eq!(
        super::target_speaker_inactive_stop_reason(2_500),
        "target_speaker_inactive_2500ms"
    );
    // Longer incomplete body keeps 1.5s.
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你继续帮我看一下吧")),
        1_500
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你帮。")),
        1_000
    );
    // Empty / no body keeps base snappy; no-body abandon is layered separately.
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(None),
        1_000
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("   ")),
        1_000
    );
}

#[test]
fn host_started_wake_guard_survives_embedded_session_begin() {
    // terminal_wake_body_continuation / host start arms the wake guard before
    // BLE PCM attaches. begin_embedded_audio_dictation_session_id reuses the
    // Starting session; the guard for that session must not be wiped or the
    // no-body path falls back to snappy 1.0s.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Starting;
    }
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 0);
    assert!(automatic_wake_session_active(
        &coordinator.inner,
        session_id
    ));
    assert!(!automatic_wake_body_started(&coordinator.inner, session_id));

    // Mirror begin_embedded_audio_dictation_session preserve rule.
    if !automatic_wake_session_active(&coordinator.inner, session_id) {
        clear_automatic_wake_text_guard(&coordinator.inner);
    }
    assert!(
        automatic_wake_session_active(&coordinator.inner, session_id),
        "host-started wake guard must survive embedded session begin attach"
    );

    let mode_timeout = super::target_speaker_end_timeout_ms_for_preview(None);
    let no_body_timeout = if automatic_wake_body_started(&coordinator.inner, session_id) {
        mode_timeout
    } else if automatic_wake_session_active(&coordinator.inner, session_id) {
        super::EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS.max(mode_timeout)
    } else {
        mode_timeout
    };
    assert_eq!(no_body_timeout, 3_000);
    assert_eq!(
        super::target_speaker_inactive_stop_reason(no_body_timeout),
        "target_speaker_inactive_no_body_3000ms"
    );
}

#[test]
fn automatic_wake_no_body_uses_longer_endpoint_timeout() {
    // Session 72519330: after visible capsule, the 1.0s snappy
    // endpoint still measured the wake-phrase clock and empty-ended. Wake
    // sessions without body text must use the 3.0s abandon timeout; once body
    // starts, standard 1.0s returns.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);

    assert!(automatic_wake_session_active(
        &coordinator.inner,
        session_id
    ));
    assert!(!automatic_wake_body_started(&coordinator.inner, session_id));

    let mode_timeout = super::target_speaker_end_timeout_ms_for_preview(None);
    assert_eq!(mode_timeout, 1_000);
    let no_body_timeout = if automatic_wake_body_started(&coordinator.inner, session_id) {
        mode_timeout
    } else if automatic_wake_session_active(&coordinator.inner, session_id) {
        super::EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS.max(mode_timeout)
    } else {
        mode_timeout
    };
    assert_eq!(no_body_timeout, 3_000);
    assert_eq!(
        super::target_speaker_inactive_stop_reason(no_body_timeout),
        "target_speaker_inactive_no_body_3000ms"
    );

    // Wake-only target clock at 1332ms is not yet due at +1000 under no-body
    // timeout (would have been due under snappy 1.0s).
    let wake_only = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(1_332),
        provider_audio_duration_ms: Some(2_500),
        audio_duration_ms: Some(2_500),
        local_speech_end_ms: Some(2_500),
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(2_500),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(1_332),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(super::target_speaker_endpoint_due_with_timeout(
        &wake_only, 1_000
    ));
    assert!(!super::target_speaker_endpoint_due_with_timeout(
        &wake_only, 3_000
    ));
    let abandoned = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_400),
        audio_duration_ms: Some(4_400),
        ..wake_only
    };
    assert!(super::target_speaker_endpoint_due_with_timeout(
        &abandoned, 3_000
    ));

    // Body text latch restores snappy 1.0s.
    assert_eq!(
        filter_automatic_wake_text(
            &coordinator.inner,
            session_id,
            "开始录音。今天继续测试。",
            true,
        ),
        "今天继续测试。"
    );
    assert!(automatic_wake_body_started(&coordinator.inner, session_id));
    assert_eq!(
        super::target_speaker_inactive_stop_reason(1_000),
        "target_speaker_inactive_1000ms"
    );
}

#[test]
fn automatic_wake_preserves_first_clause_after_supported_body_pauses() {
    for pause_ms in [0_u64, 500, 1_000, 2_000, 2_500] {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
        acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);

        assert!(automatic_wake_initial_body_wait_active(
            &coordinator.inner,
            session_id,
            Some(1_200 + pause_ms),
        ));
        assert_eq!(
            filter_automatic_wake_text(
                &coordinator.inner,
                session_id,
                "开始录音。主讲人第一句内容要保持完整。最后这句话也不能丢。",
                true,
            ),
            "主讲人第一句内容要保持完整。最后这句话也不能丢。",
            "pause_ms={pause_ms}"
        );
        assert!(automatic_wake_body_started(&coordinator.inner, session_id));
        assert!(!automatic_wake_initial_body_wait_active(
            &coordinator.inner,
            session_id,
            Some(1_200 + pause_ms),
        ));
        clear_automatic_wake_text_guard(&coordinator.inner);
    }
}

#[test]
fn wake_only_expiry_is_handled_before_empty_transcript_failure_history() {
    let source = include_str!("dictation.rs");
    let branch_start = source
        .find("let wake_only_expired = automatic_wake_session_active")
        .expect("wake-only empty-result branch");
    let branch_tail = &source[branch_start..];
    let silent_return = branch_tail
        .find("return Ok(());")
        .expect("wake-only branch returns before generic failure");
    let empty_failure = branch_tail
        .find("error_code: Some(\"emptyTranscript\".to_string())")
        .expect("generic empty-transcript failure remains after wake-only handling");
    assert!(silent_return < empty_failure);
    assert!(branch_tail[..silent_return].contains("error_code: None"));
    assert!(branch_tail[..silent_return].contains("publish_embedded_ble_wake_only_expired"));
}

#[test]
fn automatic_wake_starts_initial_body_wait_at_visible_capsule_ack() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);

    // Before visible ack, wait is force-active (deadline not armed yet).
    assert!(automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        session_id,
        Some(1_200)
    ));
    assert_eq!(
        filter_automatic_wake_text(
            &coordinator.inner,
            session_id,
            "开始录音。今天继续测试。",
            true,
        ),
        "今天继续测试。"
    );
    // body_started=true; still force-active until visible ack arms/clears path.
    assert!(automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        session_id,
        Some(1_300)
    ));
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);
    // First non-empty body ends wait immediately after ack.
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        session_id,
        Some(1_300)
    ));

    let no_body_session_id = new_session_id();
    arm_automatic_wake_text_guard(
        &coordinator.inner,
        no_body_session_id,
        "开始录音".into(),
        1_200,
    );
    assert!(automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        no_body_session_id,
        Some(1_200)
    ));
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, no_body_session_id);
    // Deadline = capsule_audio 1200 + body wait 3000 = 4200.
    assert!(automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        no_body_session_id,
        Some(4_199)
    ));
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        no_body_session_id,
        Some(4_200)
    ));

    let manual_session_id = new_session_id();
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        manual_session_id,
        Some(0)
    ));
}

#[test]
fn long_ambient_wake_candidate_rotates_with_overlap_until_phrase_hit() {
    assert_eq!(
        super::rolling_kws_rotation_start(2_399 * 32, 0, false),
        None
    );
    assert_eq!(
        super::rolling_kws_rotation_start(2_400 * 32, 0, false),
        Some(1_000 * 32)
    );
    assert_eq!(
        super::rolling_kws_rotation_start(3_400 * 32, 1_000 * 32, false),
        Some(2_000 * 32)
    );
    assert_eq!(
        super::rolling_kws_rotation_start(4_500 * 32, 1_500 * 32, true),
        None
    );
}

#[test]
fn rolling_wake_match_keeps_absolute_candidate_boundary() {
    let found = crate::wake_phrase::Match {
        end_seconds: 0.75,
        matched_keyword: Some("开始录音".into()),
    };
    let adjusted = super::offset_streaming_wake_match(Some(found), 1_500 * 32)
        .expect("rolling detector match");
    assert!((adjusted.end_seconds - 2.25).abs() < f32::EPSILON);
}

#[test]
fn rolling_local_confirmation_restarts_the_800ms_ladder_per_window() {
    let origin = 1_000 * 32;
    assert!(super::should_advance_local_confirmation_window(
        true, false, 0, origin
    ));
    assert!(!super::should_advance_local_confirmation_window(
        true, true, 0, origin
    ));
    assert_eq!(
        super::local_confirmation_snapshot_for_window(1_799 * 32, origin, 0),
        None
    );
    assert_eq!(
        super::local_confirmation_snapshot_for_window(1_800 * 32, origin, 0),
        Some(800 * 32)
    );
}

#[test]
fn ambient_speech_bounds_each_window_but_never_disables_late_phrase_confirmation() {
    assert!(super::exploratory_local_confirmation_allowed(
        false, 0, 0, 0
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false, 2, 0, 2
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false, 3, 0, 3
    ));
    assert!(!super::exploratory_local_confirmation_allowed(
        false, 4, 0, 4
    ));
    // Every rolling window gets exactly one focused retry regardless of older
    // candidate-wide Absents; repeated work inside that window remains blocked.
    assert!(super::exploratory_local_confirmation_allowed(
        false,
        3,
        1_040 * 32,
        0
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false,
        4,
        1_040 * 32,
        0
    ));
    assert!(!super::exploratory_local_confirmation_allowed(
        false,
        4,
        1_040 * 32,
        1
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false,
        u8::MAX,
        2_040 * 32,
        0
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        true,
        u8::MAX,
        0,
        usize::MAX
    ));
}

#[test]
fn rolling_local_confirmation_discards_only_stale_exploratory_tasks() {
    let old_origin = 1_000 * 32;
    let current_origin = 2_000 * 32;
    assert!(super::local_confirmation_task_is_stale(
        old_origin,
        current_origin,
        false
    ));
    assert!(!super::local_confirmation_task_is_stale(
        current_origin,
        current_origin,
        false
    ));
    assert!(!super::local_confirmation_task_is_stale(
        old_origin,
        current_origin,
        true
    ));
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
        .consume_streaming_pcm(&coordinator.inner, &tail_pcm, None)
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
            .consume_streaming_pcm(&coordinator.inner, &packet, None)
            .expect("short voiced packet is accepted");
    }
    assert_eq!(consumer.bytes.load(Ordering::SeqCst), 0);
    assert_eq!(session.streaming_agc.voiced_chunks, 0);

    session
        .consume_streaming_pcm(&coordinator.inner, &packet, None)
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
        raw_input_level_percent: None,
        after_stop_boundary: false,
    };
    let after_stop = StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 1,
        pcm: vec![3, 4],
        raw_input_level_percent: None,
        after_stop_boundary: true,
    };

    assert!(embedded_streaming_chunk_is_asr_input(&before_stop));
    assert!(embedded_streaming_chunk_is_asr_input(&after_stop));
    session
        .consume_streaming_pcm(
            &coordinator.inner,
            &after_stop.pcm,
            after_stop.raw_input_level_percent,
        )
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
        raw_input_level_percent: None,
        after_stop_boundary: false,
    });
    let middle = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 17,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
        after_stop_boundary: false,
    });
    let sample = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 50,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
        after_stop_boundary: false,
    });
    let after_stop = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 51,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
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
    // No voiceprint enrolled for the configured phrase: phrase hit is enough.
    // The 1.1s owner window only applies to a phrase-bound owner template.
    assert!(!super::owner_verification_window_ready(
        super::OWNER_VERIFICATION_START_BYTES - 2,
        true,
    ));
    assert!(super::owner_verification_window_ready(
        super::OWNER_VERIFICATION_START_BYTES,
        true,
    ));
    assert!(super::owner_verification_window_ready(0, false));
    assert!(super::owner_verification_window_ready(
        super::OWNER_VERIFICATION_START_BYTES - 2,
        false,
    ));
    assert_eq!(super::next_owner_verification_retry_ms(1_100), Some(1_800));
    assert_eq!(super::next_owner_verification_retry_ms(1_800), Some(2_400));
    assert_eq!(super::next_owner_verification_retry_ms(2_400), None);
}

#[test]
fn local_confirmation_adds_context_with_a_strict_attempt_cap() {
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(0),
        Some(800 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(1),
        Some(1_400 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(2),
        Some(1_600 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(3),
        Some(2_000 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(4),
        Some(2_400 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(5),
        Some(3_000 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(6),
        Some(5_000 * 32)
    );
    assert_eq!(super::next_local_confirmation_snapshot_bytes(7), None);
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
        include_str!("dictation_embedded_stream.rs"),
        "
",
        include_str!("dictation_embedded_stream_completion.rs")
    );
    assert!(
        source.contains("crate::speaker_verification::is_enrolled_for_phrase(&phrase)")
            && source.contains("fn owner_verification_window_ready"),
        "no-voiceprint path must skip the owner speech window delay for the configured phrase"
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
    assert!(body.contains("detector.accept_pcm(&pcm_to_feed)"));
    assert!(body.contains("rolling_kws_rotation_start("));
    assert!(body.contains("STREAMING_KWS_ROTATE_OVERLAP_MS"));
    assert!(body.contains("candidate.kws_fed_bytes = candidate.pcm.len()"));
    assert!(body.contains("crate::speaker_verification::verify(&pcm, &voiceprint_phrase)"));
    assert!(
        body.find("detector.accept_pcm(&pcm_to_feed)")
            < body.find("crate::speaker_verification::verify(&pcm, &voiceprint_phrase)")
    );
    assert!(
        body.find("let recording_control_task")
            < body.find("begin_embedded_audio_dictation_session")
    );
    assert!(
        body.find("begin_embedded_audio_dictation_session")
            < body.find("recording_control_task.await")
    );
    assert!(body.contains("latency_target_ms=1000"));
    assert!(body.contains("latency_ceiling_ms=1200"));
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
    // Successful type/paste: silent capsule (text already on screen).
    assert_eq!(
        default_done_message(InsertStatus::PasteSent, false, false),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::PasteSent, false, true),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::Inserted, true, true),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::PasteSent, true, true),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::Inserted, false, false),
        None
    );
    let copied_message = if cfg!(target_os = "windows") {
        "润色不可用，已复制原文，请 Ctrl+V"
    } else {
        "润色不可用，已复制原文，请粘贴"
    };
    assert_eq!(
        default_done_message(InsertStatus::CopiedFallback, true, false),
        Some(copied_message.to_string())
    );
    assert_eq!(
        default_done_message(InsertStatus::Failed, true, false),
        Some("润色不可用，插入失败".to_string())
    );
    assert_eq!(
        default_done_message(InsertStatus::Failed, true, true),
        Some(if cfg!(target_os = "windows") {
            "上屏失败，内容在剪贴板，请 Ctrl+V".to_string()
        } else {
            "上屏失败，内容在剪贴板，请粘贴".to_string()
        })
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
            && polish.contains("KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS: usize = 700")
            && polish.contains("KWS_LOCAL_CONFIRM_RETRY_MS: usize = 400")
            && polish.contains("KWS_SECONDARY_CONFIRM_BUDGET_MS: u64 = 60")
            && polish.contains("KWS_SECONDARY_ABSENT_REJECT_COUNT: u8 = 2")
            && polish.contains("gain_normalized_pcm16"),
        "secondary budget 60ms + 2 Absent rejects + boosted (min 8x) local ASR"
    );
    assert_eq!(super::KWS_SECONDARY_CONFIRM_BUDGET_MS, 60);
    assert_eq!(super::KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS, 700);
    assert!(
        super::KWS_SECONDARY_CONFIRM_BUDGET_MS + 100 <= 350,
        "keyword fail-open plus actor/control allowance must fit the phrase-tail target"
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
fn pending_secondary_timeout_obeys_the_sixty_ms_latency_boundary() {
    use super::PendingSecondaryDecision::{
        AcceptKeywordModel, AwaitSecondary, HoldAfterExplicitAbsent,
    };

    assert_eq!(
        super::pending_secondary_decision(true, 59, 0),
        AwaitSecondary
    );
    assert_eq!(
        super::pending_secondary_decision(true, 60, 0),
        AcceptKeywordModel
    );
    assert_eq!(
        super::pending_secondary_decision(true, 60, 1),
        HoldAfterExplicitAbsent
    );
    assert_eq!(
        super::pending_secondary_decision(false, u64::MAX, 0),
        AwaitSecondary
    );
}

#[cfg(target_os = "windows")]
#[test]
fn secondary_budget_counts_pre_hit_confirmation_work_once() {
    assert_eq!(super::effective_secondary_waited_ms(0, 158), 158);
    assert_eq!(super::effective_secondary_waited_ms(36, 158), 158);
    assert_eq!(super::effective_secondary_waited_ms(100, 20), 100);
    assert_eq!(super::effective_secondary_waited_ms(99, 20), 99);
    assert_eq!(
        super::pending_secondary_decision(true, super::effective_secondary_waited_ms(0, 158), 0,),
        super::PendingSecondaryDecision::AcceptKeywordModel,
    );
}

#[cfg(target_os = "windows")]
#[test]
fn incomplete_pre_hit_absent_cannot_veto_a_later_keyword_hit() {
    use crate::wake_phrase::LocalPhraseRelation::Absent;

    assert_eq!(
        super::authoritative_local_absent_coverage(Absent, 3, 4, 0, 1_600 * 32),
        None,
        "a 3/4-character partial is HoldForMoreEvidence, not explicit Absent",
    );
    assert_eq!(
        super::authoritative_local_absent_coverage(Absent, 4, 4, 0, 800 * 32),
        Some(super::LocalConfirmationCoverage {
            start_bytes: 0,
            end_bytes: 800 * 32,
        }),
        "a full-length non-match remains authoritative anti-false-wake evidence",
    );
}

#[cfg(target_os = "windows")]
#[test]
fn pre_hit_absent_blocks_only_the_keyword_endpoint_it_already_covered() {
    let covered = super::LocalConfirmationCoverage {
        start_bytes: 0,
        end_bytes: 1_855 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(covered),
        1_055 * 32,
        1.815,
    ));
    assert!(!super::local_absent_covers_keyword_endpoint(
        Some(covered),
        2_035 * 32,
        2.795,
    ));

    let later_window = super::LocalConfirmationCoverage {
        start_bytes: 2_035 * 32,
        end_bytes: 3_435 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(later_window),
        2_035 * 32,
        2.795,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn session_643_full_absent_tail_conflict_blocks_keyword_timeout_fallback() {
    let covered = super::LocalConfirmationCoverage {
        start_bytes: 0,
        end_bytes: 839 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(covered),
        0,
        1.040,
    ));
    assert_eq!(
        super::pending_secondary_decision(true, 103, 1),
        super::PendingSecondaryDecision::HoldAfterExplicitAbsent,
        "an authoritative local non-match must not be reversed by the 60 ms KWS timeout",
    );

    let session_862_covered = super::LocalConfirmationCoverage {
        start_bytes: 1_010 * 32,
        end_bytes: 2_410 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(session_862_covered),
        1_010 * 32,
        2.690,
    ));

    let old_unrelated = super::LocalConfirmationCoverage {
        start_bytes: 0,
        end_bytes: 700 * 32,
    };
    assert!(!super::local_absent_covers_keyword_endpoint(
        Some(old_unrelated),
        0,
        1.040,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn completed_full_length_absent_blocks_fallback_without_rejecting_partial_phrase() {
    use crate::wake_phrase::LocalPhraseRelation;

    assert!(super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::Absent,
        4,
        4,
    ));
    assert!(super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::Absent,
        5,
        4,
    ));
    assert!(!super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::Absent,
        3,
        4,
    ));
    assert!(!super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::ExactStart,
        4,
        4,
    ));
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
fn terminal_offline_recall_stops_after_initial_plus_focused_absence() {
    assert!(!super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES - 2,
        0,
        false,
    ));
    assert!(super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        super::TERMINAL_OFFLINE_SKIP_ABSENT_COUNT - 1,
        false,
    ));
    assert!(!super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        super::TERMINAL_OFFLINE_SKIP_ABSENT_COUNT,
        false,
    ));
    assert!(super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        u8::MAX,
        true,
    ));

    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("Duration::from_millis(TERMINAL_OFFLINE_RECALL_BUDGET_MS)")
            && stream.contains("terminal offline recall released actor after bounded wait")
            && stream.contains("terminal skip offline cascade reason={}")
            && stream.contains("candidate.local_absent_count.saturating_add(1)"),
        "terminal offline recovery must remain bounded and repeated focused Absents must suppress hidden native work"
    );
    assert_eq!(super::TERMINAL_OFFLINE_RECALL_BUDGET_MS, 500);
}

#[cfg(target_os = "windows")]
#[test]
fn phonetic_near_match_requires_independent_kws_and_never_wakes_alone() {
    let near = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 3,
        phonetic_prefix_units: 0,
        phonetic_best_distance: 1,
        phonetic_best_window_start: 0,
        inference_ms: 100,
        snapshot_pcm_ms: 1_400,
        recovered_keyword_end_seconds: None,
    };
    assert!(super::phonetic_near_phrase_evidence(&near, 4));

    let too_far = super::LocalWakeConfirmation {
        phonetic_best_distance: 2,
        ..near
    };
    assert!(!super::phonetic_near_phrase_evidence(&too_far, 4));

    let too_short = super::LocalWakeConfirmation {
        transcript_chars: 2,
        ..near
    };
    assert!(!super::phonetic_near_phrase_evidence(&too_short, 4));

    use super::TerminalInflightLocalDecision::{AcceptLocal, PreserveKwsFusion, RecordAbsent};
    let exact = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 4,
        phonetic_prefix_units: 4,
        phonetic_best_distance: 0,
        phonetic_best_window_start: 0,
        inference_ms: 100,
        snapshot_pcm_ms: 1_400,
        recovered_keyword_end_seconds: None,
    };
    assert_eq!(
        super::terminal_inflight_local_decision(&exact, false, 4),
        AcceptLocal
    );

    let later = super::LocalWakeConfirmation {
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::PresentLater,
        ..exact
    };
    assert_eq!(
        super::terminal_inflight_local_decision(&later, false, 4),
        PreserveKwsFusion
    );
    assert_eq!(
        super::terminal_inflight_local_decision(&later, true, 4),
        AcceptLocal
    );
    assert_eq!(
        super::terminal_inflight_local_decision(&near, false, 4),
        PreserveKwsFusion
    );
    assert_eq!(
        super::terminal_inflight_local_decision(&too_far, false, 4),
        RecordAbsent
    );
}

#[test]
fn terminal_wait_budget_counts_time_already_spent_by_the_inflight_confirmation() {
    assert_eq!(super::terminal_inflight_confirmation_remaining_ms(0), 250);
    assert_eq!(super::terminal_inflight_confirmation_remaining_ms(43), 207);
    assert_eq!(super::terminal_inflight_confirmation_remaining_ms(250), 0);
    assert_eq!(super::terminal_inflight_confirmation_remaining_ms(999), 0);
}

#[test]
fn terminal_consumes_the_running_confirmation_before_applying_absent_skip() {
    let stream = include_str!("dictation_embedded_stream.rs");
    let terminal = stream
        .find("async fn finish_buffered_speaker_candidate")
        .expect("terminal candidate handler");
    let body = &stream[terminal..];
    let join = body
        .find("terminal joining in-flight local confirmation")
        .expect("terminal in-flight join");
    let skip = body
        .find("should_run_terminal_offline_recall(")
        .expect("terminal offline skip decision");
    assert!(
        join < skip,
        "an already-running final-window confirmation must finish before older Absent evidence can skip terminal recall"
    );
    assert!(body.contains("terminal_inflight_confirmation_remaining_ms(elapsed_ms)"));
    assert!(body.contains("terminal_completed_local_confirmation"));
}

#[test]
fn automatic_wake_discards_pre_wake_pcm_for_local_transcript() {
    // Preserve only a bounded wake-speaker anchor, never the long ambient prefix.
    assert_eq!(super::post_wake_pcm_offset_bytes(0.0, 32_000), 0);
    assert_eq!(
        super::post_wake_pcm_offset_bytes(10.24, 400_000),
        ((10.24_f32 + 0.12) * 32_000.0) as usize
    );
    assert_eq!(super::wake_speaker_anchor_pcm_offset_bytes(0.76, 64_000), 0);
    assert_eq!(
        super::wake_speaker_anchor_pcm_offset_bytes(3.255, 128_000),
        ((3.255_f32 * 32_000.0).round() as usize - 800 * 32) & !1usize
    );
    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("wake_speaker_anchor_pcm_offset_bytes(wake_match.end_seconds")
            && !stream.contains(
                "phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {\n                    ((wake_match.end_seconds"
            ),
        "LocalTranscript and KeywordModel must share the bounded wake-speaker anchor"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn exact_phrase_only_local_confirmation_refines_late_keyword_boundary() {
    let confirmation = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 4,
        phonetic_prefix_units: 4,
        phonetic_best_distance: 0,
        phonetic_best_window_start: 0,
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
        phonetic_prefix_units: 4,
        phonetic_best_distance: 0,
        phonetic_best_window_start: 0,
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

#[cfg(target_os = "windows")]
#[test]
fn completed_local_confirmation_does_not_run_redundant_boundary_kws() {
    let source = include_str!("dictation_wake_polish.rs");
    let start = source
        .find("fn spawn_local_wake_confirmation(")
        .expect("local confirmation task");
    let body = &source[start..];
    let end = body
        .find("\nstruct EmbeddedStreamingDictation")
        .expect("local confirmation task end");
    let body = &body[..end];

    assert!(body.contains("result.recovered_keyword_end_seconds = None"));
    assert!(
        !body.contains("crate::wake_phrase::detect("),
        "a completed local transcript must not pay another blocking KWS pass"
    );
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
fn embedded_pcm_host_path_never_boosts_quiet_firmware_pcm() {
    let pcm = pcm_from_samples(&vec![320i16; 1_600]);
    let (limited, stats) = normalize_embedded_pcm_for_asr(&pcm);

    assert_eq!(limited, pcm);
    assert_eq!(stats.gain, 1.0);
    assert_eq!(stats.rms_after, stats.rms_before);
    assert_eq!(stats.peak_after, stats.peak_before);
    assert_eq!(stats.upstream_clipped_samples, 0);
    assert_eq!(stats.clipped_samples, 0);
}

#[test]
fn embedded_pcm_host_limiter_flags_upstream_clip_without_creating_one() {
    let mut samples = vec![30_000i16; 1_600];
    samples[0] = i16::MAX;
    samples[1] = i16::MIN;
    let pcm = pcm_from_samples(&samples);
    let (limited, stats) = normalize_embedded_pcm_for_asr(&pcm);
    let (_, peak_after) = embedded_pcm_rms_and_peak(&limited);

    assert_eq!(limited.len(), pcm.len());
    assert!(stats.gain < 1.0);
    assert!(stats.limiter_reduction_db > 0.0);
    assert_eq!(stats.upstream_clipped_samples, 2);
    assert_eq!(stats.clipped_samples, 0);
    assert!(peak_after as f64 <= EMBEDDED_AUDIO_HOST_LIMITER_PEAK.ceil());
}

#[test]
fn volcengine_streaming_path_is_attenuation_only_and_unbuffered() {
    let quiet = pcm_from_samples(&vec![12i16; 320]);
    let voice = pcm_from_samples(&vec![320i16; 320]);
    let mut state = EmbeddedStreamingAgcState::default();

    let (quiet_out, quiet_stats) = normalize_embedded_streaming_pcm_for_asr(&quiet, &mut state);
    let (voice_out, voice_stats) = normalize_embedded_streaming_pcm_for_asr(&voice, &mut state);

    assert_eq!(quiet_out, quiet);
    assert_eq!(voice_out, voice);
    assert_eq!(quiet_stats.gain, 1.0);
    assert_eq!(voice_stats.gain, 1.0);
    assert_eq!(state.quiet_chunks, 1);
    assert_eq!(state.voiced_chunks, 1);
    assert_eq!(state.gain_update_count, 0);
    assert_eq!(state.clipped_samples, 0);
}

#[test]
fn volcengine_streaming_limiter_does_not_poison_later_blocks() {
    let hot = pcm_from_samples(&vec![30_000i16; 1_600]);
    let ordinary = pcm_from_samples(&vec![3_000i16; 1_600]);
    let mut state = EmbeddedStreamingAgcState::default();

    let (_, hot_stats) = normalize_embedded_streaming_pcm_for_asr(&hot, &mut state);
    let (ordinary_out, ordinary_stats) =
        normalize_embedded_streaming_pcm_for_asr(&ordinary, &mut state);

    assert!(hot_stats.gain < 1.0);
    assert_eq!(hot_stats.clipped_samples, 0);
    assert_eq!(ordinary_stats.gain, 1.0);
    assert_eq!(ordinary_out, ordinary);
    assert_eq!(state.gain_update_count, 1);
    assert!(state.limiter_reduction_db_max > 0.0);
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

#[test]
fn windows_volcengine_session_refreshes_external_credential_updates_before_reading() {
    let source = include_str!("../coordinator.rs");
    let reader = source
        .split("fn read_volc_credentials()")
        .nth(1)
        .expect("Volcengine credential reader must exist");
    let refresh = reader
        .find("CredentialsVault::refresh_from_system()")
        .expect("Windows sessions must refresh the OS credential document");
    let first_read = reader
        .find("CredentialsVault::get(CredentialAccount::VolcengineAppKey)")
        .expect("Volcengine App ID read must exist");
    assert!(
        refresh < first_read,
        "the external vault refresh must happen before any cached Volcengine field is read"
    );
}

#[test]
fn every_automatic_wake_path_seeds_session_speaker_tracking() {
    let body = concat!(
        include_str!("dictation_embedded_stream_session.rs"),
        "\n",
        include_str!("dictation_embedded_stream.rs")
    );
    assert_eq!(
        body.matches("start_local_speaker_tracking(").count(),
        3,
        "live, terminal-with-body, and terminal-continuation wake paths must bind endpointing to the wake speaker"
    );

    let continuation_take = body
        .find("take_terminal_wake_continuation(inner, session.session_id)")
        .expect("terminal continuation must attach to the next coordinator session");
    let continuation_seed = body[continuation_take..]
        .find("start_local_speaker_tracking(")
        .map(|offset| continuation_take + offset)
        .expect("terminal continuation must seed target-speaker tracking");
    let continuation_activate = body[continuation_seed..]
        .find("activate_embedded_audio_dictation_session")
        .map(|offset| continuation_seed + offset)
        .expect("terminal continuation must activate its bound session");
    assert!(continuation_take < continuation_seed && continuation_seed < continuation_activate);

    let live_accept = body
        .find("live automatic session activated and released")
        .expect("live automatic wake path must remain present");
    let live_seed = body[..live_accept]
        .rfind("start_local_speaker_tracking(")
        .expect("live automatic wake path must seed target-speaker tracking");
    let live_session = body[..live_accept]
        .rfind("begin_embedded_audio_dictation_session(inner).await?")
        .expect("live automatic wake path must create a dictation session");
    assert!(live_session < live_seed && live_seed < live_accept);
}

#[tokio::test]
async fn activation_segment_race_rebinds_post_activation_segment_instead_of_finalizing() {
    // 2026-08-09 12:46:59 复现 fixture：VREC:ACTIVATE 后 0.1s 旧唤醒段（93）
    // complete，仅含 1845ms 唤醒词——竞态窗口内旧段 STOP 不得 finalize（否则
    // 必空稿）；听写会话必须绑定激活后的新设备段（94），其 PCM 正常进 ASR。
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(93);
    streaming.session = Some(embedded_audio_test_session(
        session_id,
        consumer_for_session,
    ));
    streaming.activation_segment_race_guard = Some((93, Instant::now()));

    // 旧段在竞态窗口内 STOP：不 finalize，会话保持打开，守卫保留等待新段。
    let handled = streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Stopped {
                session_id: 93,
                expected_packet_count: 58,
                origin: crate::embedded_audio::SessionStopOrigin::VoiceActivation,
            },
        )
        .await
        .expect("pre-activation stop handled");
    assert!(
        !handled,
        "pre-activation segment rotation is pending, not a completed dictation"
    );
    assert!(
        streaming.session.is_some(),
        "dictation session must stay open across the pre-activation segment stop"
    );
    assert_eq!(
        streaming.activation_segment_race_guard.map(|guard| guard.0),
        Some(93),
        "race guard stays armed until the post-activation segment binds"
    );
    assert_eq!(streaming.embedded_session_id, None);
    assert_eq!(streaming.pending_stop_expected_packet_count, None);
    assert!(!streaming.terminal_received);

    // 同段尾包（极端时序）仍正常喂入——正常路径里正文就在激活后的同段延续。
    let wake_tail = pcm_from_samples(&samples_for_ms(100, 3_000));
    streaming.activation_segment_race_guard = Some((93, Instant::now()));
    streaming.embedded_session_id = Some(93);
    streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 93,
                packet_sequence: 12,
                pcm: wake_tail.clone(),
                raw_input_level_percent: Some(20),
                after_stop_boundary: false,
            }),
        )
        .await
        .expect("same-segment tail handled");
    assert_eq!(
        consumer.bytes.load(Ordering::SeqCst),
        wake_tail.len(),
        "same-segment PCM after activation must still feed ASR (normal path body)"
    );
    assert_eq!(
        streaming.activation_segment_race_guard.map(|guard| guard.0),
        Some(93),
        "same-segment PCM must not clear the race guard"
    );

    // 激活后的新设备段（94，唤醒监听窗 rotation）Started：直接绑定听写会话。
    streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Started {
                session_id: 94,
                origin: crate::embedded_audio::SessionStartOrigin::VoiceActivation,
            },
        )
        .await
        .expect("post-activation segment start handled");
    assert_eq!(streaming.embedded_session_id, Some(94));
    assert!(streaming.activation_segment_race_guard.is_none());
    assert!(streaming.session.is_some());

    // 新段正文 PCM 正常进 ASR。
    let body = pcm_from_samples(&samples_for_ms(200, 2_500));
    streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 94,
                packet_sequence: 0,
                pcm: body.clone(),
                raw_input_level_percent: Some(33),
                after_stop_boundary: false,
            }),
        )
        .await
        .expect("post-activation body handled");
    assert_eq!(
        consumer.bytes.load(Ordering::SeqCst),
        wake_tail.len() + body.len(),
        "post-activation segment body must reach ASR"
    );
}

#[tokio::test]
async fn activation_segment_race_guard_does_not_break_normal_stop_paths() {
    // 守卫不得改变正常路径：窗口外（>2s）或正文已开始时，旧段 STOP 照常走
    // pending-stop/finalize 流程。
    for (guard_age, with_body, label) in [
        (Some(Duration::from_secs(3)), false, "race window expired"),
        (None, true, "body already started"),
    ] {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
        let consumer = Arc::new(CountingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut streaming = EmbeddedStreamingDictation::background_listener();
        streaming.embedded_session_id = Some(93);
        streaming.session = Some(embedded_audio_test_session(
            session_id,
            consumer_for_session,
        ));
        let activated_at = guard_age
            .map(|age| Instant::now() - age)
            .unwrap_or_else(Instant::now);
        streaming.activation_segment_race_guard = Some((93, activated_at));
        if with_body {
            update_embedded_audio_partial_preview(&coordinator.inner, session_id, "正文".into());
        }

        let handled = streaming
            .handle_ble_packet_actor_command(
                &coordinator.inner,
                StreamingSessionEvent::Stopped {
                    session_id: 93,
                    expected_packet_count: 58,
                    origin: crate::embedded_audio::SessionStopOrigin::VoiceActivation,
                },
            )
            .await
            .expect("stop handled");
        assert!(!handled, "{label}: normal stop path keeps waiting for tail");
        assert!(
            streaming.activation_segment_race_guard.is_none(),
            "{label}: race guard disarms once the normal path takes over"
        );
        assert_eq!(
            streaming.pending_stop_expected_packet_count,
            Some(58),
            "{label}: normal pending-stop flow armed"
        );
        assert!(streaming.session.is_some(), "{label}: session untouched");
    }
}

#[test]
fn unresolved_local_speech_hold_is_capped_two_seconds_after_confirmed_owner() {
    // F4 fixture（2026-08-09 12:47:04）：旁人连续说话，本地未归属人声持续推进，
    // 旧逻辑会把自动结束无限挂起。挂起以最后一次确认本人语音 +2s 封顶。
    let base = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(10_000),
        provider_audio_duration_ms: Some(20_000),
        audio_duration_ms: Some(20_000),
        local_speech_end_ms: Some(19_900),
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(10_000),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    // cap 内（本人 10s + 2s = 12s；未归属人声尾端 11s、且仍在 1.0s 窗口内 recent）
    // → 仍阻挡结束（本人说话分类滞后不受影响的通道保留）。
    let within_cap = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(11_500),
        audio_duration_ms: Some(11_500),
        local_speech_end_ms: Some(11_000),
        ..base.clone()
    };
    assert!(super::has_unresolved_recent_local_speech(
        &within_cap,
        1_000
    ));
    assert!(!super::target_speaker_endpoint_due(&within_cap));
    // cap 外（未归属人声尾端推进到 19.9s > 12s 封顶）→ 不再阻挡，端点可按
    // 1.0s 合同触发。
    assert!(!super::has_unresolved_recent_local_speech(&base, 1_000));
    assert!(super::target_speaker_endpoint_due(&base));

    // Installed session 1494: owner ended locally at 10.9s; provider later
    // attributed room speech through 12.712s and local energy reached 13.8s.
    // The unrelated attribution must not renew the uncertain-owner allowance.
    let installed_session_1494 = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(10_322),
        provider_audio_duration_ms: Some(13_700),
        audio_duration_ms: Some(13_800),
        local_speech_end_ms: Some(13_800),
        local_target_speech_end_ms: Some(10_900),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(12_712),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::has_unresolved_recent_local_speech(
        &installed_session_1494,
        1_000
    ));
    assert!(super::target_speaker_endpoint_due(&installed_session_1494));
}

#[test]
fn explicit_wake_diagnostic_sequence_stops_at_retention_limit() {
    assert_eq!(
        super::next_wake_diagnostic_capture_count(false, super::WAKE_DIAGNOSTIC_MAX_CANDIDATES),
        None,
        "an explicit operator-managed capture session must remain bounded"
    );
}

#[test]
fn production_default_resolves_zero_wake_diagnostic_targets_for_one_hundred_candidates() {
    for _ in 0..100 {
        assert!(super::explicit_wake_diagnostic_directory(None).is_none());
    }
    assert!(super::explicit_wake_diagnostic_directory(Some("   ".into())).is_none());
    assert_eq!(
        super::explicit_wake_diagnostic_directory(Some("D:\\listener-wake-diag".into())),
        Some(std::path::PathBuf::from("D:\\listener-wake-diag"))
    );
}
