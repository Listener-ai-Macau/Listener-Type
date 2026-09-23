use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
// A coordinator input stream identity is intentionally independent from the
// capture generation.  A generation describes the capture lifecycle; this ID
// describes the logical PCM producer whose offsets may be reused from zero.
static NEXT_EMBEDDED_COORDINATOR_SOURCE_STREAM_ID: AtomicU64 = AtomicU64::new(1);

fn next_embedded_coordinator_source_stream_id() -> u64 {
    NEXT_EMBEDDED_COORDINATOR_SOURCE_STREAM_ID.fetch_add(1, Ordering::Relaxed)
}

// Seeded lazily from the wall clock (never 0 = unseeded).  The firmware keeps
// a boot-scoped ensure-request watermark and rejects any `request_id` below it
// as stale; the watermark reset only runs on a fresh TYPE:READY host sync,
// which the reconnect-over-existing-connection path never triggers.  A process
// counter starting at 1 therefore loses against the previous Type process's
// watermark and the wake-capture lease renewal is silently dropped — the device
// then auto-stops its hidden window at max_duration mid-body (r46e: 15 chars
// sealed, serial `ensure rejected ... watermark=E reason=stale_request`).
// Wall-clock seconds are monotonic across app restarts, so a restarted Type
// process always allocates ids above the device's watermark.
static NEXT_ACCEPTED_WAKE_CAPTURE_REQUEST_ID: AtomicU64 = AtomicU64::new(0);

fn next_accepted_wake_capture_request_id() -> u32 {
    if NEXT_ACCEPTED_WAKE_CAPTURE_REQUEST_ID.load(Ordering::Relaxed) == 0 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1)
            .clamp(1, 0x7FFF_FFFF) as u64;
        NEXT_ACCEPTED_WAKE_CAPTURE_REQUEST_ID
            .compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed)
            .ok();
    }
    loop {
        let id = NEXT_ACCEPTED_WAKE_CAPTURE_REQUEST_ID.fetch_add(1, Ordering::Relaxed) as u32;
        if id != 0 {
            return id;
        }
    }
}
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
// 2026-09-20: 128 per-process stage2 captures exhausted within one active
// debugging day (live incident 2274297663 hit index 127), blinding exactly the
// evenings that need forensics. 2026-09-21: 512 still died in ~3h of heavy
// morning use (index 511 at 09:41, the 10:12-10:16 wake incidents had no WAV
// forensics at all). Diagnostics-only caps; no product behavior.
const WAKE_DIAGNOSTIC_MAX_CANDIDATES: usize = 2048;
const WAKE_DIAGNOSTIC_MAX_PCM_BYTES: usize = 12 * 16_000 * 2;
const WAKE_DIAGNOSTIC_RETENTION_MAX_FILES: usize = 2048;
const WAKE_DIAGNOSTIC_RETENTION_MAX_BYTES: u64 = 512 * 1024 * 1024;
const WAKE_DIAGNOSTIC_RETENTION_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const POST_DICTATION_KEY_DELAY: Duration = Duration::from_millis(60);
const EMBEDDED_ASR_SPEECH_ACTIVITY_TIMEOUT: Duration = Duration::from_millis(300);
const EMBEDDED_ASR_SPEECH_ACTIVITY_MAX_QUEUE_AGE: Duration = Duration::from_millis(600);
// Owner dictation endpoint. Once body speech has started, every preview shape
// uses the established inactivity contract. Do not make completion depend
// on optimistic punctuation, body length, or an uncertain voiceprint vote: the
// installed 1.0.5 ladder (1.5/2.0/2.5s) made the same spoken ending complete at
// different speeds. Only the wake/target speaker's latest speech refreshes this
// clock, so other people talking still cannot lengthen auto-end.
// 2026-09-23 15:48 续接掐断实锤 + 用户拍板 3s：停顿落屏(tjs)+early-seal(tjp)
// 上线后，文字在停顿稳定窗即落屏，会话关停速度不影响上屏跟手——1.0s 老契约
// "防完成变慢"的理由失效，而 1s 关停把 2-4s 思考停顿的续接整段掐死
// (target_speaker_inactive 1s 后本人续说被当新候选拒掉)。用户明确要求
// "窗口改到 3 秒，3 秒内可以续上，反正上屏很快不用硬等"——放宽到 3.0s；
// 固件 1s 静默兜底由 should_keep_firmware_alive_for_host_hang 跟随本常量
// 自动续租。
const EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS: u64 = 3_000;
// A settled-text timer shares the async runtime with BLE and ASR callbacks.
// Arm it slightly before the public endpoint so ordinary Windows scheduling
// jitter still dispatches at about the public deadline (live 577 measured
// 1,141 ms from a nominal 1,000 ms timer; live 578 lost the race to firmware).
// The stop reason and provider/audio-clock policy remain the endpoint
// contract; only this wall-clock wake-up receives the scheduling allowance
// (see EMBEDDED_SETTLED_TARGET_SCHEDULING_ALLOWANCE_MS, derived below).
const EMBEDDED_SETTLED_TARGET_WALL_CLOCK_MS: u64 = 2_900;
// Installed session 72519330: wake capsule → ~1.2s host auto-end on the wake
// clock with empty body → "没有识别到语音". Initial body wait is only 700ms, so
// 1.0s snappy endpoint after that treats "thinking after wake" as done. Keep
// 1.0s once body text exists; before body, require a longer abandon silence.
const EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS: u64 = 8_000;
// Total budget for endpoint-continuation evidence that is not POSITIVELY
// owner-attributed.  Positive evidence = local qualified/Target watermark
// advance or visible preview growth.  Cloud target activity and unclassified
// local speech edges do not count: a settled session in a room whose energy
// detector keeps tripping (breathing, fans) plus a cloud row that keeps
// absorbing that noise into the target id fed each other and auto-end never
// fired (2026-09-19 17:4x "不能自动结束" sessions: rearm
// reason=provider_activity_advanced every ~1.7 s, local_speech_end_ms pinned
// to the live edge, classification=None, 3 of 5 sessions needed a manual
// stop).  Once this budget expires, unclassified/cloud evidence stops
// re-arming and holding the endpoint; the next positive evidence re-opens it
// immediately.  The budget deliberately exceeds the open-clause continuation
// window (3.5 s since the 2026-09-23 3 s endpoint contract) so a deliberate
// mid-sentence pause still survives — a budget that expires first would
// cancel the continuation hold and fire the endpoint early (open-clause test
// caught exactly this ordering when both moved to the 3 s scale).
const EMBEDDED_OWNER_POSITIVE_EVIDENCE_BUDGET_MS: u64 = 4_000;
const EMBEDDED_PROVIDER_STALL_FALLBACK_LAG_MS: u64 = 500;
// 500ms confirmed stalls still mid-cut live speech when the cloud clock freezes
// for one network blip. Require a full second of no provider coverage growth
// before local-clock fallback may end the session.
const EMBEDDED_PROVIDER_STALL_CONFIRM_MS: u64 = 1_000;
// The warm Paraformer helper measured 341 ms on a 6 s Chinese overlap sample.
// Start it in parallel with cloud finalization and give the whole shadow path a
// bounded budget, so omission recovery cannot bring back the old multi-second
// Done delay. Long dictations remain cloud-authoritative.
const LOCAL_SHADOW_ASR_MIN_AUDIO_MS: usize = 2_000;
const LOCAL_SHADOW_ASR_MAX_AUDIO_MS: usize = 20_000;
const LOCAL_SHADOW_ASR_HELPER_TIMEOUT_MS: u64 = 800;
const LOCAL_SHADOW_ASR_TOTAL_BUDGET_MS: u64 = 850;
// A provider may stabilize an old utterance several seconds after the owner
// stopped. Treating that late bookkeeping update as live speech refreshes the
// firmware's two-second safety timer and makes completion feel randomly slow.
// Real preview growth refreshes the local owner clock at the current capture
// edge, so a 600 ms alignment window retains live keepalives without allowing
// a stale two-pass boundary to extend recording.
const EMBEDDED_LIVE_OWNER_ACTIVITY_ALIGNMENT_MS: u64 = 600;

include!("dictation_endpoint_clock.rs");
include!("dictation_endpoint_policy.rs");
include!("dictation_local_speech_activity.rs");

fn target_speaker_inactive_stop_reason(timeout_ms: u64) -> &'static str {
    // Tier boundaries must stay ordered: no-body > dangling continuation >
    // ordinary endpoint. With the ordinary endpoint at 3.0 s and the dangling
    // tier above it, comparing against the ordinary constant keeps each label
    // exact instead of letting the >= chain swallow the higher tier.
    if timeout_ms >= EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS {
        "target_speaker_inactive_no_body_8000ms"
    } else if timeout_ms > EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS {
        "target_speaker_inactive_3500ms"
    } else {
        "target_speaker_inactive_3000ms"
    }
}
// The wake phrase is a complete activation command: after the capsule becomes
// visible, give the owner a full eight seconds to begin the body (2026-09-22
// 16:47 实锤：用户说完唤醒词想词 13s，3s 宽限把胶囊掐死，体验成"唤醒不行"；
// 对齐 Siri 的想词宽限）。The first non-empty body preview ends this wait
// immediately, after which the 3000 ms owner-inactivity endpoint
// (EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS) applies.
const EMBEDDED_AUTOMATIC_BODY_INITIAL_WAIT_MS: u64 = 8_000;
const EMBEDDED_TERMINAL_WAKE_CONTINUATION_TTL: Duration = Duration::from_secs(6);
const EMBEDDED_ACCEPTED_WAKE_CAPTURE_REPLACEMENT_TIMEOUT: Duration = Duration::from_secs(3);
// Firmware uses the high bit of SessionStartOrigin to carry the low 15 bits
// of the accepted ENSURE request id.  This is a transport receipt, not a
// wake/speaker decision; a continuation is admitted only when this marker
// matches the outstanding request exactly.
const EMBEDDED_ENSURE_START_ORIGIN_MARKER_MASK: u16 = 0x8000;
const EMBEDDED_ENSURE_START_ORIGIN_REQUEST_MASK: u16 = 0x7fff;

fn embedded_ensure_start_origin_marker(request_id: u32) -> u16 {
    EMBEDDED_ENSURE_START_ORIGIN_MARKER_MASK
        | ((request_id as u16) & EMBEDDED_ENSURE_START_ORIGIN_REQUEST_MASK)
}

fn embedded_ensure_start_origin_matches(
    origin: crate::embedded_audio::SessionStartOrigin,
    request_id: u32,
) -> bool {
    matches!(
        origin,
        crate::embedded_audio::SessionStartOrigin::Unknown(value)
            if value == embedded_ensure_start_origin_marker(request_id)
    )
}

const EMBEDDED_LOCAL_SPEECH_ALIGNMENT_SLACK_MS: u64 = 200;
// The local speaker verifier runs on overlapping windows and reports roughly
// every 400 ms. Its classified audio edge therefore legitimately trails the
// newest VAD speech edge by one cadence. Installed multi-speaker session 1831
// had a confirmed NonTarget edge at 13.9 s while the live speech clock was
// already near 14.3 s; the former 100 ms allowance treated that sustained room
// speaker as unclassified owner speech and disabled provider-stall auto-end.
// Keep this below two verifier cadences so stale evidence still expires.
const EMBEDDED_LOCAL_SPEAKER_CLASSIFICATION_SLACK_MS: u64 = 600;
// Do not let one borderline low-score window release a provisional provider
// tail.  Require the confirmed local other-speaker edge to remain at least one
// full endpoint interval beyond the last owner boundary; this preserves the
// owner-recovery guard while handling a cloud diarizer that merged both voices.
const EMBEDDED_CONFIRMED_NON_TARGET_OWNER_GAP_MS: u64 = 1_000;
// F4（2026-08-09 12:47:04）：旁人连续说话时未归属本地语音不断前进，会把
// 自动结束无限挂起。挂起以最后一次归属语音 +6s 封顶；本人正常说话的分类
// 滞后远小于 6s，不受影响。
const EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS: u64 = 2_000;

struct EmbeddedAsrSpeechActivityJob {
    inner: Weak<Inner>,
    session_id: SessionId,
    coalesced: bool,
    queued_at: Instant,
}

struct LatestSpeechActivityQueue<T> {
    in_flight: bool,
    pending: Option<T>,
}

impl<T> Default for LatestSpeechActivityQueue<T> {
    fn default() -> Self {
        Self {
            in_flight: false,
            pending: None,
        }
    }
}

impl<T> LatestSpeechActivityQueue<T> {
    /// Retain the newest refresh while one BLE write is in flight. Returning
    /// `true` transfers ownership of draining the queue to the caller.
    fn enqueue(&mut self, job: T) -> bool {
        self.pending = Some(job);
        if self.in_flight {
            false
        } else {
            self.in_flight = true;
            true
        }
    }

    /// Called only by the active drain worker. Clearing `in_flight` under the
    /// same lock makes enqueue-vs-finish race-free.
    fn take_pending_or_finish(&mut self) -> Option<T> {
        match self.pending.take() {
            Some(job) => Some(job),
            None => {
                self.in_flight = false;
                None
            }
        }
    }
}

fn embedded_asr_speech_activity_queue(
) -> &'static Mutex<LatestSpeechActivityQueue<EmbeddedAsrSpeechActivityJob>> {
    static QUEUE: OnceLock<Mutex<LatestSpeechActivityQueue<EmbeddedAsrSpeechActivityJob>>> =
        OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(LatestSpeechActivityQueue::default()))
}

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

include!("dictation_asr_selection.rs");

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
    if !active {
        return;
    }

    let should_spawn = {
        let mut queue = embedded_asr_speech_activity_queue().lock();
        let coalesced = queue.in_flight;
        queue.enqueue(EmbeddedAsrSpeechActivityJob {
            inner: Arc::downgrade(inner),
            session_id,
            coalesced,
            queued_at: Instant::now(),
        })
    };
    if !should_spawn {
        return;
    }

    async_runtime::spawn_blocking(move || loop {
        let Some(job) = embedded_asr_speech_activity_queue()
            .lock()
            .take_pending_or_finish()
        else {
            break;
        };
        let Some(inner) = job.inner.upgrade() else {
            continue;
        };
        if job.queued_at.elapsed() >= EMBEDDED_ASR_SPEECH_ACTIVITY_MAX_QUEUE_AGE {
            log::warn!(
                "[embedded-ble] discarded stale coalesced speech refresh session_id={} queued_ms={}",
                job.session_id,
                job.queued_at.elapsed().as_millis()
            );
            continue;
        }
        let session_still_active = {
            let state = inner.state.lock();
            state.session_id == job.session_id
                && !state.cancelled
                && matches!(
                    state.phase,
                    SessionPhase::Starting | SessionPhase::Listening
                )
        };
        let still_active = session_still_active
            && embedded_ble_host_recording_control_context_active(&inner)
            && !embedded_audio_stop_feedback_latched(&inner);
        if !still_active {
            continue;
        }

        let started = Instant::now();
        let result = crate::embedded_ble::send_recording_control_speech_activity(
            EMBEDDED_ASR_SPEECH_ACTIVITY_TIMEOUT,
        );
        match result {
            Ok(()) if job.coalesced => log::info!(
                "[embedded-ble] coalesced recognized speech refresh delivered session_id={} elapsed_ms={}",
                job.session_id,
                started.elapsed().as_millis()
            ),
            Ok(()) => log::debug!(
                "[embedded-ble] recognized speech refreshed auto-stop timeout session_id={} elapsed_ms={}",
                job.session_id,
                started.elapsed().as_millis()
            ),
            Err(err) => log::warn!(
                "[embedded-ble] recognized speech refresh failed session_id={}: {err}",
                job.session_id
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

include!("dictation_target_speaker_update.rs");

fn qualified_owner_speech_end_ms(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> Option<u64> {
    update.qualified_owner_speech_end_ms
}

/// A qualified watermark is valid only when the latest classifier summary is
/// itself a usable Target observation for that same audio interval. A retained
/// Target label must not be applied to newer raw-energy chunks or a late cloud
/// callback.
fn has_current_qualified_owner_observation(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> bool {
    if !update.local_speaker_tracking_enabled
        || update.local_speaker_classification_kind
            != Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target)
        || update.local_speaker_signal_quality_sufficient != Some(true)
    {
        return false;
    }
    let Some(qualified_ms) = qualified_owner_speech_end_ms(update) else {
        return false;
    };
    let Some(observation_ms) = update.local_speaker_observation_end_ms else {
        return false;
    };
    qualified_ms == observation_ms
        && update
            .local_speech_end_ms
            .is_none_or(|raw_ms| raw_ms <= observation_ms)
}

fn endpoint_owner_watermark(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> Option<u64> {
    if update.local_speaker_tracking_enabled {
        qualified_owner_speech_end_ms(update).or(update.target_speech_end_ms)
    } else {
        qualified_owner_speech_end_ms(update)
            .or(update.local_target_speech_end_ms)
            .or(update.target_speech_end_ms)
    }
}

/// The latest local window positively classified the newest speech as the
/// target speaker with sufficient signal quality, and its unqualified Target
/// edge is still aligned with the live speech edge. The quality-qualified
/// watermark trails the audio by at most one verifier cadence in that state,
/// so this edge is owner evidence for endpointing (installed sessions 36-1378
/// / be0c3e6e: the owner kept talking while the qualified watermark lagged).
/// An Uncertain or low-quality window never qualifies, and a Target edge that
/// the speech energy has long outrun is stale, not current.
fn local_target_edge_is_fresh_target_observation(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> Option<u64> {
    if !update.local_speaker_tracking_enabled
        || update.local_speaker_classification_kind
            != Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target)
        || update.local_speaker_signal_quality_sufficient != Some(true)
    {
        return None;
    }
    update
        .local_target_speech_end_ms
        .zip(update.local_speech_end_ms)
        .and_then(|(target_ms, speech_ms)| {
            (target_ms.abs_diff(speech_ms) <= EMBEDDED_LIVE_OWNER_ACTIVITY_ALIGNMENT_MS
                && !local_speech_confidently_non_target(update, speech_ms))
                .then_some(target_ms.max(speech_ms))
        })
}

/// Select the identity-authoritative owner boundary for endpointing.
///
/// Cloud diarization is useful until the enrolled local verifier has observed
/// the owner. After that point the qualified local Target watermark owns the endpoint:
/// a provider speaker row can merge a nearby second speaker into the owner's
/// row and must not move the stop deadline. Raw energy, Uncertain and NonTarget windows
/// do not advance this watermark, while a later positive Target window can
/// still rearm it normally.
fn authoritative_owner_endpoint_boundary(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    cloud_owner_boundary_ms: Option<u64>,
) -> Option<u64> {
    if update.local_speaker_tracking_enabled {
        let local_owner_ms = qualified_owner_speech_end_ms(update);
        let fresh_local_target_ms = local_target_edge_is_fresh_target_observation(update);
        if local_owner_ms.is_none() && fresh_local_target_ms.is_none() {
            return cloud_owner_boundary_ms;
        }
        // A local watermark is the owner authority, but the local verifier is
        // windowed and can trail a still-current cloud boundary by one
        // cadence.  Permit that narrow bridge only while the newest local
        // window still positively classifies the current speech as the target
        // (an Uncertain window cannot launder a merged cloud row back in),
        // the live speech edge stays within the bounded hold of the local
        // authority, and the cloud edge is aligned with that live edge.  A
        // cloud row that outruns the live local edge (or arrives after local
        // silence) remains bookkeeping and cannot renew the endpoint through
        // room speech.
        let local_authority_ms = local_owner_ms
            .into_iter()
            .chain(fresh_local_target_ms)
            .max();
        let cloud_boundary_is_current = update.local_speaker_classification_kind
            == Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target)
            && update.local_speaker_signal_quality_sufficient == Some(true)
            && update.local_speech_end_ms.is_some_and(|speech_ms| {
            speech_ms
                <= local_authority_ms
                    .unwrap_or(0)
                    .saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
                && !local_speech_confidently_non_target(update, speech_ms)
                && cloud_owner_boundary_ms.is_some_and(|cloud_ms| {
                    cloud_ms.saturating_sub(speech_ms) <= EMBEDDED_LIVE_OWNER_ACTIVITY_ALIGNMENT_MS
                })
        });
        cloud_owner_boundary_ms
            .filter(|_| cloud_boundary_is_current)
            .into_iter()
            .chain(local_owner_ms)
            .chain(fresh_local_target_ms)
            .max()
    } else {
        cloud_owner_boundary_ms
            .into_iter()
            .chain(qualified_owner_speech_end_ms(update))
            .chain(update.local_target_speech_end_ms)
            .max()
    }
}

fn target_speaker_update_has_live_owner_activity(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> bool {
    if !update.target_activity_advanced
        && !update.pending_activity_advanced
        && !update.qualified_owner_activity_advanced
    {
        return false;
    }
    // Confirmed other-speaker energy must not keep the owner clock alive (G).
    if update
        .local_speech_end_ms
        .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms))
    {
        return false;
    }
    let audio_edge_ms = update
        .audio_duration_ms
        .max(update.provider_audio_duration_ms);
    let cloud_owner_edge_ms = update
        .target_speech_end_ms
        .max(update.stable_attributed_speech_end_ms);
    if update.local_speaker_tracking_enabled
        && qualified_owner_speech_end_ms(update).is_some()
        && !update.qualified_owner_activity_advanced
    {
        return false;
    }
    let owner_edge_ms = authoritative_owner_endpoint_boundary(update, cloud_owner_edge_ms);
    audio_edge_ms
        .zip(owner_edge_ms)
        .is_some_and(|(audio, owner)| {
            audio.saturating_sub(owner) <= EMBEDDED_LIVE_OWNER_ACTIVITY_ALIGNMENT_MS
        })
}
#[cfg(test)]
fn target_speaker_endpoint_due(update: &crate::asr::volcengine::TargetSpeakerUpdate) -> bool {
    target_speaker_endpoint_due_with_timeout(update, EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS)
}

#[cfg(test)]
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
    if !update.local_speaker_tracking_enabled
        || !update_has_fresh_unclassified_local_speech(
            update,
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
    {
        return false;
    }
    qualified_owner_speech_end_ms(update)
        .zip(update.local_speech_end_ms)
        .is_some_and(|(target_ms, speech_ms)| {
            speech_ms > target_ms.saturating_add(EMBEDDED_LOCAL_SPEECH_ALIGNMENT_SLACK_MS)
                && speech_ms
                    <= target_ms.saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
                && !local_speech_confidently_non_target(update, speech_ms)
        })
}

/// Installed r10 (fdc68b04, first session on a healthy enrolled bank): the
/// wake window classified Target and established the owner, but the owner's
/// own body under continuous interference stayed in the Uncertain band
/// (0.28–0.47, `signal_quality_sufficient=false`) for the whole utterance —
/// the same normal owner band recorded since 2026-08-14. The bounded
/// `has_uncertain_owner_identity_tail` window (2 s past the last positive
/// owner edge) expired 2 s after the wake boundary, so the inactivity clock
/// ran from the wake and fired `target_speaker_inactive_1000ms` at the
/// user's deliberate ~1 s mid-sentence pause once the provider text flow
/// stalled. Speech that keeps advancing, stays Uncertain, and never becomes
/// confidently non-target is the established owner still talking: identity
/// is degraded, not absent. Bystander-only windows flip to NonTarget
/// against the owner bank (installed r10 post-stop: 0.01–0.11) and keep the
/// ordinary one-second contract. Callers bound this hold with the same
/// `EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS` uncertainty wall so a
/// genuine ending still stops within ~2 s.
fn has_established_owner_uncertain_continuation(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> bool {
    if !update.local_speaker_tracking_enabled
        || !matches!(
            update.local_speaker_classification_kind,
            Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Uncertain)
        )
        || qualified_owner_speech_end_ms(update).is_none()
    {
        return false;
    }
    // Identity-uncertain speech gets the identity-uncertainty budget (2 s),
    // not the confident-speech one-second contract: the accepted round
    // script's deliberate ~1 s mid-sentence pause must survive.
    update
        .audio_duration_ms
        .zip(update.local_speech_end_ms)
        .is_some_and(|(audio_ms, speech_ms)| {
            audio_ms.saturating_sub(speech_ms) < EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS
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
    // Provider growth is only continuation evidence when it is still aligned
    // with a fresh owner-compatible local edge. A cloud row can absorb a room
    // speaker after the local owner watermark has gone quiet; treating that
    // raw growth as OwnerContinuing reopens the endpoint indefinitely.
    if target_speaker_update_has_live_owner_activity(update) {
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

fn owner_endpoint_stop_blocked_by_live_owner(
    fusion_state: TargetSpeakerFusionState,
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    body_started: bool,
    preview_grew_recently: bool,
    body_started_recently: bool,
) -> bool {
    // Product bias: prefer not ending too early over hanging forever.
    // Confirmed other-speaker can still stop (G is lower priority).
    if matches!(fusion_state, TargetSpeakerFusionState::ConfirmedOther) {
        // Continuous-interference guard (ef-hybrid-r1 2026-09-18): a loud
        // second voice can make the local classifier read ConfirmedOther
        // while the owner is still mid-sentence and the visible,
        // target-attributed preview keeps growing (session 1cc84c7d: a
        // 1000 ms target-inactive stop cut a ~15 s body in half at 7.9 s).
        // While a started body's visible text is actively growing, defer the
        // stop to a later tick; the ordinary ConfirmedOther stop resumes as
        // soon as the growth settles (and a no-body session still stops
        // immediately — G stays lower priority).
        if body_started && preview_grew_recently {
            return true;
        }
        return false;
    }
    if matches!(
        fusion_state,
        TargetSpeakerFusionState::OwnerContinuing | TargetSpeakerFusionState::UncertainOwnerTail
    ) {
        return true;
    }
    // r46f: a just-started body inherits an inactive deadline that the wake
    // phrase armed up to a second earlier (user's natural post-wake pause +
    // provider latency). Give the reading its own contract window: ongoing
    // activity (preview growth, owner attribution) re-arms the clock from
    // here, and if nothing follows the ordinary deadline still stops the
    // session within the normal interval after the grace expires.
    if body_started_recently {
        return true;
    }
    if !body_started && update_has_fresh_unclassified_local_speech(update, 1_000) {
        return true;
    }
    // A preview callback alone is not owner evidence. Once the local owner
    // boundary is stale, ordinary/provisional room text can keep growing while
    // fusion is Quiet; reopening on every such callback hangs auto-end. Only
    // a fresh, owner-aligned activity edge may renew the body stop barrier.
    body_started && preview_grew_recently && target_speaker_update_has_live_owner_activity(update)
}

/// Give recent speech near a confirmed owner boundary a bounded chance to be
/// classified. This is a stop barrier, not a new owner watermark. Explicit
/// other-speaker evidence and speech beyond the uncertainty window cannot hold.
fn has_unresolved_recent_owner_speech(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    endpoint_timeout_ms: u64,
) -> bool {
    let Some(audio_ms) = update.audio_duration_ms else {
        return false;
    };
    let Some(owner_ms) = endpoint_owner_watermark(update) else {
        return false;
    };
    let speech_ms = update.local_speech_end_ms.unwrap_or(owner_ms);
    if local_speech_confidently_non_target(update, speech_ms)
        || audio_ms.saturating_sub(speech_ms) >= endpoint_timeout_ms
        || speech_ms >= owner_ms.saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
    {
        return false;
    }
    speech_ms > owner_ms
}

#[cfg(test)]
fn target_speaker_endpoint_due_with_provider_stall(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    provider_stall_confirmed: bool,
    endpoint_timeout_ms: u64,
) -> bool {
    let unresolved_recent_owner_speech =
        has_unresolved_recent_owner_speech(update, endpoint_timeout_ms);
    let local_target_authority =
        update.local_speaker_tracking_enabled && qualified_owner_speech_end_ms(update).is_some();
    let cloud_target_authority = update.speaker_info_present && update.speaker_id.is_some();
    let recent_local_speech_is_non_target = update
        .local_speech_end_ms
        .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms));
    let confirmed_non_target_owner_gap = update
        .local_non_target_speech_end_ms
        .zip(qualified_owner_speech_end_ms(update))
        .is_some_and(|(non_target_ms, target_ms)| {
            non_target_ms.saturating_sub(target_ms) >= EMBEDDED_CONFIRMED_NON_TARGET_OWNER_GAP_MS
        });
    let uncertain_owner_budget_exhausted = update
        .qualified_owner_speech_end_ms
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
    // A provisional provider row is not, by itself, evidence that the owner
    // is still speaking. Volcengine may keep `pending_unattributed_speech` set
    // after the owner clock is quiet; making that flag an unconditional
    // barrier is the reason automatic stop can hang forever. Pending text is
    // therefore a hold only while a bounded, recent owner-compatible tail is
    // present, on both the normal and stalled provider paths.
    // Strong local NonTarget is sufficient only after it has clearly moved
    // beyond the last owner boundary. Cloud diarization may collapse both
    // voices into the same speaker id, so requiring
    // `provider_other_speaker_advanced` alone can leave the session recording
    // forever; releasing on a single borderline window would instead swallow
    // the owner's recovering tail.
    let pending_blocks_endpoint = update.pending_unattributed_speech
        && unresolved_recent_owner_speech
        && !(recent_local_speech_is_non_target && confirmed_non_target_owner_gap);
    let stable_attributed_speech_end_ms = (!recent_local_speech_is_non_target
        && !uncertain_owner_budget_exhausted)
        .then_some(update.stable_attributed_speech_end_ms)
        .flatten();
    let cloud_target_speech_end_ms = update
        .target_speech_end_ms
        .into_iter()
        // Provider diarization can briefly split one continuous owner utterance
        // into a new speaker id. Stable attributed speech must still hold the
        // endpoint clock even though target-only text filtering remains strict.
        .chain(stable_attributed_speech_end_ms)
        .max();
    let target_speech_end_ms =
        authoritative_owner_endpoint_boundary(update, cloud_target_speech_end_ms);
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
        && !unresolved_recent_owner_speech
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
        || local_audio_ms.saturating_sub(provider_audio_ms)
            < EMBEDDED_PROVIDER_STALL_FALLBACK_LAG_MS
    {
        return false;
    }

    // Ongoing unclassified local energy means the owner may still be speaking
    // while ASR/provider clocks froze. Confirmed non-target (other people) or
    // energy that has itself been quiet for the full endpoint interval may
    // still use stall fallback so room noise does not hold the session open.
    let unresolved_recent_owner_speech =
        has_unresolved_recent_owner_speech(update, endpoint_timeout_ms);
    if unresolved_recent_owner_speech {
        return false;
    }

    // A provisional provider row is not an endpoint authority.  During room
    // interference Volcengine can leave `pending_unattributed_speech` latched
    // after the local owner clock is quiet; treating that bit as a hard stall
    // barrier made the local provider-stall fallback wait forever.  The
    // bounded local-owner-tail check above is the only pending condition that
    // may hold auto-end.

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
    if qualified_owner_speech_end_ms(update).is_none() && !local_latest_is_non_target {
        return false;
    }

    // A newer locally confirmed target tail is authoritative only after that
    // newer boundary has itself been inactive for the full endpoint interval.
    // This preserves quiet tails without waiting forever for a stalled provider
    // to repeat coverage it has already stopped reporting.
    let newest_target_end_ms =
        authoritative_owner_endpoint_boundary(update, Some(cloud_target_end_ms))
            .unwrap_or(cloud_target_end_ms);
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
    enrolled_owner_matched: bool,
) -> bool {
    stage_terminal_wake_continuation_with_body_at(
        inner,
        wake_pcm,
        wake_end_seconds,
        wake_phrase,
        enrolled_owner_matched,
        TerminalWakeBody {
            candidate_id: 0,
            pcm: Vec::new(),
            candidate_range: None,
            source_runs: VecDeque::new(),
            source_admission_ledger: Arc::new(std::sync::Mutex::new(
                SourceAdmissionDependencyLedger::default(),
            )),
        },
        Instant::now(),
    )
}

fn stage_terminal_wake_continuation_at(
    inner: &Arc<Inner>,
    wake_pcm: Vec<u8>,
    wake_end_seconds: f32,
    wake_phrase: String,
    enrolled_owner_matched: bool,
    now: Instant,
) -> bool {
    stage_terminal_wake_continuation_with_body_at(
        inner,
        wake_pcm,
        wake_end_seconds,
        wake_phrase,
        enrolled_owner_matched,
        TerminalWakeBody {
            candidate_id: 0,
            pcm: Vec::new(),
            candidate_range: None,
            source_runs: VecDeque::new(),
            source_admission_ledger: Arc::new(std::sync::Mutex::new(
                SourceAdmissionDependencyLedger::default(),
            )),
        },
        now,
    )
}

fn stage_terminal_wake_continuation_with_body_at(
    inner: &Arc<Inner>,
    wake_pcm: Vec<u8>,
    wake_end_seconds: f32,
    wake_phrase: String,
    enrolled_owner_matched: bool,
    body: TerminalWakeBody,
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
        enrolled_owner_matched,
        body,
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

fn terminal_wake_continuation_waiting_for_audio(inner: &Arc<Inner>) -> Option<SessionId> {
    terminal_wake_continuation_waiting_for_audio_at(inner, Instant::now())
}

fn terminal_wake_continuation_waiting_for_audio_at(
    inner: &Arc<Inner>,
    now: Instant,
) -> Option<SessionId> {
    let (bound_session_id, expired) = {
        let mut slot = inner.embedded_audio_terminal_wake_continuation.lock();
        let Some(continuation) = slot.as_ref() else {
            return None;
        };
        if continuation.expires_at <= now {
            *slot = None;
            (None, true)
        } else {
            (continuation.session_id, false)
        }
    };
    if expired {
        clear_automatic_wake_text_guard(inner);
        return None;
    }
    let session_id = bound_session_id?;
    let state = inner.state.lock();
    (state.phase == SessionPhase::Starting && state.session_id == session_id).then_some(session_id)
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
        let mut lifecycle = inner.recording_lifecycle.lock();
        if let Some(candidate_id) = lifecycle.current_candidate_session_id() {
            let _ = lifecycle.close_candidate(candidate_id);
        }
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
        let mut lifecycle = inner.recording_lifecycle.lock();
        if let Some(candidate_id) = lifecycle.current_candidate_session_id() {
            let _ = lifecycle.close_candidate(candidate_id);
        }
    }
}

/// A successful VREC start write is not proof that the continuation segment
/// reached the host. Bound the transport-attachment phase so an accepted
/// terminal wake cannot leave the product in Starting forever. The cleanup is
/// identity-scoped through both the continuation slot and candidate lifecycle.
fn dispatch_owned_candidate_transport_stop(
    inner: &Arc<Inner>,
    candidate_id: u32,
    reason: &'static str,
    delay_ms: u64,
) {
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn_blocking(move || {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        // A host key/CLI start creates Starting before its BLE segment arrives.
        // The old candidate tombstone still exists during that interval, but
        // no longer owns the user's recording (physical session 86320180).
        if inner.state.lock().phase != SessionPhase::Idle {
            log::info!(
                "[coord] skip candidate VREC:STOP reason={reason} rejected_session={candidate_id}; user session owns capture"
            );
            return;
        }
        if !inner
            .recording_lifecycle
            .lock()
            .rejected_candidate_still_owns_transport_stop(candidate_id)
        {
            log::info!(
                "[coord] skip stale VREC:STOP reason={reason} rejected_session={candidate_id}; lifecycle ownership advanced"
            );
            return;
        }
        match crate::embedded_ble::send_recording_control_stop(
            EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
        ) {
            Ok(()) => log::info!(
                "[coord] VREC:STOP sent reason={reason} embedded_session_id={candidate_id}"
            ),
            Err(err) => log::warn!(
                "[coord] VREC:STOP failed reason={reason} embedded_session_id={candidate_id}: {err}"
            ),
        }
    });
}

fn schedule_terminal_wake_continuation_expiry(
    inner: &Arc<Inner>,
    candidate_id: u32,
    session_id: SessionId,
) {
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(EMBEDDED_TERMINAL_WAKE_CONTINUATION_TTL).await;
        let expired = {
            let mut slot = inner.embedded_audio_terminal_wake_continuation.lock();
            if slot.as_ref().is_some_and(|continuation| {
                continuation.session_id == Some(session_id)
                    && continuation.expires_at <= Instant::now()
            }) {
                *slot = None;
                true
            } else {
                false
            }
        };
        if !expired {
            return;
        }
        clear_automatic_wake_text_guard(&inner);
        if !inner
            .recording_lifecycle
            .lock()
            .close_candidate(candidate_id)
        {
            return;
        }
        log::warn!(
            "[wake-phrase] terminal continuation expired before body transport attached embedded_session_id={candidate_id} coordinator_session_id={session_id}"
        );
        let _ = publish_dictation_pipeline_error(
            &inner,
            session_id,
            "Listener 未收到唤醒后的录音数据".to_string(),
        );
        schedule_actionable_error_capsule_idle(&inner, session_id);
        dispatch_owned_candidate_transport_stop(
            &inner,
            candidate_id,
            "terminal_continuation_expiry",
            0,
        );
    });
}

include!("dictation_volcengine_callbacks.rs");

fn build_volcengine_asr(inner: &Arc<Inner>, session_id: SessionId) -> Arc<VolcengineStreamingASR> {
    let proxy_config = super::read_asr_proxy_config("volcengine").unwrap_or_else(|error| {
        log::warn!(
            "[network] invalid Volcengine ASR proxy settings; using provider default: {error}"
        );
        crate::polish::ProviderProxyConfig::provider_default("volcengine")
    });
    let mut asr_config = VolcengineStreamingASR::new_with_proxy_config(
        read_volc_credentials(),
        enabled_hotwords(inner),
        proxy_config,
    );
    if record_embedded_audio_for_debug_enabled(inner) {
        asr_config.enable_diagnostic_trace();
    }
    let asr = Arc::new(asr_config);
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
    let session_id = inner.state.lock().session_id;
    inner
        .embedded_audio_preview
        .lock()
        .authoritative(session_id)
}

fn current_embedded_audio_visual_preview(inner: &Arc<Inner>) -> Option<String> {
    let session_id = inner.state.lock().session_id;
    inner.embedded_audio_preview.lock().visible(session_id)
}

fn current_embedded_audio_endpoint_preview(inner: &Arc<Inner>) -> Option<String> {
    current_embedded_audio_visual_preview(inner)
        .or_else(|| current_embedded_audio_partial_preview(inner))
}

include!("dictation_preview.rs");
include!("dictation_streaming_composition.rs");

async fn request_embedded_ble_recording_stop_from_host_for_endpoint(
    inner: &Arc<Inner>,
    reason: &'static str,
    admission: EndpointStopAdmission,
) -> Result<bool, String> {
    if !embedded_ble_host_recording_control_context_active(inner) {
        return Ok(false);
    }
    let expected_session_id = admission.ticket.session_id;
    let phase = {
        let state = inner.state.lock();
        if state.session_id != expected_session_id
            || state.cancelled
            || !matches!(state.phase, SessionPhase::Starting | SessionPhase::Listening)
        {
            return Ok(false);
        }
        state.phase
    };
    if !admission.is_valid(inner) {
        return Ok(false);
    }
    if !admission
        .stop_state
        .lock()
        .begin_sending(admission.ticket)
    {
        return Ok(false);
    }

    // The ticket is now Sending, but the physical write has not started. A
    // newly published PendingSpeech/Speech edge, a rearmed endpoint
    // generation, or a session hand-off must still revoke it here.
    if !admission.is_valid(inner) {
        return Ok(false);
    }
    if !commit_recording_stop_owned(
        inner,
        expected_session_id,
        reason,
        admission.ticket.proposal_id,
    ) {
        return Ok(false);
    }

    record_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::StopCommand,
        Some(expected_session_id),
        format!(
            "endpoint stop requested session_id={} proposal_id={} generation={} reason={reason} phase={phase:?}",
            expected_session_id,
            admission.ticket.proposal_id,
            admission.ticket.endpoint_generation,
        ),
    );

    #[cfg(test)]
    {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            "firmware_stop_skipped_test",
            format!(
                "session_id={expected_session_id} proposal_id={} phase={phase:?} reason={reason}",
                admission.ticket.proposal_id,
            ),
        );
        return Ok(true);
    }

    #[cfg(not(test))]
    {
        // This is intentionally immediately adjacent to the blocking BLE
        // write. The endpoint ticket is allowed to stay proposed while ASR
        // and VAD callbacks run, but no stale proposal crosses this final
        // admission boundary.
        if !admission.is_valid(inner) {
            let _ = reopen_recording_stop_owned(
                inner,
                expected_session_id,
                admission.ticket.proposal_id,
            );
            return Ok(false);
        }
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
                    format!(
                        "session_id={expected_session_id} proposal_id={} phase={phase:?} reason={reason}",
                        admission.ticket.proposal_id,
                    ),
                );
                log::info!(
                    "[coord] embedded BLE endpoint STOP sent session_id={expected_session_id} proposal_id={} phase={phase:?} reason={reason}",
                    admission.ticket.proposal_id,
                );
                Ok(true)
            }
            Err(err) => {
                let _ = reopen_recording_stop_owned(
                    inner,
                    expected_session_id,
                    admission.ticket.proposal_id,
                );
                set_device_ai_processing_async(inner, false, "host_stop_failed");
                crate::timeline::mark(
                    "backend.embedded_ble_session_actor",
                    "firmware_stop_failed",
                    format!(
                        "session_id={expected_session_id} proposal_id={} phase={phase:?} reason={reason} error={err}",
                        admission.ticket.proposal_id,
                    ),
                );
                log::warn!(
                    "[coord] embedded BLE endpoint STOP failed session_id={expected_session_id} proposal_id={} phase={phase:?} reason={reason}: {err}",
                    admission.ticket.proposal_id,
                );
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some("Listener 录音停止失败".to_string()),
                    None,
                );
                schedule_capsule_idle(inner, 6000, Some(expected_session_id));
                Err(err)
            }
        }
    }
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

    // This is the sole product-owner STOP transaction. Callers may propose a
    // stop, but they cannot pre-commit the lifecycle or dispatch transport on
    // their own. Keeping identity admission and the physical write together
    // prevents the old split-brain state where the app finalized while the
    // device continued recording.
    if !commit_recording_stop(inner, session_id, reason) {
        return Ok(false);
    }
    // 手动停止不提交预览（尾部音频可能还在路上，见 46bcdda），但可以提前预热
    // 润色：delta 只进缓冲不上屏，终稿逐字一致才被采用，不一致自动丢弃。
    if matches!(
        reason,
        "capsule_confirm_stop_processing_start" | "device_key_stop" | "device_key_stop_retry"
    ) {
        maybe_start_polish_prefetch(inner, session_id);
    }

    record_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::StopCommand,
        Some(session_id),
        format!("host stop requested reason={reason} phase={phase:?}"),
    );
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
                reopen_recording_stop(inner, session_id);
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
    candidate_id: Option<u64>,
    source_stream_id: u64,
    active_asr: String,
    consumer: Arc<dyn crate::recorder::AudioConsumer>,
    volcengine_asr: Option<Arc<VolcengineStreamingASR>>,
    pipeline_observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    pcm_stage_ledger: crate::observability::PcmStageMappingLedger,
    pcm_stage_operation_id: Option<u64>,
    source_admission_operation_id: Option<u64>,
    candidate_fact_ledger: Option<crate::observability::CandidateFactLedger>,
    source_admission_ledger: Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
    accepted_pcm_cursor: PcmDiagnosticCursor,
    archive_pcm_cursor: PcmDiagnosticCursor,
    normalized_pcm_cursor: PcmDiagnosticCursor,
    archive_pcm: Option<Vec<u8>>,
    streamed_pcm_bytes: usize,
    normalized_pcm_bytes: usize,
    streaming_pcm_buffer: Vec<u8>,
    streaming_pcm_sources: VecDeque<EmbeddedStreamingPcmSourceRun>,
    streaming_agc: EmbeddedStreamingAgcState,
    local_speech_activity: LocalSpeechActivity,
    local_speaker_tracker: Option<LocalSessionSpeakerTracker>,
    device_ai_processing_started: bool,
    // Proactive trailing-silence stop (改A) state. See
    // EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS and consume_prepared_streaming_pcm.
    proactive_stop_body_started: bool,
    proactive_stop_silence_ms: u64,
    proactive_stop_dispatched: bool,
}

#[derive(Clone)]
struct EmbeddedStreamingPcmSourceRun {
    bytes: usize,
    observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    segment_id: Option<u32>,
    source_interval: Option<crate::observability::PcmSourceInterval>,
    collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    collector_emitted_range: Option<crate::embedded_audio::StreamingPcmRange>,
}

include!("dictation_wake_diagnostics.rs");
#[derive(Debug)]
struct PreparedDeliveryText {
    /// Text selected by the final pipeline as the intended delivery body.
    intended_text: String,
    /// Text already handed to the input mechanism before finalization. This
    /// is populated only by streaming and may be a strict prefix on failure.
    submitted_text: Option<String>,
    /// Text used by history and the embedded final-result bridge.
    final_text: String,
    polish_error: Option<String>,
    already_streamed: bool,
}

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
    begin_embedded_audio_preview_session(inner, current_session_id);
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
    // 组字流式(2026-09-22 切片3):TSF 就绪+目标可解析时起驱动,说话期间
    // 逐字组字;任何不满足静默保持粘贴行为。回退 LISTENER_DISABLE_STREAMING_COMPOSITION=1。
    begin_streaming_composition_session(inner, current_session_id).await;
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
    let local_speech_activity = volcengine_asr
        .as_ref()
        .map(|asr| {
            #[cfg(target_os = "windows")]
            {
                LocalSpeechActivity::new(asr.local_speech_activity_sink())
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = asr;
                LocalSpeechActivity::new()
            }
        })
        .unwrap_or_else(LocalSpeechActivity::disabled);
    Ok(EmbeddedAudioDictationSession {
        session_id: current_session_id,
        candidate_id: None,
        source_stream_id: next_embedded_coordinator_source_stream_id(),
        active_asr,
        consumer,
        volcengine_asr,
        pipeline_observation: None,
        pcm_stage_ledger: crate::observability::PcmStageMappingLedger::default(),
        pcm_stage_operation_id: Some(1),
        source_admission_operation_id: Some(1),
        candidate_fact_ledger: None,
        source_admission_ledger: Arc::new(std::sync::Mutex::new(
            SourceAdmissionDependencyLedger::default(),
        )),
        accepted_pcm_cursor: PcmDiagnosticCursor::default(),
        archive_pcm_cursor: PcmDiagnosticCursor::default(),
        normalized_pcm_cursor: PcmDiagnosticCursor::default(),
        archive_pcm,
        streamed_pcm_bytes: 0,
        normalized_pcm_bytes: 0,
        streaming_pcm_buffer: Vec::new(),
        streaming_pcm_sources: VecDeque::new(),
        streaming_agc: EmbeddedStreamingAgcState::default(),
        local_speech_activity,
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
    // r23：HWND 与标题原子成对抓取，自愈路径靠标题识别真实窗口。
    let (focus_target, focus_title) = capture_focus_target_with_title();
    let started =
        begin_session_state(&mut state, focus_target, capture_frontmost_app())
            .ok_or_else(|| "当前已有听写会话在运行，暂不能提交嵌入式音频".to_string())?;
    state.focus_target_title = focus_title;
    Ok(started)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum HiddenCandidateTransportState {
    Streaming,
    Ended,
}

/// Returns whether a physical stop was requested, independently of closing
/// the host candidate. Device STOP/CANCEL already ended the physical segment.
fn reject_hidden_automatic_candidate(
    inner: &Arc<Inner>,
    reason: &'static str,
    embedded_session_id: u32,
    transport: HiddenCandidateTransportState,
) -> bool {
    if !inner
        .recording_lifecycle
        .lock()
        .close_candidate(embedded_session_id)
    {
        log::info!(
            "[speaker-verification] ignored stale hidden candidate rejection reason={reason} embedded_session_id={embedded_session_id}"
        );
        return false;
    }
    log::info!(
        "[speaker-verification] hidden automatic candidate rejected silently reason={reason} embedded_session_id={embedded_session_id}"
    );
    if transport == HiddenCandidateTransportState::Ended {
        // The device may already be recording N+1 while its START is still in
        // the notify queue. Host identity checks cannot make a bare VREC:STOP
        // safe here, even when N is still the latest host-observed candidate.
        log::info!(
            "[speaker-verification] skipped redundant transport stop for ended hidden candidate embedded_session_id={embedded_session_id}"
        );
        return false;
    }
    // Hidden VA candidates never open a Type dictation session (phase stays Idle),
    // so request_embedded_ble_recording_stop_from_host is a no-op. Without an
    // explicit VREC:STOP the device keeps streaming ambient speech until silence
    // or max_session — owner could not re-arm 「开始录音」. Cut the device only
    // when this reject is still the latest candidate (avoid killing N+1).
    #[cfg(not(test))]
    {
        // Brief yield: SessionStart for the next candidate often races the
        // terminal reject of the previous one. Ownership is checked by the
        // shared candidate-stop dispatcher immediately before the write.
        dispatch_owned_candidate_transport_stop(inner, embedded_session_id, reason, 80);
    }
    true
}

/// Show Recording capsule after local ExactStart. Do not pop it on a bare KWS
/// hit: enrolled voiceprint can still reject, which flashed the capsule then
/// dismissed it (live 2026-09-10 9895). Product Accept still creates the
/// coordinator session.
fn show_early_wake_recording_capsule(inner: &Arc<Inner>, candidate: &mut BufferedSpeakerCandidate) {
    if candidate.early_capsule_session_id.is_some() {
        return;
    }
    // This is candidate UI, not product activation. The previous path called
    // begin_session_state here and made an unverified phrase look like a real
    // Starting session. A later reject then wrote SessionPhase::Idle directly,
    // racing the next accepted wake. Keep the capsule token independent; the
    // accepted owner path alone creates the coordinator session.
    if inner.state.lock().phase != SessionPhase::Idle {
        return;
    }
    let session_id = uuid::Uuid::new_v4();
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
    !matches!(
        origin,
        Some(
            crate::embedded_audio::SessionStopOrigin::VoiceActivation
                | crate::embedded_audio::SessionStopOrigin::VoiceActivationMaxDuration
        )
    )
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
            "event=pcm embedded_session_id={} packet_sequence={} pcm_bytes={} raw_input_level_percent={:?} after_stop={} collector_metadata={:?}",
            chunk.session_id,
            chunk.packet_sequence,
            chunk.pcm.len(),
            chunk.raw_input_level_percent,
            chunk.after_stop_boundary,
            chunk.metadata
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
    end_embedded_ble_session_with_source_integrity(inner, user_initiated_stop, detail, None).await
}

async fn end_embedded_ble_session_with_source_integrity(
    inner: &Arc<Inner>,
    user_initiated_stop: bool,
    detail: impl Into<String>,
    source_integrity_ledger: Option<Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>>,
) -> Result<(), String> {
    let session_id = inner.state.lock().session_id;
    let transition = dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::StopCommand,
        Some(session_id),
        detail,
        |_| begin_stop_session_transition(inner, user_initiated_stop),
    );
    finish_end_session_after_stop_transition_with_source_integrity(
        inner,
        transition,
        source_integrity_ledger,
    )
    .await
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

fn finish_source_integrity_blocked(
    inner: &Arc<Inner>,
    session_id: SessionId,
    decision: &crate::coordinator::source_integrity::SourceIntegrityDecision,
) -> bool {
    let reason = format!("source_integrity_blocked: {:?}", decision.reason);
    log::warn!(
        "[source-integrity] blocking embedded automatic delivery session_id={} reason={:?} evidence={:?}",
        session_id,
        decision.reason,
        decision.first_evidence
    );
    cancel_asr_for_session(inner, session_id);
    if !finish_dictation_pipeline_error(inner, session_id, reason) {
        return false;
    }
    clear_automatic_wake_text_guard(inner);
    clear_embedded_audio_preview_session(inner, session_id);
    set_device_ai_processing_async(inner, false, "source_integrity_blocked");

    let embedded_audio_stats = take_embedded_audio_stats(inner);
    let duration_ms = embedded_audio_stats
        .as_ref()
        .map(|stats| (stats.duration_seconds.max(0.0) * 1_000.0) as u64);
    let prefs_snapshot = inner.prefs.get();
    let history = DictationSession {
        id: session_id.to_string(),
        created_at: Utc::now().to_rfc3339(),
        raw_transcript: String::new(),
        final_text: String::new(),
        mode: prefs_snapshot.default_mode,
        app_bundle_id: None,
        app_name: None,
        insert_status: InsertStatus::Failed,
        error_code: Some("source_integrity_blocked".to_string()),
        duration_ms,
        dictionary_entry_count: Some(enabled_phrases(inner).len() as u32),
        has_audio_recording: Some(inner.audio_archive_active.load(Ordering::Relaxed)),
        embedded_audio_stats,
    };
    if let Err(error) = inner.history.append_with_retention(
        history,
        prefs_snapshot.history_retention_days,
        prefs_snapshot.history_max_entries,
    ) {
        log::error!("[source-integrity] blocked history append failed: {error}");
    }
    store_embedded_audio_final_result(
        inner,
        crate::embedded_audio::EmbeddedAudioTranscriptResult {
            session_id: session_id.to_string(),
            raw_transcript: String::new(),
            final_text: String::new(),
            error_code: Some("source_integrity_blocked".to_string()),
        },
    );
    true
}

fn source_integrity_must_stop(
    inner: &Arc<Inner>,
    expected_session_id: SessionId,
    ledger: Option<&Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>>,
    boundary: &'static str,
) -> bool {
    let Some(ledger) = ledger else {
        return false;
    };
    let current_session_id = inner.state.lock().session_id;
    let decision = {
        let mut ledger = ledger
            .lock()
            .expect("source admission dependency ledger lock for qualification");
        crate::coordinator::source_integrity::qualify_active_source_integrity(
            crate::coordinator::source_integrity::SourceIntegrityOwner::Session(
                expected_session_id,
            ),
            crate::coordinator::source_integrity::SourceIntegrityOwner::Session(current_session_id),
            &mut ledger,
            None,
        )
    };
    match decision.verdict {
        crate::coordinator::source_integrity::SourceIntegrityQualification::SourceIntegrityBlocked => {
            finish_source_integrity_blocked(inner, expected_session_id, &decision);
            true
        }
        crate::coordinator::source_integrity::SourceIntegrityQualification::OwnerMismatch => {
            log::warn!(
                "[source-integrity] skipping stale embedded completion boundary={} expected_session_id={} current_session_id={}",
                boundary,
                expected_session_id,
                current_session_id
            );
            true
        }
        crate::coordinator::source_integrity::SourceIntegrityQualification::Allowed => {
            if decision.evidence_completeness
                == crate::coordinator::source_integrity::SourceIntegrityEvidenceCompleteness::Unknown
            {
                log::info!(
                    "[source-integrity] evidence incomplete but preserving existing behavior boundary={} session_id={} reason={:?}",
                    boundary,
                    expected_session_id,
                    decision.reason
                );
            }
            false
        }
    }
}

fn register_embedded_source_integrity_ledger(
    inner: &Arc<Inner>,
    session_id: SessionId,
    ledger: &Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
) {
    inner
        .embedded_ble_session_actor
        .lock()
        .source_integrity_ledger = Some((session_id, Arc::clone(ledger)));
}

fn embedded_source_integrity_ledger_for_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> Option<Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>> {
    inner
        .embedded_ble_session_actor
        .lock()
        .source_integrity_ledger
        .as_ref()
        .filter(|(owner_session_id, _)| *owner_session_id == session_id)
        .map(|(_, ledger)| Arc::clone(ledger))
}

fn clear_embedded_source_integrity_ledger(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let mut actor = inner.embedded_ble_session_actor.lock();
    if actor
        .source_integrity_ledger
        .as_ref()
        .is_none_or(|(owner_session_id, _)| *owner_session_id != session_id)
    {
        return false;
    }
    actor.source_integrity_ledger = None;
    true
}

struct EmbeddedSourceIntegrityLedgerCleanup {
    inner: Arc<Inner>,
    session_id: SessionId,
}

impl Drop for EmbeddedSourceIntegrityLedgerCleanup {
    fn drop(&mut self) {
        clear_embedded_source_integrity_ledger(&self.inner, self.session_id);
    }
}

async fn finish_end_session_after_stop_transition(
    inner: &Arc<Inner>,
    transition: DictationTransition,
) -> Result<(), String> {
    finish_end_session_after_stop_transition_with_source_integrity(inner, transition, None).await
}

async fn finish_end_session_after_stop_transition_with_source_integrity(
    inner: &Arc<Inner>,
    transition: DictationTransition,
    source_integrity_ledger: Option<Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>>,
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
    let _source_integrity_ledger_cleanup = EmbeddedSourceIntegrityLedgerCleanup {
        inner: Arc::clone(inner),
        session_id: current_session_id,
    };

    if source_integrity_must_stop(
        inner,
        current_session_id,
        source_integrity_ledger.as_ref(),
        "before_asr_finalization",
    ) {
        return Ok(());
    }

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
            transition_pipeline_error_if_session_matches(inner, current_session_id);
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
    #[cfg(target_os = "windows")]
    let mut local_shadow_task = volcengine_for_empty_retry
        .as_ref()
        .filter(|asr| asr.may_run_local_shadow_decode())
        .and_then(|asr| {
            let pcm = asr.retained_pcm_snapshot();
            let audio_ms = pcm.len() / 32;
            (LOCAL_SHADOW_ASR_MIN_AUDIO_MS..=LOCAL_SHADOW_ASR_MAX_AUDIO_MS)
                .contains(&audio_ms)
                .then(|| {
                    let started = Instant::now();
                    let task = tauri::async_runtime::spawn_blocking(move || {
                        crate::asr::local::wake_helper::transcribe_if_ready(
                            &pcm,
                            Duration::from_millis(LOCAL_SHADOW_ASR_HELPER_TIMEOUT_MS),
                        )
                    });
                    (started, audio_ms, task)
                })
        });
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    let mut target_speaker_filter_required = false;
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    let mut primary_speaker_filtered_certified = false;
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    let mut separated_owner_candidate: Option<RawTranscript> = None;
    #[cfg(not(all(target_os = "windows", feature = "target-speaker-extraction")))]
    let target_speaker_filter_required = false;
    #[cfg(not(all(target_os = "windows", feature = "target-speaker-extraction")))]
    let primary_speaker_filtered_certified = false;
    #[cfg(not(all(target_os = "windows", feature = "target-speaker-extraction")))]
    let separated_owner_candidate: Option<RawTranscript> = None;
    // Quiet marks the end of capture, not completion of recognition. In
    // automatic sessions, committing the preview here used to cancel the
    // provider before its final correction and skip owner-only arbitration.
    // Both manual and automatic completion must settle the same ASR path.
    // 2026-09-22 跟手①：终稿侧改写恢复只认干净会话；标志在终稿落定后读
    // （干扰冻结多发生在 STOP 与终稿之间）。
    let mut pause_early_final_clean = false;
    let raw = match asr {
        ActiveAsr::Volcengine(asr) => {
            debug_assert!(uses_global_timeout);
            let timeout_duration = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
            let send_result = asr.send_last_frame().await;
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            let (primary, target_result) =
                match send_result {
                    Ok(()) => {
                        // r35（2026-09-18 晚）：用户在同干扰源 A/B 下判 tih
                        // "吞字/没有以前好用"（tig 四连绿 vs tih 三轮断在中
                        // 停）。延迟优化机制上只动停说后，无法解释端点提前，
                        // 但按 A/B 证据先撤回串行行为；待端点取证
                        // （fusion_state=ConfirmedOther 中停掐断）定位真因后
                        // 再评估重新引入 begin_target_speaker_final_early()。
                        // Settle the low-latency provider first. Its final speaker
                        // boundary can prove that a later local NonTarget tail was
                        // already excluded, avoiding a redundant separator wait.
                        // The separator itself has processed capture audio in the
                        // background, so ambiguous overlap still gets its bounded
                        // chance immediately afterwards.
                        // 2026-09-21 跟手：early-seal —— 干净会话端点 STOP 时账本
                        // 已稳定整个耐心窗，稳定账本即完整终稿；真实终稿 350ms
                        // 内没到就地封存（干扰会话在 ASR 内部自动退回完整等待）。
                        let primary = match tokio::time::timeout(
                            timeout_duration,
                            asr.await_final_result_with_early_seal(),
                        )
                        .await
                        {
                            Ok(result) => result.map_err(|error| (error, false)),
                            Err(_) => Err((
                                crate::asr::volcengine::VolcengineASRError::FinalResultTimeout,
                                true,
                            )),
                        };
                        let target_result = if primary.is_ok() {
                            asr.await_target_speaker_final().await
                        } else {
                            Ok(None)
                        };
                        (primary, target_result)
                    }
                    Err(error) => (Err((error, false)), Ok(None)),
                };
            #[cfg(not(all(target_os = "windows", feature = "target-speaker-extraction")))]
            let primary = match send_result {
                Ok(()) => {
                    // 添加全局超时保护：防止 await_final_result() 永远挂起
                    // （2026-09-21 跟手：同 early-seal 语义，见上方分支注释）
                    match tokio::time::timeout(
                        timeout_duration,
                        asr.await_final_result_with_early_seal(),
                    )
                    .await
                    {
                        Ok(result) => result.map_err(|error| (error, false)),
                        Err(_) => Err((
                            crate::asr::volcengine::VolcengineASRError::FinalResultTimeout,
                            true,
                        )),
                    }
                }
                Err(error) => Err((error, false)),
            };
            // 干净标志在终稿落定后读：见上方 pause_early_final_clean 注释。
            pause_early_final_clean = asr.pause_early_final_clean_session();
            let primary_result = match primary {
                Ok(result) => result,
                Err((primary_error, _)) if primary_error.permits_full_audio_replay() => {
                    // 网络报错灯（2026-09-22 用户拍板）：断流恢复期间胶囊不再装死，
                    // 明示"网络波动"；同时发射一次分层探测（火山/对照站）供定位。
                    // 2026-09-23 提到 source-integrity 门之前：门静默收线时胶囊也
                    // 得有说法（09:52 实锤：黑洞+干扰下门先收，胶囊装死到用户按停）。
                    crate::net_health::log_network_health_probe_async("primary_stream_failed");
                    emit_embedded_audio_transcribing_if_active(
                        inner,
                        current_session_id,
                        Some("网络波动，正在恢复…请稍等勿按停".to_string()),
                    );
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_replay",
                    ) {
                        return Ok(());
                    }
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
                            // 重放也失败时做一次分层探测：网络全坏 → 报"网络不佳"；
                            // 网络正常 → 维持原"识别恢复失败"（问题在别处）。
                            let failure_message = tokio::time::timeout(
                                Duration::from_millis(2_500),
                                tokio::task::spawn_blocking(
                                    crate::net_health::probe_network_health,
                                ),
                            )
                            .await
                            .ok()
                            .and_then(|joined| joined.ok())
                            .map(|snapshot| {
                                log::warn!("{} reason=replay_failed", snapshot.log_line());
                                match snapshot.classify() {
                                    crate::net_health::NetworkHealthClass::AllGood => {
                                        format!("识别恢复失败: {recovery_error}")
                                    }
                                    class => format!(
                                        "{}，本次识别未能恢复",
                                        class.user_message()
                                    ),
                                }
                            })
                            .unwrap_or_else(|| format!("识别恢复失败: {recovery_error}"));
                            finish_dictation_pipeline_error(
                                inner,
                                current_session_id,
                                failure_message,
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
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
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
            };
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            {
                target_speaker_filter_required = asr.target_speaker_filter_was_required();
                // r36：主轨终稿的封存认证。speaker_filtered 认证 = 干净本人
                // 全文，产品仲裁据此把"分离稿截断成前缀"（r36 吞字形态）判为
                // 劣化而非排除。
                primary_speaker_filtered_certified = asr.primary_seal_speaker_filtered_certified();
                separated_owner_candidate = match target_result {
                    Ok(Some(target)) if !target.text.trim().is_empty() => {
                        log::info!(
                            "[target-speaker] owner-only final offered to product arbiter primary_chars={} target_chars={}",
                            primary_result.text.chars().count(),
                            target.text.chars().count()
                        );
                        Some(target)
                    }
                    Ok(Some(_)) | Ok(None) if target_speaker_filter_required => {
                        log::warn!(
                            "[target-speaker] required owner-only final unavailable; sealed provider primary is the only owner-policy candidate"
                        );
                        None
                    }
                    Ok(Some(_)) | Ok(None) => None,
                    Err(err) if target_speaker_filter_required => {
                        log::warn!(
                            "[target-speaker] required owner-only final failed; sealed provider primary is the only owner-policy candidate: {err}"
                        );
                        None
                    }
                    Err(err) => {
                        log::warn!(
                            "[target-speaker] owner-only final unavailable; provider primary remains a candidate: {err}"
                        );
                        None
                    }
                };
                primary_result
            }
            #[cfg(not(all(target_os = "windows", feature = "target-speaker-extraction")))]
            {
                primary_result
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
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
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
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
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
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
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
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
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
                        transition_pipeline_error_if_session_matches(inner, current_session_id);
                        return Ok(());
                    }
                    log::error!("[coord] Foundry Local Whisper transcribe failed: {e:#}");
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
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
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
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
                    if source_integrity_must_stop(
                        inner,
                        current_session_id,
                        source_integrity_ledger.as_ref(),
                        "provider_error_before_error_close",
                    ) {
                        return Ok(());
                    }
                    finish_dictation_timeout(inner, current_session_id, "识别超时".to_string());
                    return Err("local global timeout".to_string());
                }
            }
        }
    };

    if source_integrity_must_stop(
        inner,
        current_session_id,
        source_integrity_ledger.as_ref(),
        "after_provider_final_before_product_arbitration",
    ) {
        return Ok(());
    }

    // ASR 完成后 cancel 检查：用户在 transcribe 进行中按 Esc 时，这里就会命中。
    // 优先级高于 empty 检查 — 用户取消 → 静默丢弃，不写失败历史也不弹错误胶囊。
    if inner.state.lock().cancelled {
        log::info!("[coord] cancel detected after ASR — discarding transcript");
        restore_prepared_windows_ime_session(inner, current_session_id);
        clear_embedded_audio_stats(inner);
        return Ok(());
    }

    // Build immutable evidence candidates first. Wake-prefix stripping is a
    // deterministic normalization; candidate selection happens exactly once
    // below in `arbitrate_product_final_transcript`.
    let mut provider_primary = raw;
    let unfiltered_text = provider_primary.text.clone();
    provider_primary.text =
        filter_automatic_wake_text(inner, current_session_id, &provider_primary.text, false);
    if provider_primary.text != unfiltered_text.trim() {
        log::info!(
            "[wake-phrase] removed automatic activation prefix from final transcript session_id={} before_chars={} after_chars={}",
            current_session_id,
            unfiltered_text.chars().count(),
            provider_primary.text.chars().count()
        );
    }
    let separated_owner_candidate = separated_owner_candidate.map(|mut candidate| {
        candidate.text =
            filter_automatic_wake_text(inner, current_session_id, &candidate.text, false);
        candidate
    });

    #[cfg(any(debug_assertions, test))]
    let debug_override_candidate =
        if provider_primary.text.trim().is_empty() && !target_speaker_filter_required {
            debug_transcript_override_text().map(|text| RawTranscript {
                text,
                duration_ms: provider_primary.duration_ms,
            })
        } else {
            None
        };
    #[cfg(not(any(debug_assertions, test)))]
    let debug_override_candidate: Option<RawTranscript> = None;

    // F2 云端空转兜底重试（2026-08-09 12:47:04：包全收、音频足量、云端
    // audio_duration 正常增长，但终稿只有唤醒词残段）。终稿空且本地证据显示
    // 整段持续人声时，用 retained_pcm 向新 ASR 会话有界重试一次；仍空才走
    // 下面的 emptyTranscript 护栏。replay_retained_audio_once 自带一次性闸。
    let mut retained_audio_replay_candidate = None;
    if provider_primary.text.trim().is_empty()
        && !nonempty_transcript(&separated_owner_candidate)
        && !target_speaker_filter_required
    {
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
                        let mut replayed = replayed;
                        replayed.text = filter_automatic_wake_text(
                            inner,
                            current_session_id,
                            &replayed.text,
                            false,
                        );
                        log::info!(
                            "[coord] empty-spin retained-audio retry offered to product arbiter session_id={} chars={}",
                            current_session_id,
                            replayed.text.chars().count()
                        );
                        retained_audio_replay_candidate = Some(replayed);
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

    // Once owner filtering is required, a provider preview that was emitted
    // before the ownership decision is no longer final evidence. Clear it at
    // the same boundary that seals the candidate set; otherwise an empty
    // provider result could resurrect stale or foreign preview text.
    if target_speaker_filter_required {
        let cleared = invalidate_embedded_audio_authoritative_preview(
            inner,
            current_session_id,
            "target_filter_required",
        );
        if cleared {
            log::info!(
                "[target-speaker] invalidated pre-filter authoritative preview session_id={current_session_id}"
            );
        }
    }

    let partial_preview_candidate = current_embedded_audio_final_preview_candidate(
        inner,
        current_session_id,
        provider_primary.duration_ms,
    );

    let any_verified_base_text = !provider_primary.text.trim().is_empty()
        || nonempty_transcript(&separated_owner_candidate)
        || nonempty_transcript(&retained_audio_replay_candidate);
    let mut local_shadow_candidate = None;
    let mut local_shadow_owner_end_aligned = false;
    #[cfg(target_os = "windows")]
    if any_verified_base_text && !target_speaker_filter_required {
        if let Some((started, audio_ms, task)) = local_shadow_task.take() {
            let total_budget = Duration::from_millis(LOCAL_SHADOW_ASR_TOTAL_BUDGET_MS);
            let remaining = total_budget.saturating_sub(started.elapsed());
            if !remaining.is_zero() {
                match tokio::time::timeout(remaining, task).await {
                    Ok(Ok(Ok(shadow))) => {
                        let local = filter_automatic_wake_text(
                            inner,
                            current_session_id,
                            &shadow.text,
                            false,
                        );
                        let owner_end_aligned = volcengine_for_empty_retry
                            .as_ref()
                            .is_some_and(|asr| asr.permits_local_shadow_omission_recovery());
                        if owner_end_aligned {
                            log::info!(
                                "[coord] local shadow offered to product arbiter session_id={} local_chars={} audio_ms={} inference_ms={} total_ms={}",
                                current_session_id,
                                local.chars().count(),
                                audio_ms,
                                shadow.inference_ms,
                                started.elapsed().as_millis()
                            );
                            local_shadow_candidate = Some(local);
                            local_shadow_owner_end_aligned = true;
                        } else {
                            log::info!(
                                "[coord] local shadow discarded because owner end clocks did not align session_id={} local_chars={} audio_ms={} inference_ms={} total_ms={}",
                                current_session_id,
                                local.chars().count(),
                                audio_ms,
                                shadow.inference_ms,
                                started.elapsed().as_millis()
                            );
                        }
                    }
                    Ok(Ok(Err(error))) => {
                        log::debug!(
                            "[coord] local shadow unavailable; cloud remains authoritative session_id={current_session_id}: {error}"
                        );
                    }
                    Ok(Err(error)) => {
                        log::warn!(
                            "[coord] local shadow worker failed session_id={current_session_id}: {error}"
                        );
                    }
                    Err(_) => {
                        log::info!(
                            "[coord] local shadow exceeded {} ms total budget; cloud remains authoritative session_id={current_session_id}",
                            LOCAL_SHADOW_ASR_TOTAL_BUDGET_MS
                        );
                    }
                }
            }
        }
    }

    let product_final = arbitrate_product_final_transcript(
        ProductFinalCandidates {
            provider_primary,
            separated_owner: separated_owner_candidate,
            retained_audio_replay: retained_audio_replay_candidate,
            debug_override: debug_override_candidate,
            partial_preview: partial_preview_candidate,
            local_shadow: local_shadow_candidate,
            local_shadow_owner_end_aligned,
            target_filter_required: target_speaker_filter_required,
            primary_speaker_filtered_certified,
        },
        &enabled_hotwords(inner),
        inner.prefs.get().remove_filler_words,
    );
    log::info!(
        "[coord] product final sealed authority={} chars={} filter_required={} local_shadow_recovered={}",
        product_final.authority.label(),
        product_final.transcript.text.chars().count(),
        target_speaker_filter_required,
        product_final.local_shadow_recovered
    );
    let mut raw = product_final.transcript;

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
            let _ = publish_embedded_ble_wake_only_expired(inner, current_session_id);
            clear_automatic_wake_text_guard(inner);
            clear_embedded_audio_preview_session(inner, current_session_id);
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
        let _ = publish_embedded_ble_asr_final(
            inner,
            current_session_id,
            true,
            Some("没有识别到语音".to_string()),
        );
        restore_prepared_windows_ime_session(inner, current_session_id);
        schedule_empty_transcript_capsule_idle(inner, current_session_id);
        return Err("ASR returned empty transcript".to_string());
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
    // Allocate the delivery correlation ID before any possible streaming
    // typer send. It is not lifecycle state; it only ties the external
    // submission attempt to its later history/final-result closeout.
    let delivery_id = Uuid::new_v4().to_string();

    let prepared_delivery = if translation_active {
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
        PreparedDeliveryText {
            intended_text: p.clone(),
            submitted_text: None,
            final_text: p,
            polish_error: e,
            already_streamed: false,
        }
    } else if (llm_auth_blocked || llm_stall_blocked) && needs_llm_polish {
        log::info!(
            "[coord] LLM circuit open (auth={llm_auth_blocked} stall={llm_stall_blocked}); inserting raw transcript without polish wait (raw_chars={})",
            raw.text.chars().count()
        );
        PreparedDeliveryText {
            intended_text: raw.text.clone(),
            submitted_text: None,
            final_text: raw.text.clone(),
            polish_error: None,
            already_streamed: false,
        }
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
            &delivery_id,
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
        PreparedDeliveryText {
            intended_text: p.clone(),
            submitted_text: None,
            final_text: p,
            polish_error: e,
            already_streamed: false,
        }
    };

    // 非流式分支（翻译/熔断/一次性）：预热用不上，取消丢弃。
    if let Some(prefetch) = polish_prefetch {
        prefetch.cancel.store(true, Ordering::SeqCst);
    }

    let PreparedDeliveryText {
        intended_text,
        submitted_text: mut pre_submitted_text,
        final_text,
        polish_error,
        already_streamed,
    } = prepared_delivery;
    let intended_text = finalize_polished_text(
        intended_text,
        translation_active,
        raw_uses_llm,
        mode,
        &polish_error,
        chinese_script_preference,
        &correction_rules,
        already_streamed,
    );
    let polished = finalize_polished_text(
        final_text,
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
        // 丢弃输出:组字里已流出来的预览一并清掉,文档不留半截。
        end_streaming_composition_session(inner, current_session_id, true);
        restore_prepared_windows_ime_session(inner, current_session_id);
        return Ok(());
    }

    // r23 自愈：focus_target 与其标题在会话开始原子成对抓取；存值 HWND 死亡时
    // 按这对标题识别真实输入窗口。
    let (focus_target, focus_target_title) = {
        let state = inner.state.lock();
        (state.focus_target, state.focus_target_title.clone())
    };
    let focus_ready_for_paste =
        restore_focus_target_if_possible(focus_target, focus_target_title.as_deref());
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
    // Pause-early-delivery（2026-09-22 跟手①）：句末稳定停顿可能已把稳定
    // 前缀当场上屏。按 stability key 对齐终稿只派发余量；云端改写/收缩了
    // 已交付前缀时跳过第二次插入（宁可少不可重——H 族），早期文本保留在屏。
    // 粘滞底线在 take 之前读：本会话只要上屏过早期文本，就算可对账前缀
    // 丢失也绝不允许终稿整段重贴（2026-09-23 用户实锤"一毛一样粘贴两次"）。
    let pause_early_sticky = pause_early_ever_delivered(inner, current_session_id);
    let pause_early_delivered = take_pause_early_delivery(inner, current_session_id);
    let pause_early_outcome = pause_early_delivered.as_ref().map(|(display, key)| {
        let remainder = pause_early_final_remainder(&polished, key);
        // 改写=云端修订：按 LCP 补回尾巴保内容（0e9b79fc 丢 16 字的教训）。
        // 2026-09-22 16:47 修正：不再限定干净会话——polished 已过归属仲裁
        // （旁人内容在上游已切），补的尾巴与不走停顿落屏时的终稿同文；
        // 干扰会话维持丢弃只会吞你自己的字（08:47:14 交付 56/57 字实锤）。
        let recovery = if remainder.is_none() {
            pause_early_mismatch_recovery_tail(&polished, key)
        } else {
            None
        };
        (display, remainder, recovery)
    });
    let (insert_text, skip_dispatch) = match pause_early_outcome.as_ref() {
        Some((_, Some(remainder), _)) if remainder.is_empty() => (polished.clone(), true),
        Some((_, Some(remainder), _)) => (remainder.clone(), false),
        Some((_, None, Some(recovery))) if !recovery.is_empty() => (recovery.clone(), false),
        Some(_) => (polished.clone(), true),
        // 账本空但本会话确实上屏过早期文本：取消终稿整段粘贴，早期文本
        // 保留在屏（用户拍板"完成那次可以取消"——双写比少交付更糟）。
        None if pause_early_sticky => (polished.clone(), true),
        None => (polished.clone(), false),
    };
    // 组字流式终局(2026-09-22 切片3):composition 活着 → 余量/恢复尾
    // stream_commit 落定(整段已覆盖传空串清残留;改写无恢复 → cancel 清组字,
    // 今日"早期文本保留,尾巴丢弃"语义)。失败则驱动已降级清场,落回常规
    // 派发。None = 本会话组字未启用/已结束,走原路径。
    let streaming_finalized = if streaming_composition_active(inner, current_session_id) {
        let op_text: Option<String> = match pause_early_outcome.as_ref() {
            Some((_, Some(remainder), _)) if remainder.is_empty() => Some(String::new()),
            Some((_, Some(remainder), _)) => Some(remainder.clone()),
            Some((_, None, Some(recovery))) if !recovery.is_empty() => Some(recovery.clone()),
            Some(_) => None,
            // 组字路由同样受粘滞底线约束：账本丢失但早期文本已 commit 到
            // 组字 → 取消 commit（清组字残留），绝不整段重写。
            None if pause_early_sticky => None,
            None => Some(polished.clone()),
        };
        let finalized = match op_text.as_deref() {
            Some(text) => streaming_composition_finalize(inner, current_session_id, text).await,
            None => {
                streaming_composition_finalize_cancel(inner, current_session_id).await
            }
        };
        if !finalized {
            log::warn!(
                "[coord] streaming-composition finalize degraded session_id={current_session_id}; falling back to regular dispatch"
            );
        }
        Some(finalized)
    } else {
        None
    };
    // 流式键入和非 TSF 输入只能证明事件已发出。自动提交要求 TSF 确认原目标接受文本。
    let delivery_request = DeliveryRequest {
        session_id: current_session_id,
        delivery_id: delivery_id.clone(),
        text: insert_text.clone(),
    };
    // async move 闭包需要自己的标题副本；外层 3998 的调用还要用原值。
    let focus_title_for_send = focus_target_title.clone();
    // 组字污染态(降级且清场也失败):文档可能有残留组字,任何粘贴都会重复,
    // 终稿宁可少交付也不双写(H 族教训)。
    let streaming_contaminated = streaming_finalized != Some(true)
        && streaming_composition_contaminated(inner, current_session_id);
    let delivery_submission = if streaming_finalized == Some(true) {
        log::info!(
            "[coord] streaming-composition finalized session_id={current_session_id} chars={} route=streaming",
            insert_text.chars().count()
        );
        DeliverySubmission {
            status: InsertStatus::Inserted,
            target_confirmed: true,
            route: DeliveryRoute::Streaming,
            submitted_text: pause_early_delivered
                .as_ref()
                .map(|(display, _)| format!("{display}{insert_text}"))
                .or_else(|| Some(insert_text.clone())),
        }
    } else if streaming_contaminated {
        log::warn!(
            "[coord] streaming-composition contaminated at final — dispatch skipped to avoid duplication (delivered_chars={} final_chars={})",
            pause_early_outcome
                .as_ref()
                .map(|(display, _, _)| display.chars().count())
                .unwrap_or(0),
            polished.chars().count()
        );
        DeliverySubmission {
            status: InsertStatus::PasteSent,
            target_confirmed: false,
            route: DeliveryRoute::Paste,
            submitted_text: pause_early_delivered
                .as_ref()
                .map(|(display, _)| display.clone()),
        }
    } else if skip_dispatch {
        if pause_early_outcome.is_none() && pause_early_sticky {
            log::warn!(
                "[coord] pause-early-delivery final full-paste cancelled: ledger lost but early text is on screen (final_chars={}) — early text stays",
                polished.chars().count()
            );
        } else if pause_early_outcome
            .as_ref()
            .is_some_and(|(_, remainder, _)| remainder.is_some())
        {
            log::info!(
                "[coord] pause-early-delivery final: prefix already covered full text chars={}",
                polished.chars().count()
            );
        } else {
            log::warn!(
                "[coord] pause-early-delivery final skipped: cloud rewrote delivered prefix (delivered_chars={} final_chars={} clean={}) — early text stays, tail dropped",
                pause_early_outcome
                    .as_ref()
                    .map(|(display, _, _)| display.chars().count())
                    .unwrap_or(0),
                polished.chars().count(),
                pause_early_final_clean,
            );
        }
        DeliverySubmission {
            status: InsertStatus::PasteSent,
            target_confirmed: false,
            route: DeliveryRoute::Paste,
            submitted_text: pause_early_delivered
                .as_ref()
                .map(|(display, _)| display.clone()),
        }
    } else {
        if pause_early_outcome.is_some() {
            if pause_early_outcome
                .as_ref()
                .is_some_and(|(_, remainder, recovery)| remainder.is_none() && recovery.is_some())
            {
                log::info!(
                    "[coord] pause-early-delivery final mismatch recovered via lcp tail chars={} of final {}",
                    insert_text.chars().count(),
                    polished.chars().count()
                );
            } else {
                log::info!(
                    "[coord] pause-early-delivery final remainder chars={} of total {}",
                    insert_text.chars().count(),
                    polished.chars().count()
                );
            }
        }
        dispatch_delivery_request(
            delivery_request.clone(),
            DeliveryDispatchPolicy {
                already_streamed,
                wayland_session,
                allow_clipboard_fallback,
                focus_ready_for_paste,
                allow_foreground_insert_fallback,
            },
            pre_submitted_text.take(),
            |operation, request| async move {
                let delivery_id = request.delivery_id;
                let polished = request.text;
                match operation {
                    DeliveryExternalOperation::CopyOnly => DeliveryExternalResult {
                        status: inner.inserter.copy_fallback(&polished),
                        target_confirmed: false,
                        route: DeliveryRoute::CopyOnly,
                        submitted_text: None,
                    },
                    DeliveryExternalOperation::OriginalTarget
                    | DeliveryExternalOperation::ForegroundFallback => {
                        #[cfg(target_os = "windows")]
                        {
                            // Re-validate immediately before the only external
                            // input operation. The earlier policy snapshot can be
                            // stale because polishing/finalization is async; do
                            // not let the current foreground (for example a
                            // terminal used by a runner) become an accidental
                            // delivery target.
                            if matches!(operation, DeliveryExternalOperation::OriginalTarget)
                                && !restore_focus_target_if_possible(
                                    focus_target,
                                    focus_title_for_send.as_deref(),
                                )
                            {
                                log::warn!(
                                    "[delivery] original target could not be restored at send time; refusing foreground insertion session_id={} focus_target={focus_target:?}",
                                    current_session_id
                                );
                                return DeliveryExternalResult {
                                    status: InsertStatus::Failed,
                                    target_confirmed: false,
                                    route: DeliveryRoute::Failed,
                                    submitted_text: None,
                                };
                            }
                            let ime_target = if matches!(
                                operation,
                                DeliveryExternalOperation::OriginalTarget
                            ) {
                                // r23 自愈：存值 HWND 可能已死，与焦点恢复共用按标题
                                // 重解出的有效窗口，否则 TSF 目标推导拿死句柄返回 None。
                                capture_ime_submit_target_for_window(resolve_insertion_window(
                                    focus_target,
                                    focus_title_for_send.as_deref(),
                                ))
                            } else {
                                capture_ime_submit_target()
                            };
                            log::info!(
                                "[delivery] target snapshot used session_id={} operation={operation:?} focus_target={focus_target:?} ime_target={ime_target:?}",
                                current_session_id
                            );
                            let result = insert_with_windows_ime_first(
                                inner,
                                current_session_id,
                                &delivery_id,
                                &polished,
                                restore_clipboard,
                                allow_non_tsf_insertion_fallback,
                                paste_shortcut,
                                ime_target,
                            )
                            .await;
                            DeliveryExternalResult {
                                status: result.status,
                                target_confirmed: result.target_confirmed,
                                route: result.route,
                                submitted_text: result.submitted_text,
                            }
                        }
                        #[cfg(not(target_os = "windows"))]
                        {
                            let status = if allow_clipboard_fallback {
                                inner
                                    .inserter
                                    .insert(&polished, restore_clipboard, paste_shortcut)
                            } else {
                                InsertStatus::Failed
                            };
                            DeliveryExternalResult {
                                status,
                                target_confirmed: false,
                                route: if cfg!(target_os = "macos") {
                                    DeliveryRoute::Direct
                                } else {
                                    DeliveryRoute::Paste
                                },
                                submitted_text: Some(polished),
                            }
                        }
                    }
                }
            },
        )
        .await
    };
    let status = delivery_submission.status;
    let original_target_confirmed = delivery_submission.target_confirmed;
    // 组字流式收尾:终局 commit/cancel 已做,这里只退驱动(幂等)。
    if streaming_finalized.is_some() {
        end_streaming_composition_session(inner, current_session_id, false);
    }
    restore_prepared_windows_ime_session(inner, current_session_id);

    let (clipboard_retention_satisfied, clipboard_result) = if retain_plain_dictation {
        retain_final_clipboard_with_foreground_budget(inner, current_session_id, &polished).await
    } else {
        (true, "disabled")
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
        if !restore_focus_target_if_possible(focus_target, focus_target_title.as_deref()) {
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
    let delivery_fact =
        finalize_delivery_fact(delivery_request, intended_text, delivery_submission, || {
            inner
                .history
                .append_with_retention(
                    session,
                    prefs_snapshot.history_retention_days,
                    prefs_snapshot.history_max_entries,
                )
                .map_err(|error| error.to_string())
        });
    debug_assert_eq!(delivery_fact.session_id, current_session_id);
    debug_assert_eq!(delivery_fact.status, status);
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
    if status == InsertStatus::SubmittedUnconfirmed {
        Some("insertUnconfirmed")
    } else if wayland_session && status == InsertStatus::Failed {
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
