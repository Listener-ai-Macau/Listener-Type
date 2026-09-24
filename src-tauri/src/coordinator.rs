//! Dictation coordinator.
//!
//! Mirrors the Swift `DictationCoordinator` state machine. Single owner of
//! session state. Receives hotkey edges, drives recorder + ASR + polish +
//! insertion, persists history, emits `capsule:state` events to the capsule
//! window.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{async_runtime, AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use uuid::Uuid;

#[cfg(target_os = "windows")]
use crate::asr::local::{foundry, FoundryLocalRuntime, FoundryLocalWhisperAsr};
use crate::asr::{
    BailianCredentials, BailianRealtimeASR, DictionaryHotword, RawTranscript,
    VolcengineCredentials, VolcengineStreamingASR, WhisperBatchASR,
};
use crate::combo_hotkey::{ComboHotkeyError, ComboHotkeyEvent, ComboHotkeyMonitor};
use crate::coordinator_state::{
    begin_recording_abort_before_restore, begin_session_state, finish_starting_session_state,
    new_session_id, publish_abort_idle_after_restore, BeginOutcome, DictationUiState, SessionId,
    SessionPhase, SessionState, StartupRaceStatus,
};
use crate::hotkey::{HotkeyEvent, HotkeyMonitor};
use crate::insertion::TextInserter;
use crate::persistence::{
    sync_style_pack_preferences, CorrectionRuleStore, CredentialAccount, CredentialsVault,
    DictionaryStore, HistoryStore, PreferencesStore, StylePackStore,
};

use crate::llm_gemini::{GeminiConfig, GeminiProvider};
use crate::polish::{
    http_client_builder_with_proxy, ActiveLLMProvider, CodexOAuthConfig, CodexOAuthLLMProvider,
    LLMError, OpenAICompatibleConfig, OpenAICompatibleLLMProvider, ProviderProxyConfig,
    CODEX_DEFAULT_MODEL, CODEX_OAUTH_PROVIDER_ID,
};
use crate::qa_hotkey::{QaHotkeyError, QaHotkeyEvent, QaHotkeyMonitor};
use crate::recorder::{Recorder, RecorderError};
use crate::selection::capture_selection;
#[cfg(target_os = "windows")]
use crate::types::PasteShortcut;
use crate::types::{
    CapsuleState, ChineseScriptPreference, DeviceCustomKeyAction, DeviceCustomKeyGesture,
    DeviceCustomKeyId, DeviceCustomKeyMapping, DeviceKnobRotationAction, DictationInputSource,
    DictationSession, HotkeyCapability, HotkeyStatus, HotkeyStatusState, InsertStatus,
    OutputLanguagePreference, PolishMode, ShortcutBinding,
};
#[cfg(target_os = "windows")]
use crate::windows_ime_ipc::ImeSubmitTarget;
#[cfg(target_os = "windows")]
use crate::windows_ime_session::{PreparedWindowsImeSession, WindowsImeSessionController};

mod dictation;
mod qa;
mod recording_gate;
mod resources;
mod source_integrity;
mod support;

const EMBEDDED_BLE_RETRY_FAST_DELAY: Duration = Duration::from_millis(200);
// After device reboot / brief link loss, retry notify quickly (was 1s floor).
const EMBEDDED_BLE_RETRY_BASE_DELAY: Duration = Duration::from_millis(200);
const EMBEDDED_BLE_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);
const EMBEDDED_BLE_RETRY_LONG_DELAY: Duration = Duration::from_secs(2);
const EMBEDDED_BLE_RETRY_OFFLINE_DELAY: Duration = Duration::from_secs(180);
const EMBEDDED_BLE_RETRY_NOISY_CCCD_DELAY: Duration = Duration::from_secs(3);
const EMBEDDED_BLE_RETRY_OTA_DEFER_DELAY: Duration = Duration::from_secs(10);
const EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD: u32 = 6;
const EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD: u32 = 3;
const EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN: Duration = Duration::from_secs(600);
const EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON: &str = "background stale pairing cleanup";
const EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON: &str =
    "background direct GATT instability pairing recovery";
const EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON: &str =
    "Type automatic pairing recovery after stale cleanup";
const EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON: &str = "manual Windows pairing removal";
const EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON: &str = "hardware recovery pairing window";
const EMBEDDED_BLE_STALE_NATIVE_HID_RECOVERY_WAIT_REASON: &str =
    "stale native HID recovery advertisement wait";
const EMBEDDED_BLE_EC11_HARDWARE_RECOVERY_NOTICE: &str =
    "Listener EC11 hardware recovery notice received before pairing reset";
const EMBEDDED_BLE_TYPE_RECOVERY_PAIRING_SETTLE: Duration = Duration::from_millis(2600);
// Windows can report the freshly paired device as absent while it rebuilds its
// BLE/HID service graph. Do not mistake that short post-PairAsync interval for
// a user-driven manual delete and reopen the recovery window.
const EMBEDDED_BLE_TYPE_PAIRASYNC_STARTUP_GUARD: Duration = Duration::from_secs(20);
const EMBEDDED_BLE_MANUAL_UNPAIR_CONFIRM_DELAY: Duration = Duration::from_millis(350);
const EMBEDDED_BLE_RECOVERY_PAIRING_ADV_SCAN_TIMEOUT: Duration = Duration::from_secs(4);
const EMBEDDED_BLE_PROBE_RECOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const EMBEDDED_BLE_PROBE_RECOVERY_POLL: Duration = Duration::from_millis(100);
const EMBEDDED_BLE_WAKE_RECOVERY_TIMEOUT: Duration = Duration::from_secs(12);
const EMBEDDED_BLE_IDLE_AUDIO_WAKE_TARGET: Duration = Duration::from_millis(50);
const EMBEDDED_BLE_RECORDING_CONTROL_READY_TIMEOUT: Duration = Duration::from_secs(5);
const EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const EMBEDDED_BLE_PAIRING_CONFIRMATION_HOLD: Duration = Duration::from_secs(180);
const EMBEDDED_BLE_PAIRING_CONFIRMATION_POLL: Duration = Duration::from_secs(3);
// A user can re-pair Listener to this Windows host after a cross-host recovery
// handoff. Poll only while the old host is deliberately passive; a completed
// local Windows pairing is the sole permission to rebuild GATT/notify.
// The monitor only lives inside the bounded manual-pairing confirmation window,
// so a sub-second cadence is cheap; each pass is dominated by the blocking PnP
// query itself. A 3 s poll added up to ~5 s of avoidable latency between the
// explicit local pairing evidence and the background notify restore.
const EMBEDDED_BLE_PASSIVE_LOCAL_REATTACH_POLL: Duration = Duration::from_millis(250);
const EMBEDDED_BLE_PAIRING_GATT_REBUILD_TIMEOUT: Duration = Duration::from_secs(20);
const EMBEDDED_BLE_POWER_CYCLE_AUDIO_RECOVERY_TARGET: Duration = Duration::from_secs(3);
const DEVICE_KEY_BLE_PENDING_ACTION_TTL: Duration = Duration::from_secs(15);
const EXTRA_ASR_HOTWORDS_ENV: &str = "LISTENER_TYPE_EXTRA_ASR_HOTWORDS";
const EMBEDDED_BLE_WAKE_GUIDANCE_MESSAGE: &str =
    "Listener BLE 正在重连。若设备处于离线状态，请按 KEY4/唤醒键，再重试；仍失败可导出诊断。";

#[cfg(test)]
use dictation::dictation_error_code;
use dictation::{
    acknowledge_automatic_wake_capsule_visible, begin_session, cancel_session, end_session,
    handle_pressed, handle_pressed_edge, handle_released_edge, hidden_automatic_candidate_active,
    note_device_key_dictation_start_intent, request_embedded_audio_stop_feedback,
    request_embedded_ble_recording_stop_from_host, request_hidden_automatic_candidate_promotion,
    request_stop_during_starting, submit_embedded_audio_ble_once, submit_embedded_audio_ble_stream,
    submit_embedded_audio_ble_stream_background, submit_embedded_audio_file,
    submit_embedded_audio_notifications, submit_embedded_audio_streaming_file,
    submit_embedded_audio_streaming_notifications, HOTKEY_DEBOUNCE,
};
use qa::{close_qa_panel, handle_qa_hotkey_pressed, QaPhase, QaSessionState};
#[allow(unused_imports)]
use support::{
    CapsuleFrontendRequest, CapsuleLayoutState, CapsuleUiThrottleState, CapsuleWindowRequest,
    DeferredAsrBridge,
};
// Re-export helpers so sibling modules / tests (`use super::*`) keep working.
#[allow(unused_imports)]
use support::{
    capture_focus_target, capture_focus_target_with_title, capture_frontmost_app, emit_capsule,
    emit_capsule_for_session,
    emit_capsule_with_session, enabled_phrases, listening_session_has_no_current_asr,
    local_qwen_transcribe_timeout, publish_dictation_capsule, publish_dictation_transition,
    restore_focus_target_if_possible, schedule_capsule_idle, startup_race_status_for_starting,
    transition_pipeline_error_if_session_matches, CAPSULE_ACTIONABLE_ERROR_HIDE_DELAY_MS,
    CAPSULE_AUTO_HIDE_DELAY_MS, CAPSULE_EMPTY_TRANSCRIPT_HIDE_DELAY_MS,
    CAPSULE_STREAM_ERROR_HIDE_DELAY_MS, CAPSULE_SUCCESS_HIDE_DELAY_MS,
    COORDINATOR_GLOBAL_TIMEOUT_SECS,
};
#[cfg(target_os = "windows")]
#[allow(unused_imports)]
use support::{
    capture_ime_submit_target, capture_ime_submit_target_for_window,
    foundry_audio_transcribe_timeout_duration, resolve_insertion_window, windows_hwnd_is_present,
};
// Tests still need these symbols in the parent namespace.
#[allow(unused_imports)]
use crate::coordinator_state::{
    publishable_dictation_snapshot, startup_race_status, DictationSnapshot, DictationTransition,
};
#[allow(unused_imports)]
use crate::types::CapsulePayload;
#[cfg(test)]
use resources::discard_startup_resources_for_session;
use resources::{
    acquire_recording_mute, release_recording_mute, selected_microphone_device_name,
    stop_microphone_preview_monitor, stop_qa_recorder, SessionResource, SharedRecordingMuteState,
};

enum ActiveAsr {
    Volcengine(Arc<VolcengineStreamingASR>),
    Whisper(Arc<WhisperBatchASR>),
    Bailian(Arc<BailianRealtimeASR>),
    #[cfg(target_os = "windows")]
    FoundryLocalWhisper(Arc<FoundryLocalWhisperAsr>),
    /// 本地 Qwen3-ASR；只在 macOS + 模型已下载时可达。
    #[cfg(target_os = "macos")]
    Local(Arc<crate::asr::local::LocalQwenAsr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceKeyBleRecordingControlDecision {
    Start,
    IgnoreStarting {
        session_id: SessionId,
        elapsed_ms: u64,
    },
    Stop {
        session_id: SessionId,
        phase: SessionPhase,
    },
}

impl DeviceKeyBleRecordingControlDecision {
    fn control_session(self) -> Option<(SessionId, SessionPhase)> {
        match self {
            DeviceKeyBleRecordingControlDecision::Start => None,
            DeviceKeyBleRecordingControlDecision::IgnoreStarting { session_id, .. } => {
                Some((session_id, SessionPhase::Starting))
            }
            DeviceKeyBleRecordingControlDecision::Stop { session_id, phase } => {
                Some((session_id, phase))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingDeviceKeyBleActionKind {
    Start,
    Stop,
}

impl PendingDeviceKeyBleActionKind {
    fn label(self) -> &'static str {
        match self {
            PendingDeviceKeyBleActionKind::Start => "start",
            PendingDeviceKeyBleActionKind::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingDeviceKeyBleAction {
    kind: PendingDeviceKeyBleActionKind,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    queued_at: Instant,
}

fn asr_transcribe_uses_global_timeout(asr: &ActiveAsr) -> bool {
    match asr {
        #[cfg(target_os = "windows")]
        ActiveAsr::FoundryLocalWhisper(_) => false,
        _ => true,
    }
}

pub struct Coordinator {
    inner: Arc<Inner>,
}

#[derive(Clone, Debug)]
struct AutomaticWakeGuard {
    session_id: SessionId,
    phrase: String,
    latest_audio_ms: u64,
    initial_body_wait_until_audio_ms: Option<u64>,
    /// Wall-clock twin of the audio deadline. Provider coverage can stall,
    /// so the initial body grace must never depend on an audio timestamp
    /// that may stop advancing.
    initial_body_wait_started_at: Option<Instant>,
    body_started: bool,
    /// Wall-clock moment the body latch first flipped true. r46f: the user's
    /// natural post-wake pause (~1.6 s) let the wake-phrase-armed 1 s inactive
    /// deadline fire 40 ms before the first body preview landed; the dispatch
    /// guard uses this timestamp to give a just-started body its contract
    /// window instead of inheriting the wake-era deadline.
    body_started_at: Option<Instant>,
    /// Stop boundary is latched before ASR finalization. Late provider text
    /// must not retroactively start a wake-only body.
    stop_requested: bool,
}

struct ProviderProgressGuard {
    session_id: SessionId,
    provider_audio_ms: u64,
    last_advanced_at: Instant,
}

struct Inner {
    app: Mutex<Option<AppHandle>>,
    history: HistoryStore,
    prefs: PreferencesStore,
    style_packs: StylePackStore,
    vocab: DictionaryStore,
    correction_rules: CorrectionRuleStore,
    inserter: TextInserter,
    #[cfg(target_os = "windows")]
    windows_ime: WindowsImeSessionController,
    #[cfg(target_os = "windows")]
    prepared_windows_ime_session: Arc<Mutex<Vec<PreparedWindowsImeSessionSlot>>>,
    state: Mutex<SessionState>,
    /// Single product recording lifecycle shared by BLE actor and ASR endpoint
    /// callbacks. Evidence paths may be concurrent; transitions are not.
    recording_lifecycle: Mutex<crate::speech_decision_kernel::RecordingLifecycleController>,
    asr: Mutex<Option<SessionResource<ActiveAsr>>>,
    /// 本地 Qwen3-ASR 引擎缓存。跨会话复用，避免每次重加载 1.2GB+ 模型。
    /// 释放时机由 prefs.local_asr_keep_loaded_secs 决定。
    local_asr_cache: Arc<crate::asr::local::LocalAsrCache>,
    #[cfg(target_os = "windows")]
    foundry_local_runtime: Arc<FoundryLocalRuntime>,
    recorder: Mutex<Option<SessionResource<Recorder>>>,
    /// 当前 dictation / QA session 的 wav 归档是否真的被写到磁盘上。
    /// 由 Recorder::start 返回值 (archive_active) 写入；history.append 路径读取，
    /// 决定 DictationSession.has_audio_recording 字段。比单纯读 prefs.record_audio_for_debug
    /// 更准确：用户开了开关但路径无法创建（权限 / 磁盘满）也算 false。
    audio_archive_active: AtomicBool,
    /// 当前嵌入式 BLE 音频会话的传输统计，随同一条 dictation history 写入。
    embedded_audio_stats: Mutex<Option<crate::embedded_audio::SessionStats>>,
    /// 当前嵌入式 BLE 音频会话的 ASR 最终文本，用于 headless A2 验收。
    embedded_audio_final_result:
        Mutex<Option<crate::embedded_audio::EmbeddedAudioTranscriptResult>>,
    /// The only preview state for embedded dictation. Authoritative and
    /// provisional evidence remain distinguishable inside the reducer, but
    /// callbacks cannot mutate independent ledgers or cross session identity.
    embedded_audio_preview: Mutex<crate::speech_decision_kernel::RecordingPreviewController>,
    /// 组字流式(2026-09-22 讯飞式逐字上屏)的会话状态机与驱动命令口。
    /// phase=Disabled 时全部路径 no-op,行为与粘贴版完全一致。
    streaming_composition: Mutex<dictation::StreamingCompositionState>,
    /// Pause-early-delivery bookkeeping (2026-09-22 跟手①): text already
    /// inserted into the target mid-session at a stable sentence-terminal
    /// pause, so the final delivery only inserts the remainder. Keyed by
    /// stability key so cloud punctuation revisions cannot double-insert.
    embedded_audio_pause_early_delivery: Mutex<dictation::PauseEarlyDeliveryLedger>,
    /// Session-scoped activation-prefix and initial-body guard. Manual sessions
    /// never arm this guard.
    embedded_audio_automatic_wake_guard: Mutex<Option<AutomaticWakeGuard>>,
    /// Distinguishes a genuinely stalled provider clock from ordinary
    /// sub-second streaming lag before local endpoint fallback is allowed.
    embedded_audio_provider_progress_guard: Mutex<Option<ProviderProgressGuard>>,
    /// One verified terminal wake may start a fresh body-only firmware capture
    /// when the VAD segment ended before it contained usable post-wake audio.
    embedded_audio_terminal_wake_continuation:
        Mutex<Option<dictation::TerminalWakeContinuation>>,
    /// 最近一次用于录音胶囊的嵌入式 BLE PCM 电平。ASR partial preview 到达时沿用它，
    /// 避免文字刷新把音量动画刷成 0。
    embedded_audio_last_capsule_level: Mutex<f32>,
    /// 嵌入式 BLE 收到停止包后锁存胶囊的停止反馈。SessionPhase 仍保持 Listening，
    /// 让 end_session 接管最终处理，同时避免尾包 / partial preview 把 UI 刷回 Recording。
    embedded_audio_stop_feedback_latched: AtomicBool,
    /// When stop→Transcribing feedback latches, record session + Instant so the
    /// completion path can log stop_to_done_ms for UX latency observability.
    dictation_stop_feedback_at: Mutex<Option<(SessionId, Instant)>>,
    /// Listener BLE 输入源的后台订阅代次。设置变化时递增，旧监听循环会自然退出。
    embedded_ble_listener_generation: AtomicU64,
    /// Firmware OTA 正在独占 BLE data plane。期间不要自动重启后台音频监听，避免抢占
    /// 同一个 Windows GATT device/session。
    embedded_ble_ota_active: AtomicBool,
    /// 当前 Listener BLE 后台订阅的取消旗标。刷新输入源或退出时主动置位，
    /// 避免旧 WinRT notify 订阅等待 60s 超时后才释放设备。
    embedded_ble_listener_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// 与当前后台订阅取消旗标绑定。仅固件确认的 BLE 改名会让旧连接马上被设备端
    /// 终止，此时跳过旧 CCCD 写入，避免 Windows 对已失效连接的额外等待。
    embedded_ble_listener_leave_cccd_enabled_on_cancel:
        Mutex<Option<(Arc<AtomicBool>, Arc<AtomicBool>)>>,
    /// 当前 Listener BLE 后台订阅已完成 CCCD notify 写入，可以接收设备音频。
    embedded_ble_listener_ready: AtomicBool,
    /// 后台订阅真正就绪时立即唤醒 OTA / 录音恢复等待者，避免 100ms 轮询把
    /// 已完成的 TYPE:READY 人为延后到下一次检查。
    embedded_ble_listener_ready_notification: Notify,
    /// 设备键从 Idle 唤醒音频时，指定下一代后台监听跳过慢速的启动期手动解配预检。
    /// 能收到这枚物理键已证明本机的原生 HID 配对仍在，不能再为重复 PnP 枚举阻塞
    /// 首次录音；代次绑定，避免旧监听循环误消费该唤醒请求。
    embedded_ble_device_key_wake_generation: AtomicU64,
    /// 已确认 OTA 服务重新出现后，指定下一代后台监听跳过一次重复的 Windows HID
    /// 枚举。OTA 前的活动连接和确认后的服务探测共同证明本机配对仍可用；若随后的
    /// GATT 打开失败，常规丢失配对恢复仍会运行。
    embedded_ble_ota_recovery_generation: AtomicU64,
    /// OTA 恢复成功后只显示一次灰色“Listener 音频已恢复”胶囊。它与预检跳过
    /// 分开消费，避免重连次数较多时被通用防刷屏规则吞掉。
    embedded_ble_ota_recovery_capsule_generation: AtomicU64,
    /// Type-controlled silent recovery may suppress its intermediate capsule;
    /// physical EC11 recovery is native Windows Swift Pair and never arms this flag.
    embedded_ble_type_recovery_audio_capsule_pending: AtomicBool,
    /// 启动时必须先把 Type 目标名同步到固件广播名，再允许后台 BLE 监听启动。
    /// 否则前端/托盘的早期 refresh 会用旧 prefs 名字扫一轮，造成第一次连接失败。
    embedded_ble_startup_name_sync_done: AtomicBool,
    /// 最近一次非空闲的 Listener BLE 后台订阅错误。Overview 读取它来区分
    /// 启动阶段的 CCCD/notify/subscription 失败，这类失败不会生成历史会话。
    embedded_ble_listener_last_error: Mutex<Option<String>>,
    /// Windows Swift Pair / Bluetooth 设置确认期间暂停后台 GATT 订阅。Windows 会在
    /// GATT service discovery / MaintainConnection 时主动连接设备；若此时旧 bond 正在
    /// 清理，会造成系统 UI 反复已连接/未连接。
    embedded_ble_pairing_hold_until: Mutex<Option<Instant>>,
    /// 配对 hold 的代次。异步 watcher 只允许清理自己创建的 hold，避免旧任务误恢复
    /// 后一次重配对流程。
    embedded_ble_pairing_hold_generation: AtomicU64,
    /// 跨主机重配对后，旧 Type 只被动观察本机 Windows 配对/HID 证据。没有这层
    /// 观察时，用户稍后手动配回本机只会让一次性 GATT probe 成功，常驻 notify
    /// listener 却会永远停在取消态。
    embedded_ble_passive_local_reattach_active: AtomicBool,
    /// 设备断电后重新出现的本机 HID 时刻。notify ready 消费该值并输出冷启动音频
    /// 恢复指标；它不参与连接决策。
    embedded_ble_power_cycle_hid_observed_at: Mutex<Option<Instant>>,
    /// Type 控制的 Windows 配对恢复流程正在运行。覆盖 UnpairAsync / PairAsync /
    /// fresh GATT link-check 的完整窗口，避免第二个后台 cleanup 在第一轮刚配好时
    /// 又删掉 Windows link key，造成系统 UI 在已连接/未连接之间反复跳。
    embedded_ble_pairing_recovery_active: AtomicBool,
    /// Type 自动 PairAsync 成功后的短服务重建窗口。Windows 此时可能暂时还没有
    /// 暴露 HID/GATT 配对证据，不能被启动期手动删除探测误判为用户主动解配。
    embedded_ble_type_pairasync_startup_guard_until: Mutex<Option<Instant>>,
    /// 用户动作触发 BLE 恢复时的结构化快照。用于 Overview、胶囊错误文案和诊断导出，
    /// 避免把底层 transport/notify 错误直接暴露给用户。
    embedded_ble_wake_recovery: Mutex<EmbeddedBleWakeRecoverySnapshot>,
    /// 设备键在 Listener BLE notify/control 尚未恢复时触发的“开始录音”意图。
    /// notify ready 后补发一次，避免低功耗唤醒场景必须按第二下。
    device_key_pending_ble_action: Mutex<Option<PendingDeviceKeyBleAction>>,
    /// Listener BLE session actor state. Embedded BLE paths must take a
    /// monotonically ordered actor ticket here before mutating the shared
    /// dictation FSM, so BLE packets, ASR callbacks, stop/cancel commands, and
    /// timeout/final events are replayable from one serialized history.
    embedded_ble_session_actor: Mutex<EmbeddedBleSessionActorState>,
    /// 当前嵌入式 BLE 抓音循环的取消标志。胶囊取消走 cancel_session 时会置位，
    /// 让 blocking BLE notify loop 及时退出。
    embedded_ble_cancel_flag: Mutex<Option<Arc<AtomicBool>>>,
    /// 预热润色：endpoint 触发时用当时的预览文本提前发起 LLM 润色，与 ASR
    /// 终稿等待并行。终稿与预热输入一致才被采用（不一致即取消丢弃，回退
    /// 正常路径）。只存一份当前会话的预热。
    polish_prefetch: Mutex<Option<(SessionId, PolishPrefetch)>>,
    /// 空闲 cancel 去重：phase=Idle 时第一次 cancel 已处理后置位，后续 Idle
    /// 重复的 Esc/cancel 直接跳过，不再每次 bounce background listener actor
    /// （2026-08-07 全天 1875 次 idle cancel，每次都 actor_restart + LED sync；
    /// 陈旧 session 的 cancel flag 泄漏让每次都有活可干）。非 Idle 的 cancel
    /// 会把它清零。
    idle_hotkey_cancel_sent: AtomicBool,
    recording_mute: Mutex<SharedRecordingMuteState>,
    hotkey: Mutex<Option<HotkeyMonitor>>,
    hotkey_status: Mutex<HotkeyStatus>,
    hotkey_trigger_held: AtomicBool,
    /// 防抖时间戳：handle_pressed_edge 入口检查与本字段的距离，< 250ms 的边沿直接
    /// 丢弃（误触双击 / 微动开关回弹 / 用户连点过快造成的空转写报错）。
    /// 与 `hotkey_trigger_held` 互补 —— held 防 press-without-release，本字段防
    /// press-release-press 三连过快。
    last_hotkey_dispatch_at: Mutex<Option<std::time::Instant>>,
    shortcut_recording_active: AtomicBool,
    /// 自定义组合键监听器（global-hotkey crate）。当 `prefs.hotkey.trigger == Custom` 时
    /// 代替 modifier-only 的 hotkey monitor。`None` 表示不使用自定义组合键或还没成功安装。
    combo_hotkey: Mutex<Option<ComboHotkeyMonitor>>,
    translation_hotkey: Mutex<Option<ComboHotkeyMonitor>>,
    switch_style_hotkey: Mutex<Option<ComboHotkeyMonitor>>,
    open_app_hotkey: Mutex<Option<ComboHotkeyMonitor>>,
    device_key_hotkeys: [Mutex<Option<ComboHotkeyMonitor>>; 13],
    device_key_last_dispatch_at:
        Mutex<HashMap<(DeviceCustomKeyGesture, DeviceCustomKeyId), Instant>>,
    /// 翻译模式触发标志。每次 begin_session 重置为 false；hotkey 监听器在
    /// Listening / Starting 阶段看到 Shift down 边沿时 set true。
    /// end_session 在调 polish/translate 前读这个 flag + translation_target_language
    /// 决定走哪条管线。详见 issue #4。
    translation_modifier_seen: AtomicBool,
    /// 划词语音问答（issue #118）：与 dictation hotkey 平行的全局快捷键
    /// 监听器（global-hotkey crate）。`None` 表示功能关闭或还没成功安装。
    qa_hotkey: Mutex<Option<QaHotkeyMonitor>>,
    /// QA 单独的 session 状态，与 dictation 的 SessionPhase 不冲突。
    qa_state: Mutex<QaSessionState>,
    /// 最近一次应用到 capsule 窗口的几何状态。避免录音 level tick 反复触发
    /// resize / reposition。
    capsule_layout: Mutex<Option<CapsuleLayoutState>>,
    /// 限流 capsule 窗口 show/position 操作；`capsule:state` 事件不受限，
    /// 保证电平条仍按前端可接受频率更新。
    capsule_ui_throttle: Mutex<CapsuleUiThrottleState>,
    /// Monotonic capsule payload sequence. The frontend rejects older snapshots
    /// so delayed UI events cannot overwrite newer terminal states.
    capsule_sequence: AtomicU64,
    /// Latest capsule payload for a lifecycle-safe frontend replay. The capsule
    /// WebView can be hidden/recreated independently of the main window; replay
    /// is display-only and never re-enters dictation or insertion.
    capsule_latest_payload: Mutex<Option<CapsulePayload>>,
    /// QA 用的 ASR 句柄（始终是 Volcengine 流式）。
    qa_asr: Mutex<Option<Arc<VolcengineStreamingASR>>>,
    /// QA 用的 Recorder 句柄。
    qa_recorder: Mutex<Option<Recorder>>,
    /// QA SSE 流取消标志。begin_qa_session 重置为 false；cancel_qa_session 设 true；
    /// polish::chat_completion_history_streaming 的 loop 每帧检查，true 时 break loop
    /// 避免取消后 LLM 仍 drain HTTP body 烧 token。详见 issue #161。
    qa_stream_cancelled: Arc<AtomicBool>,
    /// Coordinator 退出信号。各 hotkey supervisor loop 在每轮重试 sleep 之前会检查
    /// 此 flag；为 true 时 loop 立刻 return。生产场景里 process exit 一并 reap 所有
    /// supervisor 线程，但 integration test 和未来 RunEvent::Exit 钩子需要这条
    /// 显式退出路径。审计 3.1.2。
    shutdown: AtomicBool,
}

const DICTATION_RUNTIME_SNAPSHOT_SCHEMA: &str = "listener.dictation_runtime_snapshot.v1";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DictationRuntimeSnapshot {
    pub schema: &'static str,
    pub request_id: u32,
    pub phase: &'static str,
    pub session_id: Option<SessionId>,
    pub pending_stop: bool,
    pub cancelled: bool,
}

fn dictation_runtime_snapshot_from_state(
    request_id: u32,
    state: &SessionState,
) -> DictationRuntimeSnapshot {
    let phase = match state.phase {
        SessionPhase::Idle => "idle",
        SessionPhase::Starting => "starting",
        SessionPhase::Listening => "listening",
        SessionPhase::Processing => "processing",
        SessionPhase::Inserting => "inserting",
    };
    DictationRuntimeSnapshot {
        schema: DICTATION_RUNTIME_SNAPSHOT_SCHEMA,
        request_id,
        phase,
        session_id: (!state.session_id.is_nil()).then_some(state.session_id),
        pending_stop: state.pending_stop,
        cancelled: state.cancelled,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryRoute {
    Tsf,
    Unicode,
    Paste,
    CopyOnly,
    Streaming,
    Direct,
    Failed,
}

impl DeliveryRoute {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Tsf => "tsf",
            Self::Unicode => "unicode",
            Self::Paste => "paste",
            Self::CopyOnly => "copy_only",
            Self::Streaming => "streaming",
            Self::Direct => "direct",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryPersistenceOutcome {
    Saved,
    Failed,
}

impl DeliveryPersistenceOutcome {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Failed => "failed",
        }
    }
}

/// The immutable request handed to one external input submission attempt.
/// `delivery_id` is allocated before the attempt starts and is carried through
/// the production submission and history closeout without becoming lifecycle
/// state or part of the persisted/IPC schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryRequest {
    pub(super) session_id: SessionId,
    pub(super) delivery_id: String,
    pub(super) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliverySubmission {
    pub(super) status: InsertStatus,
    pub(super) target_confirmed: bool,
    pub(super) route: DeliveryRoute,
    pub(super) submitted_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliveryExternalOperation {
    OriginalTarget,
    ForegroundFallback,
    CopyOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryExternalResult {
    pub(super) status: InsertStatus,
    pub(super) target_confirmed: bool,
    pub(super) route: DeliveryRoute,
    pub(super) submitted_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DeliveryDispatchPolicy {
    pub(super) already_streamed: bool,
    pub(super) wayland_session: bool,
    pub(super) allow_clipboard_fallback: bool,
    pub(super) focus_ready_for_paste: bool,
    pub(super) allow_foreground_insert_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeliveryDispatchChoice {
    External(DeliveryExternalOperation),
    Immediate(DeliverySubmission),
}

fn choose_delivery_dispatch(
    policy: DeliveryDispatchPolicy,
    streamed_submitted_text: Option<String>,
) -> DeliveryDispatchChoice {
    if policy.already_streamed {
        return DeliveryDispatchChoice::Immediate(DeliverySubmission {
            // Streaming only proves that synthetic events were emitted. It
            // has no receiver acknowledgement, so do not persist the same
            // success state as a confirmed TSF commit.
            status: InsertStatus::SubmittedUnconfirmed,
            target_confirmed: false,
            route: DeliveryRoute::Streaming,
            submitted_text: streamed_submitted_text,
        });
    }
    if policy.wayland_session {
        return if policy.allow_clipboard_fallback {
            DeliveryDispatchChoice::External(DeliveryExternalOperation::CopyOnly)
        } else {
            DeliveryDispatchChoice::Immediate(DeliverySubmission {
                status: InsertStatus::Failed,
                target_confirmed: false,
                route: DeliveryRoute::Failed,
                submitted_text: None,
            })
        };
    }
    if policy.focus_ready_for_paste {
        DeliveryDispatchChoice::External(DeliveryExternalOperation::OriginalTarget)
    } else if policy.allow_foreground_insert_fallback {
        DeliveryDispatchChoice::External(DeliveryExternalOperation::ForegroundFallback)
    } else if policy.allow_clipboard_fallback {
        DeliveryDispatchChoice::External(DeliveryExternalOperation::CopyOnly)
    } else {
        DeliveryDispatchChoice::Immediate(DeliverySubmission {
            status: InsertStatus::Failed,
            target_confirmed: false,
            route: DeliveryRoute::Failed,
            submitted_text: None,
        })
    }
}

fn map_delivery_external_result(
    operation: DeliveryExternalOperation,
    result: DeliveryExternalResult,
) -> DeliverySubmission {
    let status = if result.status == InsertStatus::Inserted
        && (result.route != DeliveryRoute::Tsf
            || !matches!(operation, DeliveryExternalOperation::OriginalTarget))
    {
        // KEYEVENTF_UNICODE and explicit foreground fallback report only that
        // Windows accepted the input event. There is no receiver ack proving
        // that the intended control rendered it.
        InsertStatus::SubmittedUnconfirmed
    } else {
        result.status
    };
    let target_confirmed = matches!(operation, DeliveryExternalOperation::OriginalTarget)
        && result.route == DeliveryRoute::Tsf
        && status == InsertStatus::Inserted
        && result.target_confirmed;
    let submitted_text = if matches!(operation, DeliveryExternalOperation::CopyOnly) {
        None
    } else {
        result.submitted_text
    };
    DeliverySubmission {
        status,
        target_confirmed,
        route: if matches!(operation, DeliveryExternalOperation::CopyOnly) {
            DeliveryRoute::CopyOnly
        } else {
            result.route
        },
        submitted_text,
    }
}

/// Stable, privacy-preserving payload comparison for delivery evidence.
/// FNV-1a is applied to an explicit presence byte followed by the exact UTF-8
/// bytes, so `None` and `Some("")` cannot collide by construction.
pub(super) fn delivery_payload_digest(text: Option<&str>) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let presence = if text.is_some() { 0x01_u8 } else { 0x00_u8 };
    hash ^= presence as u64;
    hash = hash.wrapping_mul(0x100000001b3_u64);
    if let Some(text) = text {
        for byte in text.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3_u64);
        }
    }
    format!("{hash:016x}")
}

pub(super) fn delivery_payload_bytes(text: Option<&str>) -> Option<usize> {
    text.map(str::len)
}

/// Run the production's one-shot external submission closure exactly once.
/// The closure is the only replaceable boundary in offline tests; the request
/// and all closeout data use this same production path.
pub(super) async fn execute_delivery_submission<F, Fut>(
    request: DeliveryRequest,
    submit: F,
) -> DeliverySubmission
where
    F: FnOnce(DeliveryRequest) -> Fut,
    Fut: Future<Output = DeliverySubmission>,
{
    log::info!(
        "[delivery] dispatch begin session_id={} delivery_id={} text_bytes={} text_digest={}",
        request.session_id,
        request.delivery_id,
        request.text.len(),
        delivery_payload_digest(Some(&request.text))
    );
    let submission = submit(request.clone()).await;
    log::info!(
        "[delivery] dispatch end session_id={} delivery_id={} route={} status={:?} target_confirmed={} submitted_bytes={:?} submitted_digest={}",
        request.session_id,
        request.delivery_id,
        submission.route.label(),
        submission.status,
        submission.target_confirmed,
        delivery_payload_bytes(submission.submitted_text.as_deref()),
        delivery_payload_digest(submission.submitted_text.as_deref())
    );
    submission
}

/// Choose the production delivery branch, invoke only the selected external
/// system operation, and normalize its result before closeout. Tests replace
/// only `submit`; branch choice and target-confirmation mapping stay shared
/// with production.
pub(super) async fn dispatch_delivery_request<F, Fut>(
    request: DeliveryRequest,
    policy: DeliveryDispatchPolicy,
    streamed_submitted_text: Option<String>,
    submit: F,
) -> DeliverySubmission
where
    F: FnOnce(DeliveryExternalOperation, DeliveryRequest) -> Fut,
    Fut: Future<Output = DeliveryExternalResult>,
{
    let choice = choose_delivery_dispatch(policy, streamed_submitted_text);
    execute_delivery_submission(request, |request| async move {
        match choice {
            DeliveryDispatchChoice::External(operation) => {
                let result = submit(operation, request.clone()).await;
                map_delivery_external_result(operation, result)
            }
            DeliveryDispatchChoice::Immediate(submission) => submission,
        }
    })
    .await
}

/// Immutable, internal evidence for one final delivery. This deliberately is
/// not a new lifecycle state and is not part of the persisted/IPC schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliveryFact {
    pub(super) session_id: SessionId,
    pub(super) delivery_id: String,
    /// Text that the pipeline intended to deliver after the configured
    /// polish/translation fallback was resolved.
    pub(super) intended_text: String,
    /// Text actually submitted to an input mechanism. `None` means this
    /// delivery was copy-only or no input mechanism accepted a payload.
    pub(super) submitted_text: Option<String>,
    pub(super) route: DeliveryRoute,
    pub(super) status: InsertStatus,
    /// Only TSF commit (or an equivalent future explicit target acknowledgement)
    /// may set this true. SendInput/paste/streaming remain unconfirmed.
    pub(super) target_confirmed: bool,
    pub(super) persistence: DeliveryPersistenceOutcome,
    pub(super) intended_bytes: usize,
    pub(super) intended_digest: String,
    pub(super) submitted_bytes: Option<usize>,
    pub(super) submitted_digest: String,
}

pub(super) fn record_delivery_fact<F>(
    session_id: SessionId,
    delivery_id: String,
    intended_text: String,
    submitted_text: Option<String>,
    route: DeliveryRoute,
    status: InsertStatus,
    target_confirmed: bool,
    persist_history: F,
) -> DeliveryFact
where
    F: FnOnce() -> Result<(), String>,
{
    let intended_bytes = intended_text.len();
    let intended_digest = delivery_payload_digest(Some(&intended_text));
    let submitted_bytes = delivery_payload_bytes(submitted_text.as_deref());
    let submitted_digest = delivery_payload_digest(submitted_text.as_deref());
    let persistence = match persist_history() {
        Ok(()) => DeliveryPersistenceOutcome::Saved,
        Err(error) => {
            log::error!("[delivery] history append failed: {error}");
            DeliveryPersistenceOutcome::Failed
        }
    };
    let fact = DeliveryFact {
        session_id,
        delivery_id,
        intended_text,
        submitted_text,
        route,
        status,
        target_confirmed,
        persistence,
        intended_bytes,
        intended_digest,
        submitted_bytes,
        submitted_digest,
    };
    log::info!(
        "[delivery] fact session_id={} delivery_id={} route={} status={:?} target_confirmed={} persistence={} intended_bytes={} intended_digest={} submitted_bytes={:?} submitted_digest={}",
        fact.session_id,
        fact.delivery_id,
        fact.route.label(),
        fact.status,
        fact.target_confirmed,
        fact.persistence.label(),
        fact.intended_bytes,
        fact.intended_digest,
        fact.submitted_bytes,
        fact.submitted_digest,
    );
    fact
}

/// Close one production delivery with the already-used request ID and the
/// actual submission result. The history sink is invoked once and never
/// retried here; callers retain their established side-effect ordering and
/// pass the same session object they would have persisted before O3.
pub(super) fn finalize_delivery_fact<F>(
    request: DeliveryRequest,
    intended_text: String,
    submission: DeliverySubmission,
    persist_history: F,
) -> DeliveryFact
where
    F: FnOnce() -> Result<(), String>,
{
    record_delivery_fact(
        request.session_id,
        request.delivery_id,
        intended_text,
        submission.submitted_text,
        submission.route,
        submission.status,
        submission.target_confirmed,
        persist_history,
    )
}

#[cfg(target_os = "windows")]
pub(super) struct WindowsInsertionResult {
    pub(super) status: InsertStatus,
    /// Only a successful TSF submit confirms that the original target accepted the text.
    pub(super) target_confirmed: bool,
    pub(super) route: DeliveryRoute,
    pub(super) submitted_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedBleWakeRecoverySnapshot {
    pub status: EmbeddedBleWakeRecoveryStatus,
    pub user_guidance: String,
    pub recent_disconnect_reason: Option<String>,
    pub reconnect_attempts: u32,
    pub consecutive_reconnect_failures: u32,
    pub notify_subscription_state: EmbeddedBleNotifySubscriptionState,
    pub usb_powered: Option<bool>,
    pub battery_percent: Option<u8>,
    pub firmware_wake_policy: FirmwareWakePolicySnapshot,
    pub last_attempt_at: Option<String>,
    pub last_ready_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EmbeddedBleWakeRecoveryStatus {
    Idle,
    Reconnecting,
    Ready,
    NeedsWakeKey,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EmbeddedBleNotifySubscriptionState {
    Unknown,
    Opening,
    Subscribed,
    Lost,
    Failed,
    Cancelled,
}

const EMBEDDED_BLE_SESSION_ACTOR_HISTORY_LIMIT: usize = 64;
const EMBEDDED_BLE_PCM_CAPSULE_TRACE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddedBleSessionActorCommand {
    BlePacket,
    AsrPartial,
    AsrFinal,
    StartCommand,
    StopCommand,
    CancelCommand,
    Timeout,
    NotifyReady,
    NotifyCleanupDelay,
    ActorRestart,
}

impl EmbeddedBleSessionActorCommand {
    fn as_str(self) -> &'static str {
        match self {
            Self::BlePacket => "ble_packet",
            Self::AsrPartial => "asr_partial",
            Self::AsrFinal => "asr_final",
            Self::StartCommand => "start_command",
            Self::StopCommand => "stop_command",
            Self::CancelCommand => "cancel_command",
            Self::Timeout => "timeout",
            Self::NotifyReady => "notify_ready",
            Self::NotifyCleanupDelay => "notify_cleanup_delay",
            Self::ActorRestart => "actor_restart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmbeddedBleSessionActorRecord {
    seq: u64,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedBleSessionActorDiagnosticRecord {
    pub seq: u64,
    pub command: &'static str,
    pub session_id: Option<String>,
    pub detail: String,
}

impl EmbeddedBleSessionActorRecord {
    fn diagnostic(&self) -> EmbeddedBleSessionActorDiagnosticRecord {
        EmbeddedBleSessionActorDiagnosticRecord {
            seq: self.seq,
            command: self.command.as_str(),
            session_id: self.session_id.map(|id| id.to_string()),
            detail: self.detail.clone(),
        }
    }
}

#[derive(Default)]
struct EmbeddedBleSessionActorState {
    next_seq: u64,
    history: VecDeque<EmbeddedBleSessionActorRecord>,
    pcm_capsule_trace: EmbeddedBlePcmCapsuleTraceState,
    /// The live source-integrity ledger for the logical product session owned
    /// by this actor.  Keep the Arc here so endpoint tasks that finish a
    /// rotated wake-only segment inspect the exact same evidence context as
    /// the actor's normal STOP path.
    source_integrity_ledger: Option<(
        SessionId,
        Arc<std::sync::Mutex<crate::coordinator::dictation::SourceAdmissionDependencyLedger>>,
    )>,
    /// Product session whose pre-activation physical segment has ended while
    /// the actor is waiting for an optional post-activation segment.  The
    /// endpoint stop path consumes this hand-off only after firmware STOP and
    /// provider final-frame dispatch complete; a newly arrived segment clears
    /// it first.  Physical transport therefore cannot keep a logically ended
    /// wake-only session alive or capture the next wake.
    awaiting_post_activation_segment: Option<SessionId>,
}

#[derive(Debug, Default)]
struct EmbeddedBlePcmCapsuleTraceState {
    session_id: Option<SessionId>,
    last_after_stop: Option<bool>,
    last_trace_at: Option<Instant>,
}

impl EmbeddedBlePcmCapsuleTraceState {
    fn should_trace(&mut self, session_id: SessionId, after_stop: bool, now: Instant) -> bool {
        let session_changed = self.session_id != Some(session_id);
        let boundary_changed = self.last_after_stop != Some(after_stop);
        let due = self
            .last_trace_at
            .map(|last| now.duration_since(last) >= EMBEDDED_BLE_PCM_CAPSULE_TRACE_INTERVAL)
            .unwrap_or(true);

        let should_trace = session_changed || boundary_changed || due;
        if should_trace {
            self.session_id = Some(session_id);
            self.last_after_stop = Some(after_stop);
            self.last_trace_at = Some(now);
        }
        should_trace
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareWakePolicySnapshot {
    pub policy: &'static str,
    pub wake_capable_keys: &'static str,
    pub voice_key: &'static str,
    pub voice_key_deep_sleep_wake: bool,
    pub readiness: &'static str,
    pub source: &'static str,
}

impl Default for EmbeddedBleWakeRecoverySnapshot {
    fn default() -> Self {
        Self {
            status: EmbeddedBleWakeRecoveryStatus::Idle,
            user_guidance:
                "Listener BLE 空闲。按设备语音键开始录音；若设备离线，请先按 KEY4/唤醒键。"
                    .to_string(),
            recent_disconnect_reason: None,
            reconnect_attempts: 0,
            consecutive_reconnect_failures: 0,
            notify_subscription_state: EmbeddedBleNotifySubscriptionState::Unknown,
            usb_powered: None,
            battery_percent: None,
            firmware_wake_policy: FirmwareWakePolicySnapshot::current_v1(),
            last_attempt_at: None,
            last_ready_at: None,
        }
    }
}

impl FirmwareWakePolicySnapshot {
    fn current_v1() -> Self {
        Self {
            policy: "key4_only",
            wake_capable_keys: "KEY4/GPIO21",
            voice_key: "GPIO35",
            voice_key_deep_sleep_wake: false,
            readiness: "voice_key_cannot_wake_from_deep_sleep_on_current_v1_board",
            source: "Firmware 1.1 ~POWER:STATUS wake-policy contract",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionHotkeyKind {
    SwitchStyle,
    OpenApp,
    DeviceKey {
        key: DeviceCustomKeyId,
        gesture: DeviceCustomKeyGesture,
    },
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct PreparedWindowsImeSessionSlot {
    session_id: SessionId,
    prepared: PreparedWindowsImeSession,
}

impl Coordinator {
    pub fn new() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::new_with_foundry_runtime(Arc::new(FoundryLocalRuntime::new()))
        }

        #[cfg(not(target_os = "windows"))]
        {
            let history = HistoryStore::new_or_empty();
            let prefs = PreferencesStore::new().expect("preferences store init");
            let style_packs = StylePackStore::new(&prefs).expect("style pack store init");
            let vocab = DictionaryStore::new().expect("dictionary store init");
            let correction_rules = CorrectionRuleStore::new().expect("correction rule store init");

            Self {
                inner: Arc::new(Inner {
                    app: Mutex::new(None),
                    history,
                    prefs,
                    style_packs,
                    vocab,
                    correction_rules,
                    inserter: TextInserter::new(),
                    state: Mutex::new(SessionState::default()),
                    recording_lifecycle: Mutex::new(Default::default()),
                    asr: Mutex::new(None),
                    recorder: Mutex::new(None),
                    audio_archive_active: AtomicBool::new(false),
                    embedded_audio_stats: Mutex::new(None),
                    embedded_audio_final_result: Mutex::new(None),
                    embedded_audio_preview: Mutex::new(Default::default()),
                    embedded_audio_pause_early_delivery: Mutex::new(Default::default()),
                    streaming_composition: Mutex::new(Default::default()),
                    embedded_audio_automatic_wake_guard: Mutex::new(None),
                    embedded_audio_provider_progress_guard: Mutex::new(None),
                    embedded_audio_terminal_wake_continuation: Mutex::new(None),
                    embedded_audio_last_capsule_level: Mutex::new(0.0),
                    embedded_audio_stop_feedback_latched: AtomicBool::new(false),
                    dictation_stop_feedback_at: Mutex::new(None),
                    embedded_ble_listener_generation: AtomicU64::new(0),
                    embedded_ble_ota_active: AtomicBool::new(false),
                    embedded_ble_listener_cancel: Mutex::new(None),
                    embedded_ble_listener_leave_cccd_enabled_on_cancel: Mutex::new(None),
                    embedded_ble_listener_ready: AtomicBool::new(false),
                    embedded_ble_listener_ready_notification: Notify::new(),
                    embedded_ble_device_key_wake_generation: AtomicU64::new(0),
                    embedded_ble_ota_recovery_generation: AtomicU64::new(0),
                    embedded_ble_ota_recovery_capsule_generation: AtomicU64::new(0),
                    embedded_ble_type_recovery_audio_capsule_pending: AtomicBool::new(false),
                    embedded_ble_startup_name_sync_done: AtomicBool::new(false),
                    embedded_ble_listener_last_error: Mutex::new(None),
                    embedded_ble_pairing_hold_until: Mutex::new(None),
                    embedded_ble_pairing_hold_generation: AtomicU64::new(0),
                    embedded_ble_passive_local_reattach_active: AtomicBool::new(false),
                    embedded_ble_power_cycle_hid_observed_at: Mutex::new(None),
                    embedded_ble_pairing_recovery_active: AtomicBool::new(false),
                    embedded_ble_type_pairasync_startup_guard_until: Mutex::new(None),
                    embedded_ble_wake_recovery: Mutex::new(
                        EmbeddedBleWakeRecoverySnapshot::default(),
                    ),
                    device_key_pending_ble_action: Mutex::new(None),
                    embedded_ble_session_actor: Mutex::new(EmbeddedBleSessionActorState::default()),
                    embedded_ble_cancel_flag: Mutex::new(None),
                    polish_prefetch: Mutex::new(None),
                    idle_hotkey_cancel_sent: AtomicBool::new(false),
                    recording_mute: Mutex::new(SharedRecordingMuteState::new()),
                    hotkey: Mutex::new(None),
                    hotkey_status: Mutex::new(HotkeyStatus::default()),
                    hotkey_trigger_held: AtomicBool::new(false),
                    last_hotkey_dispatch_at: Mutex::new(None),
                    shortcut_recording_active: AtomicBool::new(false),
                    combo_hotkey: Mutex::new(None),
                    translation_hotkey: Mutex::new(None),
                    switch_style_hotkey: Mutex::new(None),
                    open_app_hotkey: Mutex::new(None),
                    device_key_hotkeys: std::array::from_fn(|_| Mutex::new(None)),
                    device_key_last_dispatch_at: Mutex::new(HashMap::new()),
                    translation_modifier_seen: AtomicBool::new(false),
                    qa_hotkey: Mutex::new(None),
                    qa_state: Mutex::new(QaSessionState::default()),
                    capsule_layout: Mutex::new(None),
                    capsule_ui_throttle: Mutex::new(CapsuleUiThrottleState::default()),
                    capsule_sequence: AtomicU64::new(0),
                    capsule_latest_payload: Mutex::new(None),
                    qa_asr: Mutex::new(None),
                    qa_recorder: Mutex::new(None),
                    qa_stream_cancelled: Arc::new(AtomicBool::new(false)),
                    local_asr_cache: Arc::new(crate::asr::local::LocalAsrCache::new()),
                    shutdown: AtomicBool::new(false),
                }),
            }
        }
    }

    #[cfg(target_os = "windows")]
    pub fn new_with_foundry_runtime(foundry_local_runtime: Arc<FoundryLocalRuntime>) -> Self {
        let history = HistoryStore::new_or_empty();
        let prefs = PreferencesStore::new().expect("preferences store init");
        crate::embedded_ble::set_configured_bluetooth_target_name(&prefs.get().device_ble_name);
        let style_packs = StylePackStore::new(&prefs).expect("style pack store init");
        let vocab = DictionaryStore::new().expect("dictionary store init");
        let correction_rules = CorrectionRuleStore::new().expect("correction rule store init");

        Self {
            inner: Arc::new(Inner {
                app: Mutex::new(None),
                history,
                prefs,
                style_packs,
                vocab,
                correction_rules,
                inserter: TextInserter::new(),
                windows_ime: WindowsImeSessionController::new(),
                prepared_windows_ime_session: Arc::new(Mutex::new(Vec::new())),
                state: Mutex::new(SessionState::default()),
                recording_lifecycle: Mutex::new(Default::default()),
                asr: Mutex::new(None),
                recorder: Mutex::new(None),
                audio_archive_active: AtomicBool::new(false),
                embedded_audio_stats: Mutex::new(None),
                embedded_audio_final_result: Mutex::new(None),
                embedded_audio_preview: Mutex::new(Default::default()),
                embedded_audio_pause_early_delivery: Mutex::new(Default::default()),
                streaming_composition: Mutex::new(Default::default()),
                embedded_audio_automatic_wake_guard: Mutex::new(None),
                embedded_audio_provider_progress_guard: Mutex::new(None),
                embedded_audio_terminal_wake_continuation: Mutex::new(None),
                embedded_audio_last_capsule_level: Mutex::new(0.0),
                embedded_audio_stop_feedback_latched: AtomicBool::new(false),
                dictation_stop_feedback_at: Mutex::new(None),
                embedded_ble_listener_generation: AtomicU64::new(0),
                embedded_ble_ota_active: AtomicBool::new(false),
                embedded_ble_listener_cancel: Mutex::new(None),
                embedded_ble_listener_leave_cccd_enabled_on_cancel: Mutex::new(None),
                embedded_ble_listener_ready: AtomicBool::new(false),
                embedded_ble_listener_ready_notification: Notify::new(),
                embedded_ble_device_key_wake_generation: AtomicU64::new(0),
                embedded_ble_ota_recovery_generation: AtomicU64::new(0),
                embedded_ble_ota_recovery_capsule_generation: AtomicU64::new(0),
                embedded_ble_type_recovery_audio_capsule_pending: AtomicBool::new(false),
                embedded_ble_startup_name_sync_done: AtomicBool::new(false),
                embedded_ble_listener_last_error: Mutex::new(None),
                embedded_ble_pairing_hold_until: Mutex::new(None),
                embedded_ble_pairing_hold_generation: AtomicU64::new(0),
                embedded_ble_passive_local_reattach_active: AtomicBool::new(false),
                embedded_ble_power_cycle_hid_observed_at: Mutex::new(None),
                embedded_ble_pairing_recovery_active: AtomicBool::new(false),
                embedded_ble_type_pairasync_startup_guard_until: Mutex::new(None),
                embedded_ble_wake_recovery: Mutex::new(EmbeddedBleWakeRecoverySnapshot::default()),
                device_key_pending_ble_action: Mutex::new(None),
                embedded_ble_session_actor: Mutex::new(EmbeddedBleSessionActorState::default()),
                embedded_ble_cancel_flag: Mutex::new(None),
                polish_prefetch: Mutex::new(None),
                idle_hotkey_cancel_sent: AtomicBool::new(false),
                recording_mute: Mutex::new(SharedRecordingMuteState::new()),
                hotkey: Mutex::new(None),
                hotkey_status: Mutex::new(HotkeyStatus::default()),
                hotkey_trigger_held: AtomicBool::new(false),
                last_hotkey_dispatch_at: Mutex::new(None),
                shortcut_recording_active: AtomicBool::new(false),
                combo_hotkey: Mutex::new(None),
                translation_hotkey: Mutex::new(None),
                switch_style_hotkey: Mutex::new(None),
                open_app_hotkey: Mutex::new(None),
                device_key_hotkeys: std::array::from_fn(|_| Mutex::new(None)),
                device_key_last_dispatch_at: Mutex::new(HashMap::new()),
                translation_modifier_seen: AtomicBool::new(false),
                qa_hotkey: Mutex::new(None),
                qa_state: Mutex::new(QaSessionState::default()),
                capsule_layout: Mutex::new(None),
                capsule_ui_throttle: Mutex::new(CapsuleUiThrottleState::default()),
                capsule_sequence: AtomicU64::new(0),
                capsule_latest_payload: Mutex::new(None),
                qa_asr: Mutex::new(None),
                qa_recorder: Mutex::new(None),
                qa_stream_cancelled: Arc::new(AtomicBool::new(false)),
                local_asr_cache: Arc::new(crate::asr::local::LocalAsrCache::new()),
                foundry_local_runtime,
                shutdown: AtomicBool::new(false),
            }),
        }
    }

    pub fn reposition_capsule_after_display_change<R: tauri::Runtime>(
        &self,
        app: &AppHandle<R>,
        force: bool,
    ) {
        let Some(window) = app.get_webview_window("capsule") else {
            return;
        };
        let translation_active = self
            .inner
            .capsule_layout
            .lock()
            .as_ref()
            .map(|layout| layout.translation_active)
            .unwrap_or(false);
        let current_monitor = window.current_monitor().ok().flatten();
        let target_monitor = crate::capsule_target_monitor(app, &window);
        let off_screen = crate::capsule_window_off_all_monitors(app, &window);
        let target_changed = match target_monitor.as_ref() {
            Some(monitor) => {
                let position = monitor.position();
                let size = monitor.size();
                self.inner
                    .capsule_layout
                    .lock()
                    .as_ref()
                    .map_or(true, |last| {
                        last.monitor_x != position.x
                            || last.monitor_y != position.y
                            || last.monitor_width != size.width
                            || last.monitor_height != size.height
                            || last.scale_bits != monitor.scale_factor().to_bits()
                    })
            }
            None => off_screen,
        };
        let window_on_target_monitor = match (current_monitor.as_ref(), target_monitor.as_ref()) {
            (Some(current), Some(target)) => {
                let current_position = current.position();
                let current_size = current.size();
                let target_position = target.position();
                let target_size = target.size();
                current_position.x == target_position.x
                    && current_position.y == target_position.y
                    && current_size.width == target_size.width
                    && current_size.height == target_size.height
                    && current.scale_factor().to_bits() == target.scale_factor().to_bits()
            }
            (None, None) => true,
            _ => false,
        };
        if !force && !off_screen && !target_changed && window_on_target_monitor {
            return;
        }

        let result = match target_monitor.as_ref() {
            Some(monitor) => crate::position_capsule_bottom_center_on_monitor(
                &window,
                monitor,
                translation_active,
            ),
            None => crate::position_capsule_bottom_center(app, &window, translation_active),
        };
        if let Err(error) = result {
            log::warn!("[coord] capsule display-change reposition failed: {error}");
            return;
        }

        let Some(monitor) = target_monitor.or_else(|| crate::capsule_target_monitor(app, &window))
        else {
            return;
        };
        let position = monitor.position();
        let size = monitor.size();
        let mut layout = self.inner.capsule_layout.lock();
        *layout = Some(CapsuleLayoutState {
            translation_active,
            monitor_x: position.x,
            monitor_y: position.y,
            monitor_width: size.width,
            monitor_height: size.height,
            scale_bits: monitor.scale_factor().to_bits(),
        });
    }

    /// 后台预加载本地 ASR 引擎；当用户在 UI 切到 local-qwen3 provider 时调一次。
    /// 加载是阻塞且数秒，所以放 spawn_blocking 里，不影响 UI 响应。
    /// 模型未下载或不在 macOS 上时静默跳过。
    pub fn preload_local_asr_in_background(self: &Arc<Self>) {
        #[cfg(target_os = "macos")]
        {
            let inner = Arc::clone(&self.inner);
            tauri::async_runtime::spawn(async move {
                let prefs = inner.prefs.get();
                let model_id =
                    match crate::asr::local::ModelId::from_str(&prefs.local_asr_active_model) {
                        Some(m) => m,
                        None => return,
                    };
                if !crate::asr::local::models::is_downloaded(model_id) {
                    log::info!(
                        "[coord] local ASR preload skipped: model {} not downloaded",
                        model_id.as_str()
                    );
                    return;
                }
                let dir = match crate::asr::local::models::model_dir(model_id) {
                    Ok(d) => d,
                    Err(_) => return,
                };
                let cache = Arc::clone(&inner.local_asr_cache);
                let mid = model_id.as_str().to_string();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    if let Err(e) = cache.get_or_load(&mid, &dir) {
                        log::warn!("[coord] local ASR preload failed: {e:#}");
                    }
                })
                .await;
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            // no-op
        }
    }

    pub fn preload_foundry_local_asr_in_background(self: &Arc<Self>, reason: &'static str) {
        #[cfg(target_os = "windows")]
        {
            let inner = Arc::clone(&self.inner);
            tauri::async_runtime::spawn(async move {
                let prefs = inner.prefs.get();
                let active_foundry = foundry::is_foundry_local_whisper(&prefs.active_asr_provider);
                // Local confirmation helper is needed for automatic wake whether or not
                // a voiceprint is enrolled (phrase-only open gate after delete voiceprint).
                let helper_result =
                    tauri::async_runtime::spawn_blocking(crate::asr::local::wake_helper::preload)
                        .await;
                match helper_result {
                    Ok(Ok(())) => log::info!(
                        "[wake-phrase] isolated local confirmation helper ready reason={reason}"
                    ),
                    Ok(Err(error)) => log::warn!(
                        "[wake-phrase] isolated local confirmation helper unavailable reason={reason}: {error}"
                    ),
                    Err(error) => log::warn!(
                        "[wake-phrase] isolated local confirmation helper task failed reason={reason}: {error}"
                    ),
                }
                if !active_foundry {
                    return;
                }
                let model_alias = if foundry::model_alias_is_known(&prefs.foundry_local_asr_model) {
                    prefs.foundry_local_asr_model.clone()
                } else {
                    foundry::DEFAULT_MODEL_ALIAS.to_string()
                };
                let runtime_source = prefs.foundry_local_runtime_source.clone();
                let runtime = Arc::clone(&inner.foundry_local_runtime);
                log::info!(
                    "[foundry-asr] background preload started reason={reason} model={model_alias} source={runtime_source} cached_only=false"
                );
                let result = runtime.ensure_loaded(&model_alias, &runtime_source).await;
                match result {
                    Ok(model_id) => log::info!(
                        "[foundry-asr] background preload ready reason={reason} model={model_alias} model_id={model_id}"
                    ),
                    Err(error) => log::warn!(
                        "[foundry-asr] background preload failed reason={reason} model={model_alias}: {error:#}"
                    ),
                }
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = reason;
        }
    }

    /// 释放当前缓存的本地 ASR 引擎（用户主动点 / 或 删除模型时调）。
    pub fn release_local_asr_engine(&self) {
        self.inner.local_asr_cache.release_now();
    }

    pub fn local_asr_loaded_model(&self) -> Option<String> {
        self.inner.local_asr_cache.loaded_model_id()
    }

    pub fn bind_app(&self, handle: AppHandle) {
        *self.inner.app.lock() = Some(handle);
    }

    pub fn auto_select_embedded_ble_input_source_in_background(&self) {
        if embedded_ble_background_listener_disabled_by_env() {
            log::info!(
                "[embedded-ble] auto input source selection skipped because background BLE is disabled"
            );
            return;
        }
        let prefs = self.inner.prefs.get();
        if prefs.dictation_input_source == DictationInputSource::EmbeddedBle {
            let inner = Arc::clone(&self.inner);
            log::info!(
                "[embedded-ble] startup BLE reconnect fast-path for existing embedded BLE source user_overridden={}",
                prefs.dictation_input_source_user_overridden
            );
            async_runtime::spawn_blocking(move || {
                // Critical path for restart reconnect: do NOT wait on device-settings
                // GATT (up to 2s) before opening notify. Open the name-sync gate and
                // arm the background listener immediately on the persisted target;
                // polish power/name in a short parallel-ish follow-up.
                mark_startup_ble_name_sync_done(&inner, "startup_embedded_ble_power_probe");
                refresh_embedded_ble_listener(&inner);
                polish_startup_ble_settings_after_fast_open(
                    &inner,
                    "startup_embedded_ble_power_probe",
                );
            });
            return;
        }
        if prefs.dictation_input_source_user_overridden {
            log::info!(
                "[embedded-ble] auto input source selection skipped before firmware probe source={:?} user_overridden={}",
                prefs.dictation_input_source,
                prefs.dictation_input_source_user_overridden
            );
            return;
        }

        let inner = Arc::clone(&self.inner);
        async_runtime::spawn_blocking(move || {
            sync_device_ble_name_from_firmware_settings(&inner, "auto_input_source_probe");
            let firmware = crate::embedded_ble::firmware_ota_device_snapshot();
            record_embedded_ble_firmware_power_snapshot(
                &inner,
                &firmware,
                "auto_input_source_probe",
            );
            auto_select_embedded_ble_input_source_from_snapshot(&inner, &firmware);
        });
    }

    /// 让所有 hotkey supervisor loop（dictation / qa / combo / translation /
    /// switch_style / open_app）在下一轮 sleep / poll 后退出。生产场景下进程退出
    /// 一并 reap 所有线程，但 integration test 和未来 RunEvent::Exit 钩子需要
    /// 显式退出路径。审计 3.1.2。
    #[allow(dead_code)]
    pub fn request_shutdown(&self) {
        let first_shutdown = !self.inner.shutdown.swap(true, Ordering::SeqCst);
        if first_shutdown {
            #[cfg(target_os = "windows")]
            {
                match crate::embedded_ble::send_recording_control_type_bye(
                    Duration::from_millis(250),
                ) {
                    Ok(()) => log::info!(
                        "[embedded-ble] Type heartbeat bye sent before coordinator shutdown"
                    ),
                    Err(err) => log::warn!(
                        "[embedded-ble] Type heartbeat bye before coordinator shutdown failed: {err}"
                    ),
                }
            }
        }
        cancel_embedded_ble_listener_capture(&self.inner, "shutdown", false);
        #[cfg(target_os = "windows")]
        if first_shutdown {
            if wait_for_embedded_ble_listener_shutdown_release(Duration::from_secs(3)) {
                log::info!("[embedded-ble] shutdown waited for the WinRT notify owner to release");
            } else {
                log::warn!(
                    "[embedded-ble] shutdown WinRT notify owner did not release within 3000 ms"
                );
            }
        }
    }

    pub fn start_hotkey_listener(&self) {
        // 起一个守护线程，反复尝试安装 hotkey hook。Accessibility 一被授予就立即生效，
        // 用户不需要手动重启 Listener Type。
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("listener-type-hotkey-supervisor".into())
            .spawn(move || hotkey_supervisor_loop(inner))
            .ok();
    }

    pub fn stop_hotkey_listener(&self) {
        self.inner.hotkey.lock().take();
    }

    /// 启动 QA hotkey supervisor（issue #118）。和 `start_hotkey_listener` 平行：
    /// 守护线程反复尝试注册（用户可能改了组合键），失败则 3s 后重试。
    pub fn start_qa_hotkey_listener(&self) {
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("listener-type-qa-hotkey-supervisor".into())
            .spawn(move || qa_hotkey_supervisor_loop(inner))
            .ok();
    }

    pub fn stop_qa_hotkey_listener(&self) {
        // QaHotkeyMonitor::drop 在 macOS 底层是 Carbon RemoveEventHotKey，要求主线程。
        // RunEvent::Exit 回调不保证在 AppKit 主线程跑，drop 漏到 tokio worker 上会
        // 触发 macOS dispatch_assert_queue_fail SIGTRAP。包到 run_on_main_thread 让
        // drop 在主线程发生；AppHandle 已 None 时直接 drop（最坏 crash 也是退出时刻）。
        // 详见 issue #169。
        let app = self.inner.app.lock().clone();
        if let Some(app) = app {
            let inner = Arc::clone(&self.inner);
            let _ = app.run_on_main_thread(move || {
                inner.qa_hotkey.lock().take();
            });
        } else {
            self.inner.qa_hotkey.lock().take();
        }
    }

    /// 启动自定义组合键监听器。当 `prefs.hotkey.trigger == Custom` 时，
    /// 代替 modifier-only 的 hotkey monitor。
    pub fn start_combo_hotkey_listener(&self) {
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("listener-type-combo-hotkey-supervisor".into())
            .spawn(move || combo_hotkey_supervisor_loop(inner))
            .ok();
    }

    pub fn stop_combo_hotkey_listener(&self) {
        take_combo_hotkey_on_main_thread(&self.inner);
    }

    pub fn start_translation_hotkey_listener(&self) {
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("listener-type-translation-hotkey-supervisor".into())
            .spawn(move || translation_hotkey_supervisor_loop(inner))
            .ok();
    }

    pub fn stop_translation_hotkey_listener(&self) {
        take_translation_hotkey_on_main_thread(&self.inner);
    }

    pub fn start_switch_style_hotkey_listener(&self) {
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("listener-type-switch-style-hotkey-supervisor".into())
            .spawn(move || action_hotkey_supervisor_loop(inner, ActionHotkeyKind::SwitchStyle))
            .ok();
    }

    pub fn stop_switch_style_hotkey_listener(&self) {
        take_action_hotkey_on_main_thread(&self.inner, ActionHotkeyKind::SwitchStyle);
    }

    pub fn start_open_app_hotkey_listener(&self) {
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("listener-type-open-app-hotkey-supervisor".into())
            .spawn(move || action_hotkey_supervisor_loop(inner, ActionHotkeyKind::OpenApp))
            .ok();
    }

    pub fn stop_open_app_hotkey_listener(&self) {
        take_action_hotkey_on_main_thread(&self.inner, ActionHotkeyKind::OpenApp);
    }

    pub fn start_device_custom_key_hotkey_listeners(&self) {
        // Device fallback keys already enter through HotkeyMonitor's Windows
        // low-level hook as HotkeyEvent::DeviceCustomKeyPressed. Registering
        // the same F13-F24 combinations with global-hotkey creates a second
        // delivery path whose delayed event can start a new recording after a
        // previous EC11 session has completed.
        log::info!("[coord] device custom keys use the direct low-level hook only");
    }

    pub fn stop_device_custom_key_hotkey_listeners(&self) {
        // See start_device_custom_key_hotkey_listeners: no global-hotkey
        // monitors are installed for device-reserved fallback combinations.
    }

    /// 用户在设置里改了自定义组合键时调用。
    pub fn update_combo_hotkey_binding(&self) {
        let prefs = self.inner.prefs.get();
        if crate::shortcut_binding::legacy_modifier_trigger(&prefs.dictation_hotkey).is_some() {
            // 修饰键单键由 HotkeyMonitor 处理，组合键 monitor 要释放。
            take_combo_hotkey_on_main_thread(&self.inner);
            log::info!("[coord] combo hotkey 已关闭（modifier-only）");
            return;
        }
        let binding = prefs.dictation_hotkey.clone();
        if is_unconfigured_shortcut(&binding) {
            // Custom 但没录到有效主键：清掉旧 monitor，避免旧快捷键继续生效。
            take_combo_hotkey_on_main_thread(&self.inner);
            log::info!("[coord] combo hotkey 已关闭（无绑定）");
            return;
        };
        let app = self.inner.app.lock().clone();
        let Some(app) = app else {
            log::warn!("[coord] update combo hotkey binding: AppHandle 未 bind，跳过");
            return;
        };
        let inner_clone = Arc::clone(&self.inner);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(monitor) = inner_clone.combo_hotkey.lock().as_ref() {
                if let Err(e) = monitor.update_binding(binding_for_main.clone()) {
                    log::warn!("[coord] update combo hotkey binding 失败: {e}");
                }
                return;
            }
            let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
            match ComboHotkeyMonitor::start(binding_for_main, tx) {
                Ok(monitor) => {
                    *inner_clone.combo_hotkey.lock() = Some(monitor);
                    log::info!(
                        "[coord] combo hotkey listener installed on main thread (via update)"
                    );
                    let bridge_inner = Arc::clone(&inner_clone);
                    std::thread::Builder::new()
                        .name("listener-type-combo-hotkey-bridge".into())
                        .spawn(move || combo_hotkey_bridge_loop(bridge_inner, rx))
                        .ok();
                }
                Err(e) => {
                    log::warn!("[coord] update combo hotkey binding 失败: {e}");
                }
            }
        });
    }

    /// 用户在设置里改了 QA 组合键时调用。先持久化（由 prefs.set 完成），
    /// 然后通知活着的 monitor 重新注册；monitor 不存在时 supervisor 会自然
    /// 在下一次循环里读到新的 prefs。
    pub fn update_qa_hotkey_binding(&self) {
        let prefs = self.inner.prefs.get();
        let Some(binding) = prefs.qa_hotkey.clone() else {
            // 用户把功能关了 → 直接 drop monitor。drop 也得在主线程，否则 Carbon
            // unregister 会失败/UB。
            let app = self.inner.app.lock().clone();
            if let Some(app) = app {
                let inner_clone = Arc::clone(&self.inner);
                let _ = app.run_on_main_thread(move || {
                    inner_clone.qa_hotkey.lock().take();
                });
            } else {
                self.inner.qa_hotkey.lock().take();
            }
            log::info!("[coord] QA hotkey 已关闭");
            self.update_modifier_shortcut_bindings();
            return;
        };
        if crate::shortcut_binding::legacy_modifier_trigger(&binding).is_some() {
            let app = self.inner.app.lock().clone();
            if let Some(app) = app {
                let inner_clone = Arc::clone(&self.inner);
                let _ = app.run_on_main_thread(move || {
                    inner_clone.qa_hotkey.lock().take();
                });
            } else {
                self.inner.qa_hotkey.lock().take();
            }
            self.update_modifier_shortcut_bindings();
            log::info!("[coord] QA hotkey uses modifier-only listener");
            return;
        }
        self.update_modifier_shortcut_bindings();
        // global-hotkey crate 的 manager.register/unregister 必须主线程跑。
        // 没在主线程会让 Carbon 句柄注册看似成功但事件不派发。
        let app = self.inner.app.lock().clone();
        let Some(app) = app else {
            log::warn!("[coord] update QA hotkey binding: AppHandle 未 bind，跳过");
            return;
        };
        let inner_clone = Arc::clone(&self.inner);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            // 路径 1：当前已有 monitor → 在主线程换绑定。
            if let Some(monitor) = inner_clone.qa_hotkey.lock().as_ref() {
                if let Err(e) = monitor.update_binding(binding_for_main.clone()) {
                    log::warn!("[coord] update QA hotkey binding 失败: {e}");
                }
                return;
            }
            // 路径 2：之前还没装上 → 主线程上重装一次（supervisor 也会重试，
            // 但用户体感更快：set_qa_hotkey 命令一返回，hotkey 立即生效）。
            let (tx, rx) = mpsc::channel::<QaHotkeyEvent>();
            match QaHotkeyMonitor::start(binding_for_main, tx) {
                Ok(monitor) => {
                    *inner_clone.qa_hotkey.lock() = Some(monitor);
                    log::info!("[coord] QA hotkey listener installed on main thread (via update)");
                    let bridge_inner = Arc::clone(&inner_clone);
                    std::thread::Builder::new()
                        .name("listener-type-qa-hotkey-bridge".into())
                        .spawn(move || qa_hotkey_bridge_loop(bridge_inner, rx))
                        .ok();
                }
                Err(e) => {
                    log::warn!("[coord] update QA hotkey binding 失败: {e}");
                }
            }
        });
    }

    pub fn update_translation_hotkey_binding(&self) {
        if let Err(e) = self.try_update_translation_hotkey_binding() {
            log::warn!("[coord] update translation hotkey binding 失败: {e}");
        }
    }

    pub fn try_update_translation_hotkey_binding(&self) -> Result<(), String> {
        let prefs = self.inner.prefs.get();
        if is_builtin_translation_shift(&prefs.translation_hotkey)
            || crate::shortcut_binding::legacy_modifier_trigger(&prefs.translation_hotkey).is_some()
        {
            take_translation_hotkey_on_main_thread(&self.inner);
            self.update_modifier_shortcut_bindings();
            log::info!("[coord] translation hotkey uses modifier-only listener");
            return Ok(());
        }
        self.update_modifier_shortcut_bindings();
        let app = self.inner.app.lock().clone();
        let Some(app) = app else {
            return Err("AppHandle 未 bind，无法注册翻译快捷键".into());
        };
        let inner_clone = Arc::clone(&self.inner);
        let binding_for_main = prefs.translation_hotkey.clone();
        let (result_tx, result_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let _ = app.run_on_main_thread(move || {
            let result = update_translation_hotkey_on_main_thread(inner_clone, binding_for_main);
            let _ = result_tx.send(result.map_err(|e| e.to_string()));
        });
        match result_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => Err("注册翻译快捷键超时".into()),
        }
    }

    pub fn update_switch_style_hotkey_binding(&self) {
        self.update_action_hotkey_binding(ActionHotkeyKind::SwitchStyle);
    }

    pub fn update_open_app_hotkey_binding(&self) {
        self.update_action_hotkey_binding(ActionHotkeyKind::OpenApp);
    }

    pub fn update_device_custom_key_hotkey_bindings(&self) {
        // Device mappings are read when the direct hook event is handled, so
        // preferences need no parallel global-hotkey registrations.
    }

    fn update_action_hotkey_binding(&self, kind: ActionHotkeyKind) {
        let binding = action_hotkey_binding(&self.inner, kind);
        if is_modifier_only_shortcut(&binding) {
            take_action_hotkey_on_main_thread(&self.inner, kind);
            log::warn!("[coord] action hotkey {kind:?} 使用了不支持的 modifier-only 绑定，已关闭");
            return;
        }

        let app = self.inner.app.lock().clone();
        let Some(app) = app else {
            log::warn!("[coord] update action hotkey binding: AppHandle 未 bind，跳过");
            return;
        };
        let inner_clone = Arc::clone(&self.inner);
        let _ = app.run_on_main_thread(move || {
            if let Some(monitor) = action_hotkey_slot(&inner_clone, kind).lock().as_ref() {
                if let Err(e) = monitor.update_binding(binding.clone()) {
                    log::warn!("[coord] update action hotkey {kind:?} binding 失败: {e}");
                }
                return;
            }
            let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
            match ComboHotkeyMonitor::start(binding, tx) {
                Ok(monitor) => {
                    *action_hotkey_slot(&inner_clone, kind).lock() = Some(monitor);
                    let bridge_inner = Arc::clone(&inner_clone);
                    std::thread::Builder::new()
                        .name(action_hotkey_bridge_thread_name(kind).into())
                        .spawn(move || action_hotkey_bridge_loop(bridge_inner, rx, kind))
                        .ok();
                }
                Err(e) => log::warn!("[coord] update action hotkey {kind:?} binding 失败: {e}"),
            }
        });
    }

    /// 给前端 Settings 渲染当前 QA 快捷键 label（如 "Cmd+Shift+;"）。
    /// `qa_hotkey == None` 时返回空串，UI 据此显示「未启用」。
    pub fn qa_hotkey_label(&self) -> String {
        self.inner
            .prefs
            .get()
            .qa_hotkey
            .as_ref()
            .map(|b| b.display_label())
            .unwrap_or_default()
    }

    /// 用户点 ✕ / 按 Esc 关 QA 浮窗时调。等价于：取消任何进行中的录音 +
    /// 清空多轮对话历史 + 隐藏窗口。详见 issue #118 v2。
    pub fn qa_window_dismiss(&self) {
        close_qa_panel(&self.inner);
    }

    /// 用户点 📌 切换 pinned 状态。pinned=true 时浮窗不自动隐藏。
    pub fn qa_window_pin(&self, pinned: bool) {
        self.inner.qa_state.lock().pinned = pinned;
        log::info!("[coord] QA window pinned={pinned}");
    }

    pub fn history(&self) -> &HistoryStore {
        &self.inner.history
    }
    pub fn prefs(&self) -> &PreferencesStore {
        &self.inner.prefs
    }
    pub fn style_packs(&self) -> &StylePackStore {
        &self.inner.style_packs
    }
    pub fn vocab(&self) -> &DictionaryStore {
        &self.inner.vocab
    }
    pub fn correction_rules(&self) -> &CorrectionRuleStore {
        &self.inner.correction_rules
    }

    pub fn update_hotkey_binding(&self) {
        let prefs = self.inner.prefs.get();
        let dictation_trigger =
            crate::shortcut_binding::legacy_modifier_trigger(&prefs.dictation_hotkey);
        let binding = crate::types::HotkeyBinding {
            trigger: dictation_trigger.unwrap_or(crate::types::HotkeyTrigger::Custom),
            mode: prefs.hotkey.mode,
            keys: None,
        };
        if dictation_trigger.is_some() {
            take_combo_hotkey_on_main_thread(&self.inner);
        } else {
            self.update_combo_hotkey_binding();
        }
        self.ensure_modifier_hotkey_monitor(binding);
        self.update_modifier_shortcut_bindings();
    }

    fn ensure_modifier_hotkey_monitor(&self, binding: crate::types::HotkeyBinding) {
        if let Some(monitor) = self.inner.hotkey.lock().as_ref() {
            monitor.update_binding(binding);
            return;
        }
        let (tx, rx) = mpsc::channel::<HotkeyEvent>();
        match HotkeyMonitor::start(binding, tx) {
            Ok(monitor) => {
                let adapter = monitor.kind();
                *self.inner.hotkey.lock() = Some(monitor);
                *self.inner.hotkey_status.lock() = HotkeyStatus {
                    adapter,
                    state: HotkeyStatusState::Installed,
                    message: Some(format!("{} 已安装", adapter.display_name())),
                    last_error: None,
                };
                let inner_clone = Arc::clone(&self.inner);
                std::thread::Builder::new()
                    .name("listener-type-hotkey-bridge".into())
                    .spawn(move || hotkey_bridge_loop(inner_clone, rx))
                    .ok();
            }
            Err(e) => {
                *self.inner.hotkey_status.lock() = HotkeyStatus {
                    adapter: HotkeyMonitor::capability().adapter,
                    state: HotkeyStatusState::Failed,
                    message: Some(e.message.clone()),
                    last_error: Some(e),
                };
            }
        }
    }

    pub fn update_modifier_shortcut_bindings(&self) {
        if let Some(monitor) = self.inner.hotkey.lock().as_ref() {
            let (qa_trigger, translation_trigger) = modifier_shortcut_triggers(&self.inner);
            monitor.update_modifier_shortcuts(qa_trigger, translation_trigger);
        }
    }

    pub fn hotkey_status(&self) -> HotkeyStatus {
        self.inner.hotkey_status.lock().clone()
    }

    pub fn embedded_ble_listener_last_error(&self) -> Option<String> {
        embedded_ble_listener_last_error(&self.inner)
    }

    pub fn embedded_ble_listener_active(&self) -> bool {
        embedded_ble_listener_capture_active(&self.inner)
    }

    pub fn embedded_ble_listener_ready(&self) -> bool {
        embedded_ble_listener_capture_ready(&self.inner)
    }

    pub fn embedded_ble_listener_generation(&self) -> u64 {
        self.inner
            .embedded_ble_listener_generation
            .load(Ordering::SeqCst)
    }

    pub fn embedded_ble_wake_recovery_snapshot(&self) -> EmbeddedBleWakeRecoverySnapshot {
        embedded_ble_wake_recovery_snapshot(&self.inner)
    }

    pub fn record_embedded_ble_firmware_power_snapshot(
        &self,
        snapshot: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
        reason: &'static str,
    ) {
        record_embedded_ble_firmware_power_snapshot(&self.inner, snapshot, reason);
    }

    pub fn embedded_ble_session_actor_diagnostics(
        &self,
    ) -> Vec<EmbeddedBleSessionActorDiagnosticRecord> {
        embedded_ble_session_actor_diagnostics(&self.inner)
    }

    pub fn hotkey_capability(&self) -> HotkeyCapability {
        HotkeyMonitor::capability()
    }

    pub fn acknowledge_automatic_wake_capsule_visible(&self, session_id: &str) {
        let Ok(session_id) = Uuid::parse_str(session_id) else {
            return;
        };
        acknowledge_automatic_wake_capsule_visible(&self.inner, session_id);
    }

    pub async fn start_dictation(&self) -> Result<(), String> {
        if self.inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle {
            if embedded_ble_listener_capture_ready(&self.inner) {
                record_embedded_ble_notify_ready(&self.inner);
                request_embedded_ble_recording_start_from_host(
                    &self.inner,
                    "host_start_ready_listener",
                )
                .await?;
                log::info!("[coord] start_dictation started Listener BLE recording via ready background listener");
                return Ok(());
            }
            self.refresh_embedded_ble_listener();
            record_embedded_ble_reconnect_attempt(&self.inner, "start_dictation");
            emit_capsule(
                &self.inner,
                CapsuleState::Recording,
                0.0,
                0,
                Some("正在恢复 Listener BLE".to_string()),
                None,
            );
            match wait_for_embedded_ble_listener_ready(
                &self.inner,
                EMBEDDED_BLE_WAKE_RECOVERY_TIMEOUT,
            )
            .await
            {
                Ok(()) => {
                    record_embedded_ble_notify_ready(&self.inner);
                    request_embedded_ble_recording_start_from_host(
                        &self.inner,
                        "host_start_after_recovery",
                    )
                    .await?;
                }
                Err(err) => {
                    record_embedded_ble_listener_last_error(&self.inner, &err);
                    record_embedded_ble_recovery_failure(&self.inner, &err);
                    let message = embedded_ble_wake_guidance_for_error(&err);
                    emit_capsule(
                        &self.inner,
                        CapsuleState::Error,
                        0.0,
                        0,
                        Some("Listener 音频通道未恢复".to_string()),
                        None,
                    );
                    schedule_capsule_idle(&self.inner, 5000, None);
                    return Err(message);
                }
            }
            log::info!("[coord] start_dictation routed to Listener BLE background listener");
            return Ok(());
        }
        begin_session(&self.inner).await
    }

    pub async fn stop_dictation(&self) -> Result<(), String> {
        if request_embedded_ble_recording_stop_from_host(
            &self.inner,
            "capsule_confirm_stop_processing_start",
        )
        .await?
        {
            if listening_session_has_no_current_asr(&self.inner) {
                end_session(&self.inner).await?;
            }
            return Ok(());
        }
        if self.inner.state.lock().phase == SessionPhase::Starting {
            request_stop_during_starting(&self.inner, "manual stop");
            return Ok(());
        }
        end_session(&self.inner).await
    }

    pub async fn submit_embedded_audio_notifications(
        &self,
        notifications: Vec<Vec<u8>>,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submit_embedded_audio_notifications(&self.inner, notifications).await
    }

    pub async fn submit_embedded_audio_streaming_notifications(
        &self,
        notifications: Vec<Vec<u8>>,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submit_embedded_audio_streaming_notifications(&self.inner, notifications).await
    }

    pub async fn submit_embedded_audio_file(
        &self,
        path: std::path::PathBuf,
        format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submit_embedded_audio_file(&self.inner, path, format).await
    }

    pub async fn submit_embedded_audio_streaming_file(
        &self,
        path: std::path::PathBuf,
        format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submit_embedded_audio_streaming_file(&self.inner, path, format).await
    }

    pub async fn submit_embedded_audio_ble_once(
        &self,
        timeout_ms: Option<u64>,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        pause_embedded_ble_listener_capture(&self.inner, "foreground once-shot capture");
        let result = submit_embedded_audio_ble_once(&self.inner, timeout_ms).await;
        self.refresh_embedded_ble_listener();
        result
    }

    pub async fn submit_embedded_audio_ble_stream(
        &self,
        timeout_ms: Option<u64>,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        pause_embedded_ble_listener_capture(&self.inner, "foreground streaming capture");
        let result = submit_embedded_audio_ble_stream(&self.inner, timeout_ms).await;
        self.refresh_embedded_ble_listener();
        result
    }

    pub async fn probe_embedded_audio_ble_subscription(
        &self,
        timeout_ms: Option<u64>,
    ) -> Result<(), String> {
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(10_000).clamp(1_000, 30_000));
        let background_recovery_timeout = timeout.max(Duration::from_secs(20));
        match embedded_ble_foreground_probe_mode(&self.inner) {
            // Owner bug: Overview/settings BLE probe used to tear down a live TYPE:READY
            // notify and often failed the 20s reopen race (CCCD disable timeout +
            // generation contention). Health check must not kill a working path.
            EmbeddedBleForegroundProbeMode::ReuseReadyBackground => {
                log::info!(
                    "[embedded-ble] foreground BLE path probe reusing ready background listener without refresh"
                );
                clear_embedded_ble_listener_last_error(&self.inner);
                record_embedded_ble_notify_ready(&self.inner);
                return Ok(());
            }
            EmbeddedBleForegroundProbeMode::RefreshBackgroundListener => {
                log::info!("[embedded-ble] foreground BLE path probe refreshing unready background listener");
                self.refresh_embedded_ble_listener();
                record_embedded_ble_reconnect_attempt(&self.inner, "foreground_probe_refresh");
                let result =
                    wait_for_embedded_ble_listener_ready(&self.inner, background_recovery_timeout)
                        .await;
                if result.is_ok() {
                    clear_embedded_ble_listener_last_error(&self.inner);
                    record_embedded_ble_notify_ready(&self.inner);
                } else if let Err(err) = &result {
                    record_embedded_ble_listener_last_error(&self.inner, err);
                    record_embedded_ble_recovery_failure(&self.inner, err);
                }
                return result;
            }
            EmbeddedBleForegroundProbeMode::StartBackgroundListener => {
                log::info!("[embedded-ble] foreground BLE path probe delegated to background listener recovery");
                self.refresh_embedded_ble_listener();
                record_embedded_ble_reconnect_attempt(&self.inner, "foreground_probe");
                let result =
                    wait_for_embedded_ble_listener_ready(&self.inner, background_recovery_timeout)
                        .await;
                if result.is_ok() {
                    clear_embedded_ble_listener_last_error(&self.inner);
                    record_embedded_ble_notify_ready(&self.inner);
                } else if let Err(err) = &result {
                    record_embedded_ble_listener_last_error(&self.inner, err);
                    record_embedded_ble_recovery_failure(&self.inner, err);
                }
                return result;
            }
            EmbeddedBleForegroundProbeMode::ForegroundProbe => {}
        }

        pause_embedded_ble_listener_capture(&self.inner, "foreground BLE path probe");
        // Note: tokio::time::timeout only abandons the JoinHandle on expiry;
        // the underlying blocking thread continues until probe_notify_subscription
        // returns (bounded by its own inner timeout). This is acceptable because
        // the inner timeout is always strictly less than probe_budget.
        let probe_budget = timeout + Duration::from_secs(4);
        let probe_task = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::probe_notify_subscription(timeout)
        });
        let result = match tokio::time::timeout(probe_budget, probe_task).await {
            Ok(joined) => joined
                .map_err(|err| format!("嵌入式 BLE 通路探测任务失败: {err}"))
                .and_then(|result| result),
            Err(_) => Err(format!(
                "BLE subscription check timed out after {} ms",
                probe_budget.as_millis()
            )),
        };
        if result.is_ok() {
            clear_embedded_ble_listener_last_error(&self.inner);
            record_embedded_ble_notify_ready(&self.inner);
        } else if let Err(err) = &result {
            record_embedded_ble_listener_last_error(&self.inner, err);
            record_embedded_ble_recovery_failure(&self.inner, err);
        }
        self.refresh_embedded_ble_listener();
        if result.is_ok() {
            wait_for_embedded_ble_listener_ready(
                &self.inner,
                timeout.min(EMBEDDED_BLE_PROBE_RECOVERY_TIMEOUT),
            )
            .await?;
        }
        result
    }

    pub async fn repair_embedded_ble_connection(
        &self,
        timeout_ms: Option<u64>,
    ) -> Result<EmbeddedBleWakeRecoverySnapshot, String> {
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(12_000).clamp(1_000, 45_000));
        // Owner bug: after a false "manual Windows pairing removal" hold, refresh is
        // skipped forever while passively awaiting re-pair. Customer repair / 一键修复
        // and EC11 double-click recovery must force Type to leave that dead-end and
        // re-open notify / PairAsync.
        clear_embedded_ble_passive_local_reattach(&self.inner, "customer repair action");
        clear_embedded_ble_pairing_confirmation_hold(&self.inner, "customer repair action");
        pause_embedded_ble_listener_capture(&self.inner, "customer repair action");
        clear_embedded_ble_listener_last_error(&self.inner);
        record_embedded_ble_reconnect_attempt(&self.inner, "customer_repair");
        self.refresh_embedded_ble_listener();

        match wait_for_embedded_ble_listener_ready(&self.inner, timeout).await {
            Ok(()) => {
                clear_embedded_ble_listener_last_error(&self.inner);
                record_embedded_ble_notify_ready(&self.inner);
                Ok(self.embedded_ble_wake_recovery_snapshot())
            }
            Err(err) => {
                record_embedded_ble_listener_last_error(&self.inner, &err);
                record_embedded_ble_recovery_failure(&self.inner, &err);
                Err(err)
            }
        }
    }

    pub async fn pause_embedded_ble_listener_for_recovery_cleanup(
        &self,
        timeout: Duration,
    ) -> bool {
        pause_embedded_ble_listener_capture(&self.inner, "customer recovery cleanup");
        wait_for_embedded_ble_listener_inactive(&self.inner, timeout).await
    }

    pub async fn pause_embedded_ble_listener_for_ble_name_apply_handoff(
        &self,
        timeout: Duration,
    ) -> bool {
        pause_embedded_ble_listener_capture_for_ble_name_apply_handoff(&self.inner);
        wait_for_embedded_ble_listener_inactive(&self.inner, timeout).await
    }

    pub fn hold_embedded_ble_listener_for_pairing_confirmation(&self, reason: &'static str) {
        hold_embedded_ble_listener_for_pairing_confirmation(&self.inner, reason);
    }

    pub fn hold_embedded_ble_listener_for_native_pairing_handoff(&self, expected_ble_name: String) {
        let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
            &self.inner,
            EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON,
        );
        start_embedded_ble_pairing_confirmation_watch(
            &self.inner,
            expected_ble_name,
            hold_generation,
            EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON,
        );
    }

    pub fn clear_embedded_ble_pairing_confirmation_hold(&self, reason: &'static str) {
        clear_embedded_ble_pairing_confirmation_hold(&self.inner, reason);
    }

    pub fn cancel_dictation(&self) {
        cancel_session(&self.inner);
    }

    pub async fn pause_embedded_ble_listener_for_ota(&self, timeout: Duration) -> bool {
        // Belt-and-suspenders: OTA may have been marked active already, but any
        // in-flight session must die before exclusive GATT transfer.
        dictation::suppress_dictation_pipeline_for_firmware_ota(&self.inner);
        pause_embedded_ble_listener_capture_for_ota(&self.inner);
        wait_for_embedded_ble_listener_inactive(&self.inner, timeout).await
    }

    pub fn try_begin_firmware_ota_transfer(&self) -> bool {
        let started = self
            .inner
            .embedded_ble_ota_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if started {
            // OTA owns the BLE link exclusively: kill any live dictation/wake path and
            // hide recording capsules so voice-activation cannot pop UI mid-upgrade.
            dictation::suppress_dictation_pipeline_for_firmware_ota(&self.inner);
        }
        started
    }

    pub fn begin_firmware_ota_transfer(&self) {
        let _ = self.try_begin_firmware_ota_transfer();
    }

    pub fn firmware_ota_transfer_active(&self) -> bool {
        self.inner.embedded_ble_ota_active.load(Ordering::SeqCst)
    }

    pub fn end_firmware_ota_transfer(&self) {
        self.end_firmware_ota_transfer_with_listener_restore(true);
    }

    /// Clear the exclusive OTA gate. When `restore_listener` is false, the caller owns
    /// post-OTA notify reattach (avoids a doomed first refresh racing reboot settle).
    pub fn end_firmware_ota_transfer_with_listener_restore(&self, restore_listener: bool) {
        self.inner
            .embedded_ble_ota_active
            .store(false, Ordering::SeqCst);
        // Preflight, failed BEGIN, and exclusive handoff all send TYPE:BYE / tear notify.
        // Without re-arming here the device stays in BLE CONNECTED "找 Type" breathing LED
        // instead of TYPE_READY steady blue. Clear the OTA flag first so RecordingGate allows
        // BackgroundListener refresh.
        if restore_listener && embedded_ble_background_listener_expected(&self.inner) {
            log::info!(
                "[firmware-ota] restoring background listener after OTA session (reassert TYPE:READY)"
            );
            refresh_embedded_ble_listener(&self.inner);
        } else if !restore_listener {
            log::info!(
                "[firmware-ota] OTA gate cleared; caller-owned OTA listener recovery owns TYPE:READY"
            );
        }
    }

    pub fn end_failed_firmware_ota_transfer(&self) {
        self.end_firmware_ota_transfer_with_listener_restore(false);
        if embedded_ble_background_listener_expected(&self.inner) {
            log::info!(
                "[firmware-ota] restoring failed OTA through non-destructive bonded listener recovery"
            );
            refresh_embedded_ble_listener_after_failed_firmware_ota(&self.inner);
        }
    }

    pub async fn wait_for_embedded_ble_listener_ready_after_firmware_ota(
        &self,
        timeout: Duration,
    ) -> Result<bool, String> {
        if !embedded_ble_background_listener_expected(&self.inner) {
            log::info!(
                "[firmware-ota] post-OTA Listener notify recovery is not required because EmbeddedBle is not the active input source"
            );
            return Ok(false);
        }
        // Owner: after OTA, a one-shot ready edge can race Windows disconnect/ghost-prune
        // and look "stuck until Type restart". Require a short hold so TYPE:READY sticks.
        // 800 ms hold after OTA caused "edge lost; waiting again" double reopen
        // that owners read as two boot lights. 250 ms still rejects one-shot CCCD
        // flukes without a second reconnect cycle.
        wait_for_embedded_ble_listener_ready_stable(
            &self.inner,
            timeout,
            Duration::from_millis(250),
        )
        .await?;
        Ok(true)
    }

    pub async fn wait_for_embedded_ble_listener_ready_before_firmware_ota(
        &self,
        timeout: Duration,
    ) -> Result<bool, String> {
        if !embedded_ble_background_listener_expected(&self.inner) {
            log::info!(
                "[firmware-ota] pre-transfer Listener notify readiness is not required because EmbeddedBle is not the active input source"
            );
            return Ok(false);
        }
        wait_for_embedded_ble_listener_ready(&self.inner, timeout).await?;
        Ok(true)
    }

    pub fn refresh_embedded_ble_listener(&self) {
        refresh_embedded_ble_listener(&self.inner);
    }

    pub fn refresh_embedded_ble_listener_after_firmware_ota(&self) {
        refresh_embedded_ble_listener_after_firmware_ota(&self.inner);
    }

    pub fn sync_device_knob_rotation_action_to_firmware(&self, reason: &'static str) {
        sync_device_knob_rotation_action_to_firmware(&self.inner, reason);
    }

    /// 返回当前听写阶段（read-only 快照），供 CLI 入口在 dispatch toggle 时决策。
    /// 与原热键边沿走的 `handle_pressed` 分支完全相同的判定逻辑：Idle → start，
    /// Listening → stop。Linux/Wayland 下桌面快捷键 → CLI 转发是唯一触发路径，
    /// 必须复用这套语义。
    pub fn dictation_phase_for_cli(&self) -> SessionPhase {
        self.inner.state.lock().phase
    }

    /// Read-only coordinator snapshot for diagnostics and trigger preflight.
    /// The state lock is released before the DTO crosses the command boundary.
    pub fn dictation_runtime_snapshot(&self, request_id: u32) -> DictationRuntimeSnapshot {
        let state = self.inner.state.lock();
        dictation_runtime_snapshot_from_state(request_id, &state)
    }

    /// Return the latest display-only capsule payload. The capsule WebView may
    /// be hidden or recreated while the coordinator continues running; callers
    /// use this only to restore the visible UI state, never to restart a
    /// recording or replay an insertion.
    pub fn capsule_latest_payload(&self) -> Option<CapsulePayload> {
        self.inner.capsule_latest_payload.lock().clone()
    }

    /// CLI 入口的 QA toggle：直接复用 modifier-only QA 热键边沿的处理函数。
    /// 与 `handle_qa_hotkey_pressed` 同语义 — Idle → 开浮窗 / Recording → 收尾 /
    /// Processing → 忽略。Wayland 下没有 modifier-only / global-hotkey 监听，CLI
    /// 是唯一进入点。
    pub async fn cli_toggle_qa_panel(&self) {
        handle_qa_hotkey_pressed(&self.inner).await;
    }

    pub fn set_shortcut_recording_active(&self, active: bool) {
        self.inner
            .shortcut_recording_active
            .store(active, Ordering::SeqCst);
        if active {
            reset_shortcut_held_state(&self.inner);
        }
        log::info!("[coord] shortcut recording active={active}");
    }

    pub async fn handle_window_hotkey_event(
        &self,
        event_type: String,
        key: String,
        code: String,
        repeat: bool,
    ) -> Result<(), String> {
        handle_window_hotkey_event(&self.inner, event_type, key, code, repeat).await
    }

    #[cfg(any(debug_assertions, test))]
    pub async fn inject_hotkey_click_for_dev(&self) -> Result<(), String> {
        log::info!("[coord] dev hotkey injection started");
        handle_pressed(&self.inner).await;
        dictation::handle_released(&self.inner).await;
        cancel_session(&self.inner);
        Ok(())
    }

    pub async fn repolish(&self, raw_text: String, mode: PolishMode) -> Result<String, String> {
        let hotwords = enabled_phrases(&self.inner);
        let prefs = self.inner.prefs.get();
        let pack = self
            .inner
            .style_packs
            .get_or_default_active(&prefs.active_style_pack_id)
            .map_err(|e| e.to_string())?;
        let style_system_prompt = pack.prompt.clone();
        let working_languages = prefs.working_languages;
        let chinese_script_preference = prefs.chinese_script_preference;
        let output_language_preference = prefs.output_language_preference;
        let llm_thinking_enabled = prefs.llm_thinking_enabled;
        let effective_mode = pack.base_mode;
        log::info!(
            "[style-pack] repolish dispatch active_pack={} kind={:?} effective_mode={:?} legacy_mode={:?} raw_chars={} prompt_chars={} hotwords={} thinking={}",
            pack.id,
            pack.kind,
            effective_mode,
            mode,
            raw_text.chars().count(),
            style_system_prompt.chars().count(),
            hotwords.len(),
            llm_thinking_enabled
        );
        if effective_mode == PolishMode::Raw && !raw_style_pack_uses_llm(&pack) {
            log::info!(
                "[style-pack] repolish bypass llm active_pack={} reason=default_builtin_raw",
                pack.id
            );
            return Ok(raw_text);
        }
        // repolish 是历史记录里手动重新润色，不再绑定原 session 的前台 app；
        // 当下用户调起的 app 才是相关上下文（如果可拿）。
        let front_app = capture_frontmost_app();
        // repolish 是用户主动对单条历史"重新润色"，不应该被对话感知上下文影响——
        // 用户改的就是这一条本身，不要把别的会话拿进来。所以始终走单轮路径。
        polish_text(
            &raw_text,
            effective_mode,
            &hotwords,
            &style_system_prompt,
            &working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app.as_deref(),
            &[],
        )
        .await
        .map_err(|e| e.to_string())
    }

    pub fn preview_style_pack_runtime(
        &self,
        style_pack: &crate::types::StylePack,
    ) -> crate::types::StylePackRuntimeDiagnostics {
        let prefs = self.inner.prefs.get();
        let hotwords = enabled_phrases(&self.inner);
        let single_turn = crate::polish::assemble_polish_system_prompt(
            &style_pack.prompt,
            &hotwords,
            &prefs.working_languages,
            prefs.chinese_script_preference,
            prefs.output_language_preference,
            None,
            false,
        );
        let multi_turn = crate::polish::assemble_polish_system_prompt(
            &style_pack.prompt,
            &hotwords,
            &prefs.working_languages,
            prefs.chinese_script_preference,
            prefs.output_language_preference,
            None,
            true,
        );
        crate::types::StylePackRuntimeDiagnostics {
            pack_id: style_pack.id.clone(),
            pack_name: style_pack.name.clone(),
            pack_prompt: style_pack.prompt.clone(),
            pack_prompt_chars: style_pack.prompt.chars().count(),
            context_premise: single_turn.context_premise.clone(),
            context_premise_chars: single_turn.context_premise.chars().count(),
            hotword_block: single_turn.hotword_block.clone(),
            hotword_block_chars: single_turn.hotword_block.chars().count(),
            history_instruction: multi_turn.history_instruction.clone(),
            history_instruction_chars: multi_turn.history_instruction.chars().count(),
            single_turn_prompt: single_turn.effective_system_prompt.clone(),
            single_turn_prompt_chars: single_turn.effective_system_prompt.chars().count(),
            multi_turn_prompt: multi_turn.effective_system_prompt.clone(),
            multi_turn_prompt_chars: multi_turn.effective_system_prompt.chars().count(),
            working_languages: prefs.working_languages,
            hotwords,
            context_window_minutes: prefs.polish_context_window_minutes,
            includes_context_premise: single_turn.includes_context_premise,
            includes_hotword_block: single_turn.includes_hotword_block,
            includes_history_instruction: multi_turn.includes_history_instruction,
            preview_omits_front_app: true,
        }
    }
}

fn raw_style_pack_uses_llm(pack: &crate::types::StylePack) -> bool {
    !(pack.kind == crate::types::StylePackKind::Builtin
        && pack.id == crate::types::BUILTIN_STYLE_PACK_RAW_ID
        && pack.prompt == crate::types::StyleSystemPrompts::default().raw)
}

fn raw_mode_uses_llm(style_system_prompt: &str) -> bool {
    style_system_prompt != crate::types::StyleSystemPrompts::default().raw
}

// ─────────────────────────── hotkey bridging ───────────────────────────

include!("coordinator/hotkey_device_runtime.rs");

include!("coordinator/embedded_ble_runtime.rs");

fn hotkey_bridge_loop(inner: Arc<Inner>, rx: mpsc::Receiver<HotkeyEvent>) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            continue;
        }
        let inner_cloned = Arc::clone(&inner);
        match evt {
            HotkeyEvent::Pressed => {
                async_runtime::spawn(async move { handle_pressed_edge(&inner_cloned).await });
            }
            HotkeyEvent::Released => {
                async_runtime::spawn(async move { handle_released_edge(&inner_cloned).await });
            }
            HotkeyEvent::Cancelled => {
                let phase = inner_cloned.state.lock().phase;
                if phase == SessionPhase::Idle {
                    // 空闲重复 cancel 去重：第一次已做清理；后续 Idle 的
                    // Esc/cancel 不再 bounce background listener actor。
                    if inner_cloned
                        .idle_hotkey_cancel_sent
                        .swap(true, Ordering::SeqCst)
                    {
                        continue;
                    }
                    log::info!("[coord] global hotkey cancel received phase={phase:?}");
                    cancel_session(&inner_cloned);
                } else {
                    inner_cloned
                        .idle_hotkey_cancel_sent
                        .store(false, Ordering::SeqCst);
                    log::info!("[coord] global hotkey cancel received phase={phase:?}");
                    cancel_session(&inner_cloned);
                }
            }
            HotkeyEvent::TranslationModifierPressed => {
                let translation_hotkey = inner_cloned.prefs.get().translation_hotkey;
                if is_builtin_translation_shift(&translation_hotkey)
                    || crate::shortcut_binding::legacy_modifier_trigger(&translation_hotkey)
                        .is_some()
                {
                    mark_translation_modifier_seen(&inner_cloned);
                }
            }
            HotkeyEvent::QaShortcutPressed => {
                async_runtime::spawn(async move { handle_qa_hotkey_pressed(&inner_cloned).await });
            }
            HotkeyEvent::DeviceCustomKeyPressed { key, gesture } => {
                handle_device_custom_key_pressed(&inner_cloned, key, gesture);
            }
        }
    }
}

fn reset_shortcut_held_state(inner: &Arc<Inner>) {
    inner.hotkey_trigger_held.store(false, Ordering::SeqCst);
    if let Some(monitor) = inner.hotkey.lock().as_ref() {
        monitor.reset_held_state();
    }
    let prefs = inner.prefs.get();
    if let Some(binding) = prefs.qa_hotkey.as_ref() {
        if crate::shortcut_binding::legacy_modifier_trigger(binding).is_none() {
            if let Some(monitor) = inner.qa_hotkey.lock().as_ref() {
                if let Err(e) = monitor.update_binding(binding.clone()) {
                    log::warn!("[coord] reset QA hotkey latch failed: {e}");
                }
            }
        }
    }
    if !is_builtin_translation_shift(&prefs.translation_hotkey)
        && crate::shortcut_binding::legacy_modifier_trigger(&prefs.translation_hotkey).is_none()
    {
        if let Some(monitor) = inner.translation_hotkey.lock().as_ref() {
            if let Err(e) = monitor.update_binding(prefs.translation_hotkey.clone()) {
                log::warn!("[coord] reset translation hotkey latch failed: {e}");
            }
        }
    }
    if !is_modifier_only_shortcut(&prefs.switch_style_hotkey) {
        if let Some(monitor) = inner.switch_style_hotkey.lock().as_ref() {
            if let Err(e) = monitor.update_binding(prefs.switch_style_hotkey.clone()) {
                log::warn!("[coord] reset switch-style hotkey latch failed: {e}");
            }
        }
    }
    if !is_modifier_only_shortcut(&prefs.open_app_hotkey) {
        if let Some(monitor) = inner.open_app_hotkey.lock().as_ref() {
            if let Err(e) = monitor.update_binding(prefs.open_app_hotkey.clone()) {
                log::warn!("[coord] reset open-app hotkey latch failed: {e}");
            }
        }
    }
    for gesture in DeviceCustomKeyGesture::ALL {
        for key in DeviceCustomKeyId::ALL {
            if !key.supports_gesture(gesture) {
                continue;
            }
            let kind = ActionHotkeyKind::DeviceKey { key, gesture };
            if let Some(monitor) = action_hotkey_slot(inner, kind).lock().as_ref() {
                let binding = action_hotkey_binding(inner, kind);
                if let Err(e) = monitor.update_binding(binding) {
                    log::warn!(
                        "[coord] reset {} {} hotkey latch failed: {e}",
                        key.label(),
                        gesture.label()
                    );
                }
            }
        }
    }
}

async fn handle_window_hotkey_event(
    inner: &Arc<Inner>,
    event_type: String,
    key: String,
    code: String,
    repeat: bool,
) -> Result<(), String> {
    if inner.shortcut_recording_active.load(Ordering::SeqCst) {
        return Ok(());
    }
    if event_type == "keydown" && key == "Escape" {
        // Esc 路由（issue #161）：QA 浮窗可见时优先取消 QA（不动 dictation）；
        // 否则走 dictation 取消通路。之前无条件 cancel_session 导致 QA 浮窗
        // 按 Esc 杀的是 dictation 而 QA 流还在烧 token。
        let qa_active = {
            let st = inner.qa_state.lock();
            st.panel_visible || st.phase != QaPhase::Idle
        };
        if qa_active {
            close_qa_panel(inner);
        } else {
            cancel_session(inner);
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (inner, event_type, key, code, repeat);
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        if !window_hotkey_fallback_enabled() {
            if event_type == "keydown" && !repeat {
                log::info!(
                    "[window-hotkey] ignored because Windows lifecycle owner is the low-level hook"
                );
            }
            return Ok(());
        }

        let Some(trigger) =
            crate::shortcut_binding::legacy_modifier_trigger(&inner.prefs.get().dictation_hotkey)
        else {
            return Ok(());
        };
        if !window_key_matches_trigger(trigger, &key, &code) {
            return Ok(());
        }

        match event_type.as_str() {
            "keydown" => {
                if repeat {
                    return Ok(());
                }
                log::info!(
                    "[window-hotkey] pressed trigger={trigger:?} code={code} repeat={repeat}"
                );
                handle_pressed_edge(inner).await;
            }
            "keyup" => {
                log::info!("[window-hotkey] released trigger={trigger:?} code={code}");
                handle_released_edge(inner).await;
            }
            _ => {}
        }
        Ok(())
    }
}

fn window_hotkey_fallback_enabled() -> bool {
    crate::types::HotkeyCapability::current().explicit_fallback_available
}

#[cfg(any(target_os = "windows", test))]
fn window_key_matches_trigger(trigger: crate::types::HotkeyTrigger, key: &str, code: &str) -> bool {
    use crate::types::HotkeyTrigger;

    match trigger {
        HotkeyTrigger::RightControl => key == "Control" && code == "ControlRight",
        HotkeyTrigger::LeftControl => key == "Control" && code == "ControlLeft",
        HotkeyTrigger::RightOption | HotkeyTrigger::RightAlt => {
            (key == "Alt" || key == "AltGraph") && code == "AltRight"
        }
        HotkeyTrigger::LeftOption => (key == "Alt" || key == "AltGraph") && code == "AltLeft",
        HotkeyTrigger::RightCommand => key == "Meta" && code == "MetaRight",
        HotkeyTrigger::Fn => key == "Control" && code == "ControlRight",
        // Custom 走 global-hotkey crate，不走 window hotkey fallback
        HotkeyTrigger::Custom => false,
    }
}

// ─────────────────────────── session lifecycle ───────────────────────────

/// QA 录音 runtime error 监听器。镜像 `spawn_recorder_error_monitor` 的语义但走 QA
/// 收尾路径（`finish_qa_with_error` 替代 `abort_recording_with_error`）。
/// 用 qa_state.session_id 守卫 stale 事件。详见 issue #168。
fn spawn_qa_recorder_error_monitor(inner: &Arc<Inner>, rx: mpsc::Receiver<RecorderError>) {
    let captured_session_id = inner.qa_state.lock().session_id;
    let inner = Arc::clone(inner);
    std::thread::Builder::new()
        .name("listener-type-qa-recorder-error-monitor".into())
        .spawn(move || {
            if let Ok(err) = rx.recv() {
                let current_session_id = inner.qa_state.lock().session_id;
                if captured_session_id != current_session_id {
                    log::warn!(
                        "[coord] QA recorder error from stale session {} dropped (current={}, err={})",
                        captured_session_id,
                        current_session_id,
                        err
                    );
                    return;
                }
                log::error!("[coord] QA recorder runtime error: {err}");
                finish_qa_with_error(&inner, format!("录音设备异常: {err}"));
            }
        })
        .ok();
}

#[cfg(target_os = "windows")]
fn store_prepared_windows_ime_session(
    slots: &mut Vec<PreparedWindowsImeSessionSlot>,
    session_id: SessionId,
    prepared: PreparedWindowsImeSession,
) {
    slots.retain(|slot| slot.session_id != session_id);
    slots.push(PreparedWindowsImeSessionSlot {
        session_id,
        prepared,
    });
}

#[cfg(target_os = "windows")]
fn take_matching_prepared_windows_ime_session(
    slots: &mut Vec<PreparedWindowsImeSessionSlot>,
    session_id: SessionId,
) -> Option<PreparedWindowsImeSession> {
    let index = slots
        .iter()
        .position(|slot| slot.session_id == session_id)?;
    Some(slots.remove(index).prepared)
}

#[cfg(target_os = "windows")]
fn take_current_prepared_windows_ime_session_for_restore(
    slots: &mut Vec<PreparedWindowsImeSessionSlot>,
    session_id: SessionId,
    current_session_id: SessionId,
) -> Option<PreparedWindowsImeSession> {
    let prepared = take_matching_prepared_windows_ime_session(slots, session_id)?;
    if current_session_id == session_id {
        Some(prepared)
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn restore_prepared_windows_ime_session(inner: &Arc<Inner>, session_id: SessionId) {
    let state = inner.state.lock();
    let prepared = {
        let mut slot = inner.prepared_windows_ime_session.lock();
        take_current_prepared_windows_ime_session_for_restore(
            &mut slot,
            session_id,
            state.session_id,
        )
    };
    if let Some(prepared) = prepared {
        inner.windows_ime.restore_session(prepared);
    }
}

#[cfg(not(target_os = "windows"))]
fn restore_prepared_windows_ime_session(_inner: &Arc<Inner>, _session_id: SessionId) {}

#[cfg(target_os = "windows")]
async fn insert_with_windows_ime_first(
    inner: &Arc<Inner>,
    session_id: SessionId,
    delivery_id: &str,
    polished: &str,
    restore_clipboard: bool,
    allow_non_tsf_insertion_fallback: bool,
    paste_shortcut: PasteShortcut,
    ime_target: Option<ImeSubmitTarget>,
    continue_paste_route: bool,
) -> WindowsInsertionResult {
    let prepared = {
        let mut slot = inner.prepared_windows_ime_session.lock();
        take_matching_prepared_windows_ime_session(&mut slot, session_id)
    };
    let Some(prepared) = prepared else {
        log::warn!("[windows-ime] no prepared TSF session for this dictation");
        if should_try_non_tsf_insertion_fallback(
            allow_non_tsf_insertion_fallback,
            InsertStatus::Failed,
        ) {
            return insert_via_non_tsf_fallback(inner, polished, restore_clipboard, paste_shortcut);
        }
        log::warn!("[windows-ime] non-TSF insertion fallback is disabled; failing insert");
        return WindowsInsertionResult {
            status: InsertStatus::Failed,
            target_confirmed: false,
            route: DeliveryRoute::Failed,
            submitted_text: None,
        };
    };

    if continue_paste_route && allow_non_tsf_insertion_fallback {
        log::info!(
            "[windows-ime] final remainder continues submitted pause-early paste route session_id={session_id} chars={}",
            polished.chars().count()
        );
        inner.windows_ime.restore_session(prepared);
        return insert_via_non_tsf_fallback(inner, polished, restore_clipboard, paste_shortcut);
    }

    // Recording-start activate often fails (0x80004005) while many windows
    // fight the TIP. Before abandoning TSF, re-prepare once at insert time —
    // focus is usually calmer after the user finishes speaking, so true
    // insert can still land. If it still fails, jump straight to non-TSF /
    // clipboard for snappy stop→Done (owner sessions 7ce523e5 / 6a86d8e3).
    let prepared = if prepared.is_ready_for_tsf_submit() {
        prepared
    } else {
        log::info!(
            "[windows-ime] TSF not activated at recording start; retrying prepare at insert time"
        );
        inner.windows_ime.restore_session(prepared);
        let retried = inner.windows_ime.prepare_session();
        if retried.is_ready_for_tsf_submit() {
            log::info!("[windows-ime] TSF activated on insert-time retry");
            retried
        } else {
            if !should_try_non_tsf_insertion_fallback(
                allow_non_tsf_insertion_fallback,
                InsertStatus::Failed,
            ) {
                inner.windows_ime.restore_session(retried);
                log::warn!(
                    "[windows-ime] TSF retry unavailable; non-TSF insertion fallback is disabled"
                );
                return WindowsInsertionResult {
                    status: InsertStatus::Failed,
                    target_confirmed: false,
                    route: DeliveryRoute::Failed,
                    submitted_text: None,
                };
            }
            log::info!(
                "[windows-ime] TSF still not activated at insert; using non-TSF insert path immediately"
            );
            inner.windows_ime.restore_session(retried);
            return insert_via_non_tsf_fallback(inner, polished, restore_clipboard, paste_shortcut);
        }
    };

    let request = crate::windows_ime_ipc::ImeSubmitRequest {
        session_id: delivery_id.to_string(),
        text: polished.to_string(),
        created_at: Utc::now().to_rfc3339(),
        target: ime_target,
    };

    let ime_status = match inner.windows_ime.submit_prepared(&prepared, request).await {
        Ok(status) => status,
        Err(error) if error.is_session_not_active() => {
            // session not active：录音起点 prepare_session 就没激活 Listener Type profile，
            // 目标窗口仍是用户原 IME。insert_via_non_tsf_fallback 因此 paste-first：
            // Ctrl+V 走目标窗口自己的 paste handler 绕开 IME（Unicode SendInput 会被
            // 组态中的 CJK IME 吞掉且假阳性报 Inserted）。
            log::warn!("[windows-ime] TSF submit failed: {error}");
            inner.windows_ime.restore_session(prepared);
            if !should_try_non_tsf_insertion_fallback(
                allow_non_tsf_insertion_fallback,
                InsertStatus::Failed,
            ) {
                return WindowsInsertionResult {
                    status: InsertStatus::Failed,
                    target_confirmed: false,
                    route: DeliveryRoute::Failed,
                    submitted_text: None,
                };
            }
            return insert_via_non_tsf_fallback(inner, polished, restore_clipboard, paste_shortcut);
        }
        Err(error) => {
            log::warn!("[windows-ime] TSF submit failed: {error}");
            InsertStatus::Failed
        }
    };
    inner.windows_ime.restore_session(prepared);

    if ime_status == InsertStatus::Inserted {
        WindowsInsertionResult {
            status: ime_status,
            target_confirmed: true,
            route: DeliveryRoute::Tsf,
            submitted_text: Some(polished.to_string()),
        }
    } else if should_try_non_tsf_insertion_fallback(allow_non_tsf_insertion_fallback, ime_status) {
        insert_via_non_tsf_fallback(inner, polished, restore_clipboard, paste_shortcut)
    } else {
        log::warn!("[windows-ime] TSF did not insert; non-TSF insertion fallback is disabled");
        WindowsInsertionResult {
            status: InsertStatus::Failed,
            target_confirmed: false,
            route: DeliveryRoute::Tsf,
            submitted_text: Some(polished.to_string()),
        }
    }
}

#[cfg(target_os = "windows")]
fn should_try_non_tsf_insertion_fallback(
    allow_non_tsf_insertion_fallback: bool,
    ime_status: InsertStatus,
) -> bool {
    allow_non_tsf_insertion_fallback && ime_status != InsertStatus::Inserted
}

#[cfg(target_os = "windows")]
fn insert_via_non_tsf_fallback(
    inner: &Arc<Inner>,
    polished: &str,
    restore_clipboard: bool,
    paste_shortcut: PasteShortcut,
) -> WindowsInsertionResult {
    // 2026-09-19: paste first. The target window still runs the user's CJK
    // IME, and in composition states that IME swallows Unicode SendInput
    // events while SendInput still reports them injected — a false-positive
    // Inserted that loses the text and never reaches a second path (the
    // daily-use delivery log showed every route=unicode session ending
    // SubmittedUnconfirmed). A real Ctrl+V keystroke (enigo sends the scan
    // code, not VK_PACKET) goes through the target's own paste handler and is
    // IME-immune, and PasteSent is an honest completion state (green end
    // light) instead of SubmittedUnconfirmed (yellow warning). The clipboard
    // is only a transport here: restore_clipboard puts the user's original
    // content back (empty → cleared, image → put back) unless they asked to
    // retain the dictated text. Content the restore path cannot write back
    // (copied files etc.) is never borrowed: that dictation uses keystrokes.
    // IME-safe Unicode keystrokes stay as the second path for paste-hostile
    // targets.
    let paste_status = if crate::insertion::clipboard_transport_is_reversible() {
        inner
            .inserter
            .insert_via_clipboard_fallback(polished, restore_clipboard, paste_shortcut)
    } else {
        log::info!(
            "[windows-ime] clipboard holds content the restore path cannot write back; keeping it intact and using Unicode keystrokes chars={}",
            polished.chars().count()
        );
        InsertStatus::Failed
    };
    if paste_status == InsertStatus::PasteSent || paste_status == InsertStatus::Inserted {
        log::info!(
            "[windows-ime] non-TSF clipboard paste submitted status={paste_status:?} chars={}",
            polished.chars().count()
        );
        return WindowsInsertionResult {
            status: paste_status,
            target_confirmed: false,
            route: DeliveryRoute::Paste,
            submitted_text: Some(polished.to_string()),
        };
    }
    log::info!(
        "[windows-ime] non-TSF clipboard paste not clean status={paste_status:?}; falling back to IME-safe Unicode input chars={}",
        polished.chars().count()
    );
    let unicode_status = inner
        .inserter
        .insert_via_unicode_keystrokes_ime_safe(polished);
    if unicode_status == InsertStatus::Inserted {
        log::info!(
            "[windows-ime] non-TSF IME-safe Unicode input submitted without receiver confirmation chars={}",
            polished.chars().count()
        );
        return WindowsInsertionResult {
            status: InsertStatus::SubmittedUnconfirmed,
            target_confirmed: false,
            route: DeliveryRoute::Unicode,
            submitted_text: Some(polished.to_string()),
        };
    }
    log::warn!(
        "[windows-ime] non-TSF insert failed paste={paste_status:?} unicode={unicode_status:?} chars={}",
        polished.chars().count()
    );
    WindowsInsertionResult {
        status: unicode_status,
        target_confirmed: false,
        route: DeliveryRoute::Unicode,
        submitted_text: None,
    }
}

// ─────────────────────────── helpers ───────────────────────────

#[cfg(any(debug_assertions, test))]
fn hotkey_injection_dry_run_enabled() -> bool {
    std::env::var_os("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN").is_some()
}

#[cfg(any(debug_assertions, test))]
fn debug_transcript_override_text() -> Option<String> {
    let path = std::env::var_os("LISTENER_TYPE_DEBUG_TRANSCRIPT_FILE")?;
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn ensure_microphone_permission(_inner: &Arc<Inner>) -> Result<(), String> {
    use crate::permissions;
    #[cfg(not(target_os = "windows"))]
    use crate::permissions::PermissionStatus;

    #[cfg(target_os = "windows")]
    {
        if permissions::windows_microphone_access_explicitly_denied() {
            return Err("需要麦克风权限，当前状态: Denied".to_string());
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let status = permissions::check_microphone();
        if matches!(
            status,
            PermissionStatus::Granted | PermissionStatus::NotApplicable
        ) {
            return Ok(());
        }

        let requested = permissions::request_microphone();
        if matches!(
            requested,
            PermissionStatus::Granted | PermissionStatus::NotApplicable
        ) {
            Ok(())
        } else {
            Err(format!("需要麦克风权限，当前状态: {requested:?}"))
        }
    }
}

fn active_asr_provider_from_preferences(inner: &Arc<Inner>) -> String {
    let prefs_provider = inner.prefs.get().active_asr_provider;
    let prefs_provider = prefs_provider.trim();
    if prefs_provider.is_empty() {
        return CredentialsVault::get_active_asr();
    }
    prefs_provider.to_string()
}

fn sync_active_asr_provider_to_credentials_for_runtime(active_asr: &str) {
    let vault_active_asr = CredentialsVault::get_active_asr();
    if vault_active_asr == active_asr {
        return;
    }
    match CredentialsVault::set_active_asr_provider(active_asr) {
        Ok(()) => log::warn!(
            "[coord] active ASR provider drift corrected for runtime prefs={active_asr} vault={vault_active_asr}"
        ),
        Err(err) => log::warn!(
            "[coord] active ASR provider drift detected but credential sync failed prefs={active_asr} vault={vault_active_asr}: {err}"
        ),
    }
}

fn ensure_asr_credentials(active_asr: &str) -> Result<(), String> {
    // 本地 Qwen3-ASR 没有"凭据"概念，但需要：(a) macOS 平台 (b) 模型已下载。
    if crate::asr::local::is_local_qwen3(active_asr) {
        #[cfg(not(target_os = "macos"))]
        {
            return Err("本地 ASR 当前仅支持 macOS（Windows 见 issue #256）".to_string());
        }
        #[cfg(target_os = "macos")]
        {
            return ensure_local_qwen3_model_ready();
        }
    }

    if crate::asr::local::foundry::is_foundry_local_whisper(active_asr) {
        #[cfg(not(target_os = "windows"))]
        {
            return Err("Foundry Local Whisper 当前仅支持 Windows".to_string());
        }
        #[cfg(target_os = "windows")]
        {
            return Ok(());
        }
    }

    if is_whisper_compatible_provider(active_asr) || is_bailian_provider(active_asr) {
        let api_key = CredentialsVault::get(CredentialAccount::AsrApiKey)
            .ok()
            .flatten()
            .unwrap_or_default();
        if api_key.trim().is_empty() {
            return Err("请先在设置中填写 ASR 服务商 API Key".to_string());
        }
        return Ok(());
    }

    let creds = read_volc_credentials();
    if creds.app_id.trim().is_empty() || creds.access_token.trim().is_empty() {
        Err("请先在设置中填写火山引擎 ASR App Key 和 Access Key".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
fn is_keyless_local_asr_provider(id: &str) -> bool {
    if crate::asr::local::is_local_qwen3(id) {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        crate::asr::local::foundry::is_foundry_local_whisper(id)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = id;
        false
    }
}

#[cfg(target_os = "macos")]
fn ensure_local_qwen3_model_ready() -> Result<(), String> {
    let prefs = || -> Result<crate::types::UserPreferences, String> {
        // 这里没法拿到 inner，直接读 preferences.json 即可（Coordinator 写盘后总是同步的）。
        crate::persistence::PreferencesStore::new()
            .map_err(|e| e.to_string())
            .map(|s| s.get())
    }()?;
    let model_id = crate::asr::local::ModelId::from_str(&prefs.local_asr_active_model)
        .ok_or_else(|| format!("未知的本地模型 id: {}", prefs.local_asr_active_model))?;
    if !crate::asr::local::models::is_downloaded(model_id) {
        return Err(format!(
            "本地模型 {} 未下载完整，请到 设置 → 模型设置 中下载",
            model_id.as_str()
        ));
    }
    Ok(())
}

/// 一次 dictation 结束后，按 prefs.local_asr_keep_loaded_secs 决定何时释放
/// 内存里的 Qwen3-ASR 引擎。0 = 立即释放；其它值 = sleep N 秒后看 last_used。
/// 多次会话叠加多个 sleep 任务，每个独立 check：只要中间又被使用过就跳过释放。
fn schedule_local_asr_release(inner: &Arc<Inner>) {
    let keep_secs = inner.prefs.get().local_asr_keep_loaded_secs;
    let cache = Arc::clone(&inner.local_asr_cache);
    if keep_secs == 0 {
        cache.release_now();
        return;
    }
    let dur = std::time::Duration::from_secs(keep_secs as u64);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(dur).await;
        cache.release_if_idle(dur);
    });
}

#[cfg(target_os = "windows")]
fn foundry_local_asr_release_keep_secs(inner: &Arc<Inner>) -> u32 {
    inner.prefs.get().foundry_local_asr_keep_loaded_secs
}

#[cfg(target_os = "windows")]
fn foundry_release_session_is_current(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    inner.state.lock().session_id == session_id
}

#[cfg(target_os = "windows")]
fn schedule_foundry_local_asr_release(inner: &Arc<Inner>, session_id: SessionId) {
    let keep_secs = foundry_local_asr_release_keep_secs(inner);
    let runtime = Arc::clone(&inner.foundry_local_runtime);
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn(async move {
        if keep_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(keep_secs as u64)).await;
        }
        if !foundry_release_session_is_current(&inner, session_id) {
            return;
        }
        if let Err(error) = runtime.release_now().await {
            log::warn!("[foundry-asr] scheduled release failed: {error:#}");
        }
    });
}

#[cfg(target_os = "macos")]
async fn build_local_qwen3(
    inner: &Arc<Inner>,
) -> anyhow::Result<Arc<crate::asr::local::LocalQwenAsr>> {
    let prefs = inner.prefs.get();
    let model_id = crate::asr::local::ModelId::from_str(&prefs.local_asr_active_model)
        .ok_or_else(|| anyhow::anyhow!("未知本地模型 id: {}", prefs.local_asr_active_model))?;
    let dir = crate::asr::local::models::model_dir(model_id)?;
    let app = inner
        .app
        .lock()
        .clone()
        .ok_or_else(|| anyhow::anyhow!("AppHandle 未绑定"))?;
    // 走缓存：如果已有同 id 的引擎在内存里就直接复用，避免每次会话都重加载
    // 1.2GB+ 模型。第一次加载阻塞数秒，spawn_blocking 不卡 tokio runtime。
    let cache = Arc::clone(&inner.local_asr_cache);
    let mid = model_id.as_str().to_string();
    let engine = tauri::async_runtime::spawn_blocking(move || cache.get_or_load(&mid, &dir))
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join failed: {e:#}"))??;
    Ok(Arc::new(crate::asr::local::LocalQwenAsr::new(app, engine)))
}

/// `whisper` 是 OpenAI 原生；`siliconflow` / `zhipu` / `groq` 都暴露
/// OpenAI 兼容的 `/audio/transcriptions`，统一走 `WhisperBatchASR`。
/// 新增 OpenAI 兼容 ASR 时只需在这里加一项。
///
/// 注：DashScope 的 Qwen3-ASR-Flash 不在此列——它用 MultiModalConversation
/// (messages=[{content:[{audio:...}]}]) 协议，不是 Whisper multipart，需要
/// 单独 ASR 客户端，留给 V2。
fn is_whisper_compatible_provider(id: &str) -> bool {
    matches!(id, "whisper" | "siliconflow" | "zhipu" | "groq")
}

fn is_bailian_provider(id: &str) -> bool {
    id == crate::asr::bailian::PROVIDER_ID
}

fn apply_chinese_script_preference(text: &str, pref: ChineseScriptPreference) -> String {
    if text.is_empty() {
        return String::new();
    }
    let config = match pref {
        ChineseScriptPreference::Simplified => Some(BuiltinConfig::T2s),
        ChineseScriptPreference::Traditional => Some(BuiltinConfig::S2t),
        ChineseScriptPreference::Auto => None,
    };
    let Some(config) = config else {
        return text.to_string();
    };
    match OpenCC::from_config(config) {
        Ok(converter) => converter.convert(text),
        Err(err) => {
            log::warn!("[coord] OpenCC init failed, skip script conversion: {err}");
            text.to_string()
        }
    }
}

/// QA 路径专用：begin_qa_session 永远走 Volcengine 流式（低延迟要求），所以
/// 凭据校验也只看 Volcengine 字段，不依赖 active_asr。dictation 路径请用
/// `ensure_asr_credentials`。
fn ensure_qa_volcengine_credentials() -> Result<(), String> {
    let creds = read_volc_credentials();
    if creds.app_id.trim().is_empty() || creds.access_token.trim().is_empty() {
        Err("请先在设置中填写火山引擎 ASR App Key 和 Access Key".to_string())
    } else {
        Ok(())
    }
}

/// 润色文本；失败时返回原文 + 失败原因，调用方据此弹错误胶囊 + 写历史 error_code。
/// 之前固定返回 String，调用方拿不到失败信号 → 用户感知"为什么风格设置没生效"。issue #57。
/// 流式润色的三态结果。让上层（dictation pipeline）能区分「已经流出去了」、
/// 「降级到一次性」和「真失败了走 raw 兜底」三种 case。
pub enum StreamingPolishOutcome {
    /// 流式润色成功，`String` 是已经一边流一边交给 `on_delta` 的全部文本（用于写
    /// history、做词条命中统计）。调用方不应再 `inserter.insert(&text)`，因为字符
    /// 已经通过键盘事件落到光标处。
    Streamed(String),
    /// 当前配置不支持流式：用户没开 streaming_insert / Gemini provider / Codex
    /// provider / Raw 模式 / 翻译模式 / 不是 macOS。调用方应回到现有的
    /// `polish_or_passthrough` 一次性路径，跟历史行为完全一致。
    UnsupportedFallback,
    /// 流式过程中失败（HTTP / 解析 / 空流等）。`String` 是失败原因，调用方应当
    /// 走 raw 兜底（同 `polish_or_passthrough` 失败分支的语义）。
    Failed(String),
}

/// 预热润色流的共享状态。endpoint 触发即发起；被采用前 delta 只进缓冲区，
/// 绝不上屏——采用时先回放缓冲再切 live；不采用则置 cancel 丢弃。
pub(crate) struct PolishPrefetch {
    /// 预热输入文本（endpoint 时刻的预览 + 纠错规则变换）。终稿与之相等才采用。
    pub(crate) input: String,
    pub(crate) buf: Arc<Mutex<PolishPrefetchBuf>>,
    pub(crate) notify: Arc<tokio::sync::Notify>,
    pub(crate) cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct PolishPrefetchBuf {
    pub(crate) chunks: std::collections::VecDeque<String>,
    pub(crate) result: Option<StreamingPolishOutcome>,
}

impl PolishPrefetch {
    pub(crate) fn failed(&self) -> bool {
        matches!(
            self.buf.lock().result,
            Some(StreamingPolishOutcome::Failed(_))
        )
    }
}

#[derive(Default)]
struct LlmAuthFailureCircuit {
    rejected_fingerprint: Option<u64>,
    /// 每个凭据指纹只发一次用户可见提示（熔断后静默原文是合同，但「401 一整天
    /// 用户无感知」也是事故——2026-08-05 owner 整天没润色却不知情）。
    notice_sent_fingerprint: Option<u64>,
}

impl LlmAuthFailureCircuit {
    fn rejects(&self, fingerprint: u64) -> bool {
        self.rejected_fingerprint == Some(fingerprint)
    }

    fn reject(&mut self, fingerprint: u64) {
        self.rejected_fingerprint = Some(fingerprint);
    }

    /// 熔断打开且这组凭据还没发过可见提示时，取走一次「应提示」资格。
    fn take_notice(&mut self, fingerprint: u64) -> bool {
        if self.rejected_fingerprint == Some(fingerprint)
            && self.notice_sent_fingerprint != Some(fingerprint)
        {
            self.notice_sent_fingerprint = Some(fingerprint);
            return true;
        }
        false
    }
}

static LLM_AUTH_FAILURE_CIRCUIT: OnceLock<Mutex<LlmAuthFailureCircuit>> = OnceLock::new();

fn current_llm_auth_fingerprint() -> anyhow::Result<u64> {
    let mut hasher = DefaultHasher::new();
    CredentialsVault::get_active_llm().hash(&mut hasher);
    CredentialsVault::get(CredentialAccount::ArkApiKey)?.hash(&mut hasher);
    CredentialsVault::get(CredentialAccount::ArkModelId)?.hash(&mut hasher);
    CredentialsVault::get(CredentialAccount::ArkEndpoint)?.hash(&mut hasher);
    Ok(hasher.finish())
}

fn llm_auth_failure_circuit() -> &'static Mutex<LlmAuthFailureCircuit> {
    LLM_AUTH_FAILURE_CIRCUIT.get_or_init(|| Mutex::new(LlmAuthFailureCircuit::default()))
}

fn current_llm_auth_is_rejected(fingerprint: u64) -> bool {
    llm_auth_failure_circuit().lock().rejects(fingerprint)
}

fn note_llm_auth_rejection(fingerprint: u64) {
    llm_auth_failure_circuit().lock().reject(fingerprint);
}

/// 熔断处于打开状态且这组凭据还没发过可见提示时，取走一次「应提示」资格。
/// 返回 true 仅一次；换 key（新指纹）后会重新允许提示。
fn take_llm_auth_rejection_notice(fingerprint: u64) -> bool {
    llm_auth_failure_circuit().lock().take_notice(fingerprint)
}

fn llm_error_is_auth_rejection(error: &LLMError) -> bool {
    matches!(
        error,
        LLMError::InvalidResponse {
            status: 401 | 403,
            ..
        }
    )
}

// ── LLM stall circuit（非 auth 的连续失败熔断）──
//
// 与 auth 熔断同构但面向 provider 抽风：deepseek 连续空转/网络失败时，每次
// 听写都要白等 8s 空转超时（2026-08-07 owner 连续 3 句 ×8.4s）。2 次连续失败
// 开闸 120s，期间润色直接跳过走原文；到期半开允许一次尝试，成功即复位。
// auth 失败不计入（401/403 走上面的 auth 熔断）。

#[derive(Default)]
struct LlmStallCircuit {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

impl LlmStallCircuit {
    fn is_open(&self, now: Instant) -> bool {
        self.open_until.is_some_and(|until| until > now)
    }

    fn note_success(&mut self) {
        self.consecutive_failures = 0;
        self.open_until = None;
    }

    fn note_failure(&mut self, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= LLM_STALL_OPEN_AFTER_FAILURES {
            self.open_until = Some(now + LLM_STALL_OPEN_DURATION);
        }
    }
}

const LLM_STALL_OPEN_AFTER_FAILURES: u32 = 2;
const LLM_STALL_OPEN_DURATION: Duration = Duration::from_secs(120);
static LLM_STALL_CIRCUIT: OnceLock<Mutex<LlmStallCircuit>> = OnceLock::new();

fn llm_stall_circuit() -> &'static Mutex<LlmStallCircuit> {
    LLM_STALL_CIRCUIT.get_or_init(|| Mutex::new(LlmStallCircuit::default()))
}

fn llm_stall_circuit_open() -> bool {
    llm_stall_circuit().lock().is_open(Instant::now())
}

fn note_llm_polish_success() {
    llm_stall_circuit().lock().note_success();
}

fn note_llm_polish_stall_failure() {
    let circuit = llm_stall_circuit();
    let mut circuit = circuit.lock();
    let before_open = circuit.is_open(Instant::now());
    circuit.note_failure(Instant::now());
    if !before_open && circuit.is_open(Instant::now()) {
        log::warn!(
            "[coord] LLM stall circuit open after {} consecutive failures; raw insert for {}s",
            circuit.consecutive_failures,
            LLM_STALL_OPEN_DURATION.as_secs()
        );
    }
}

/// 流式润色入口。在不支持流式的所有 case 都返回 `UnsupportedFallback`，让调用方
/// 透明降级。不修改任何持久化 / 焦点 / 光标状态。
///
/// `on_delta` 每收到一个 SSE chunk 就被调用一次（同步），调用方负责把 chunk 实际
/// 模拟键盘事件落到光标 —— 见 `coordinator/dictation.rs` 的流式分支。
/// `should_cancel` 用户取消时返回 true，立即 break SSE 读循环避免烧 quota。
pub async fn polish_or_passthrough_streaming<F, C>(
    raw: &RawTranscript,
    mode: PolishMode,
    hotwords: &[String],
    style_system_prompt: &str,
    working_languages: &[String],
    chinese_script_preference: ChineseScriptPreference,
    output_language_preference: OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
    prior_turns: &[(String, String)],
    on_delta: F,
    should_cancel: C,
) -> StreamingPolishOutcome
where
    F: Fn(&str) + Send + Sync,
    C: Fn() -> bool + Send + Sync,
{
    if mode == PolishMode::Raw && !raw_mode_uses_llm(style_system_prompt) {
        log::info!("[coord] streaming polish skipped: mode=Raw, fall back to one-shot");
        return StreamingPolishOutcome::UnsupportedFallback;
    }
    let active_llm = CredentialsVault::get_active_llm();
    if active_llm == "gemini" {
        log::info!(
            "[coord] streaming polish skipped: active LLM provider=gemini (v1 not implemented), fall back to one-shot"
        );
        return StreamingPolishOutcome::UnsupportedFallback;
    }
    let auth_fingerprint = current_llm_auth_fingerprint().ok();
    if auth_fingerprint.is_some_and(current_llm_auth_is_rejected) {
        log::warn!(
            "[coord] streaming polish skipped: current LLM credentials were already rejected; using raw text without another network wait"
        );
        return StreamingPolishOutcome::Failed(
            "current LLM credentials were already rejected".to_string(),
        );
    }
    let provider = match build_active_llm_provider(llm_thinking_enabled) {
        Ok(p) => p,
        Err(e) => {
            log::error!("[coord] streaming polish: build provider failed: {e}");
            return StreamingPolishOutcome::Failed(e.to_string());
        }
    };
    if !provider.supports_streaming_polish() {
        log::info!(
            "[coord] streaming polish skipped: provider does not support streaming (likely codex OAuth), fall back to one-shot"
        );
        return StreamingPolishOutcome::UnsupportedFallback;
    }
    log::info!(
        "[coord] streaming polish START: provider=openai-compatible mode={:?} raw_chars={} prior_turns={}",
        mode,
        raw.text.chars().count(),
        prior_turns.len()
    );
    match provider
        .polish_streaming(
            &raw.text,
            mode,
            hotwords,
            style_system_prompt,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            front_app,
            prior_turns,
            on_delta,
            should_cancel,
        )
        .await
    {
        Ok(text) => {
            log::info!(
                "[coord] streaming polish OK: final_chars={}",
                text.chars().count()
            );
            StreamingPolishOutcome::Streamed(text)
        }
        Err(e) => {
            if llm_error_is_auth_rejection(&e) {
                if let Some(fingerprint) = auth_fingerprint {
                    note_llm_auth_rejection(fingerprint);
                }
            }
            let reason = e.to_string();
            log::error!("[coord] streaming polish FAILED: {reason}");
            StreamingPolishOutcome::Failed(reason)
        }
    }
}

async fn polish_or_passthrough(
    raw: &RawTranscript,
    mode: PolishMode,
    hotwords: &[String],
    style_system_prompt: &str,
    working_languages: &[String],
    chinese_script_preference: ChineseScriptPreference,
    output_language_preference: OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
    prior_turns: &[(String, String)],
) -> (String, Option<String>) {
    if mode == PolishMode::Raw && !raw_mode_uses_llm(style_system_prompt) {
        return (raw.text.clone(), None);
    }
    match polish_text(
        &raw.text,
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
    .await
    {
        Ok(s) => (s, None),
        Err(e) => {
            let reason = e.to_string();
            log::error!("[coord] polish failed, falling back to raw: {reason}");
            (raw.text.clone(), Some(reason))
        }
    }
}

async fn polish_text(
    raw: &str,
    mode: PolishMode,
    hotwords: &[String],
    style_system_prompt: &str,
    working_languages: &[String],
    chinese_script_preference: ChineseScriptPreference,
    output_language_preference: OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
    prior_turns: &[(String, String)],
) -> anyhow::Result<String> {
    // 谷歌 Gemini 分支：所有 LLM provider 共用 ark.* 凭据槽，唯独 Gemini 走原生
    // generateContent / 自带 thinkingConfig 控制；其余 provider 走 OpenAI
    // 兼容协议，并在该路径里按 provider/channel 下发对应的思考开关。
    let active_llm = CredentialsVault::get_active_llm();
    if active_llm == "gemini" {
        let (api_key, model, base_url) = read_gemini_credentials()?;
        let proxy_config = read_llm_proxy_config(&active_llm)?;
        let provider = GeminiProvider::new(
            GeminiConfig::new(api_key, model, base_url)
                .with_thinking_enabled(llm_thinking_enabled)
                .with_proxy_config(proxy_config),
        );
        return Ok(provider
            .polish(
                raw,
                mode,
                hotwords,
                style_system_prompt,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                front_app,
                prior_turns,
            )
            .await?);
    }

    let auth_fingerprint = current_llm_auth_fingerprint()?;
    if current_llm_auth_is_rejected(auth_fingerprint) {
        anyhow::bail!("current LLM credentials were already rejected");
    }
    let provider = build_active_llm_provider(llm_thinking_enabled)?;
    let result = provider
        .polish(
            raw,
            mode,
            hotwords,
            style_system_prompt,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            front_app,
            prior_turns,
        )
        .await;
    if result.as_ref().is_err_and(llm_error_is_auth_rejection) {
        note_llm_auth_rejection(auth_fingerprint);
    }
    Ok(result?)
}

/// 翻译路径——和 polish 一样失败时返回原文 + 失败原因，避免"不丢字"约定被违反（CLAUDE.md）。
async fn translate_or_passthrough(
    raw: &RawTranscript,
    target_language: &str,
    working_languages: &[String],
    chinese_script_preference: ChineseScriptPreference,
    output_language_preference: OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
) -> (String, Option<String>) {
    match translate_text(
        &raw.text,
        target_language,
        working_languages,
        chinese_script_preference,
        output_language_preference,
        llm_thinking_enabled,
        front_app,
    )
    .await
    {
        Ok(s) => (s, None),
        Err(e) => {
            let reason = e.to_string();
            log::error!("[coord] translate failed, falling back to raw: {reason}");
            (raw.text.clone(), Some(reason))
        }
    }
}

async fn translate_text(
    raw: &str,
    target_language: &str,
    working_languages: &[String],
    chinese_script_preference: ChineseScriptPreference,
    output_language_preference: OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
) -> anyhow::Result<String> {
    // 见 polish_text 顶部注释——同样的 Gemini / OpenAI-compatible 路由逻辑。
    let active_llm = CredentialsVault::get_active_llm();
    if active_llm == "gemini" {
        let (api_key, model, base_url) = read_gemini_credentials()?;
        let proxy_config = read_llm_proxy_config(&active_llm)?;
        let provider = GeminiProvider::new(
            GeminiConfig::new(api_key, model, base_url)
                .with_thinking_enabled(llm_thinking_enabled)
                .with_proxy_config(proxy_config),
        );
        return Ok(provider
            .translate_to(
                raw,
                target_language,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                front_app,
            )
            .await?);
    }

    let provider = build_active_llm_provider(llm_thinking_enabled)?;
    Ok(provider
        .translate_to(
            raw,
            target_language,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            front_app,
        )
        .await?)
}

fn read_whisper_credentials(
    active_asr: &str,
) -> anyhow::Result<(String, String, String, ProviderProxyConfig)> {
    let api_key = CredentialsVault::get(CredentialAccount::AsrApiKey)
        .ok()
        .flatten()
        .unwrap_or_default();
    let base_url = CredentialsVault::get(CredentialAccount::AsrEndpoint)
        .ok()
        .flatten()
        .unwrap_or_default();
    let model = CredentialsVault::get(CredentialAccount::AsrModel)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "whisper-1".to_string());
    let proxy_config = read_asr_proxy_config(&active_asr)?;
    Ok((api_key, base_url, model, proxy_config))
}

fn read_bailian_credentials() -> BailianCredentials {
    let api_key = CredentialsVault::get(CredentialAccount::AsrApiKey)
        .ok()
        .flatten()
        .unwrap_or_default();
    let endpoint = CredentialsVault::get(CredentialAccount::AsrEndpoint)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| crate::asr::bailian::DEFAULT_ENDPOINT.to_string());
    let model = CredentialsVault::get(CredentialAccount::AsrModel)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| crate::asr::bailian::DEFAULT_MODEL.to_string());
    let vocabulary_id = CredentialsVault::get(CredentialAccount::AsrVocabularyId)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty());
    BailianCredentials {
        api_key,
        endpoint,
        model,
        vocabulary_id,
    }
}

fn read_volc_credentials() -> VolcengineCredentials {
    #[cfg(target_os = "windows")]
    if let Err(err) = CredentialsVault::refresh_from_system() {
        // Keep the last known-good cache available during a transient Windows
        // Credential Manager failure, but make the stale-read risk observable.
        log::warn!("[asr] refresh Volcengine credentials from system vault failed: {err}");
    }
    let app_id = CredentialsVault::get(CredentialAccount::VolcengineAppKey)
        .ok()
        .flatten()
        .unwrap_or_default();
    let access_token = CredentialsVault::get(CredentialAccount::VolcengineAccessKey)
        .ok()
        .flatten()
        .unwrap_or_default();
    let resource_id = CredentialsVault::get(CredentialAccount::VolcengineResourceId)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| VolcengineCredentials::default_resource_id().to_string());
    VolcengineCredentials {
        app_id,
        access_token,
        resource_id,
    }
}

fn enabled_hotwords(inner: &Arc<Inner>) -> Vec<DictionaryHotword> {
    let mut hotwords: Vec<DictionaryHotword> = inner
        .vocab
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|e| DictionaryHotword {
            phrase: e.phrase,
            enabled: e.enabled,
        })
        .collect();
    append_extra_asr_hotwords(&mut hotwords);
    hotwords
}

fn extra_asr_hotword_phrases() -> Vec<String> {
    let Ok(raw) = std::env::var(EXTRA_ASR_HOTWORDS_ENV) else {
        return Vec::new();
    };
    raw.split(|ch: char| matches!(ch, ',' | ';' | '|' | '\n' | '\r' | '\t'))
        .map(str::trim)
        .filter(|phrase| !phrase.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn append_extra_asr_hotwords(hotwords: &mut Vec<DictionaryHotword>) {
    for phrase in extra_asr_hotword_phrases() {
        if hotwords
            .iter()
            .any(|entry| entry.enabled && entry.phrase == phrase)
        {
            continue;
        }
        hotwords.push(DictionaryHotword {
            phrase,
            enabled: true,
        });
    }
}

// ─────────────────────────── QA session lifecycle ───────────────────────────

/// 划词语音问答会话（issue #118）。
///
/// 与 dictation 完全分离：
/// - 不进 SessionPhase（互不抢锁）
/// - 不写 history.json（除非 prefs.qa_save_history=true 才旁路写一条 placeholder）
/// - 用独立的 qa_recorder + qa_asr，复用现有 Volcengine ASR 通路
async fn begin_qa_session(inner: &Arc<Inner>) -> Result<(), String> {
    {
        let mut state = inner.qa_state.lock();
        if !state.panel_visible {
            // 防御：浮窗没开就被叫到这里说明路由错了，直接退出。
            return Ok(());
        }
        if state.phase != QaPhase::Idle {
            return Ok(());
        }
        state.phase = QaPhase::Recording;
        state.cancelled = false;
        state.session_id = new_session_id();
        state.front_app = capture_frontmost_app();
        state.selection = None;
    }
    // 重置 SSE 取消标志：上一轮可能 set 过的 true 留着会让本轮流式立即 break。
    inner.qa_stream_cancelled.store(false, Ordering::SeqCst);

    // 抓选区。每轮按 Option 都重新抓一次：用户多轮提问中可以重新选别处文字。
    // 浮窗 focus:false，原 app 仍是 frontmost，AX/Cmd+C fallback 都能拿到。
    let selection = capture_selection();
    let selection_preview_text = selection.as_ref().map(|s| s.text.clone());
    inner.qa_state.lock().selection = selection.clone();

    if let Some(app) = inner.app.lock().clone() {
        let messages = inner.qa_state.lock().messages.clone();
        let _ = app.emit_to(
            "qa",
            "qa:state",
            serde_json::json!({
                "kind": "recording",
                "selection_preview": selection_preview_text,
                "messages": messages,
            }),
        );
    }

    // 2. 凭据缺失走静默 fallback：与 dictation 一致的"用户的话不丢"约定。
    //    缺火山凭据 → 后续 Recorder 仍会跑，只是 ASR 拿不到结果，end_qa_session
    //    会发 idle 事件关浮窗。
    //    注意：QA 强制走 Volcengine 流式（见下方注释），所以这里必须直接校验
    //    Volcengine 字段，不能复用 `ensure_asr_credentials`——后者会按用户在设置
    //    里选的 active_asr 走 OpenAI 兼容分支，让 QA 把 `asr.api_key` 当成必要项，
    //    或在 Volcengine 凭据其实为空时误判通过。Codex P1，PR #213。
    if let Err(message) = ensure_qa_volcengine_credentials() {
        log::warn!("[coord] QA: ASR credentials missing: {message}");
        finish_qa_with_error(inner, format!("缺少 ASR 凭据：{message}"));
        return Err(message);
    }

    if let Err(message) = ensure_microphone_permission(inner) {
        log::warn!("[coord] QA: microphone permission gate failed: {message}");
        finish_qa_with_error(inner, message.clone());
        return Err(message);
    }

    // 3. 启动 Recorder + ASR（强制走 Volcengine 流式：QA 必须低延迟）。
    let hotwords = enabled_hotwords(inner);
    let creds = read_volc_credentials();
    let asr = Arc::new(VolcengineStreamingASR::new(creds, hotwords));
    let bridge = Arc::new(DeferredAsrBridge::new());
    let consumer: Arc<dyn crate::recorder::AudioConsumer> = bridge.clone();
    *inner.qa_asr.lock() = Some(Arc::clone(&asr));

    // QA recorder 不需要 RMS 节流到胶囊；前端 QA 浮窗有自己的电平视图，
    // 这里发一份事件给 "qa" label 用就够了。
    let inner_for_level = Arc::clone(inner);
    let last_emit_at = Arc::new(Mutex::new(None::<Instant>));
    const LEVEL_EMIT_MIN_INTERVAL_MS: u64 = 33;
    let level_handler: Arc<dyn Fn(f32) + Send + Sync> = Arc::new(move |level| {
        let phase = inner_for_level.qa_state.lock().phase;
        if phase != QaPhase::Recording {
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
        if let Some(app) = inner_for_level.app.lock().clone() {
            let _ = app.emit_to("qa", "qa:level", serde_json::json!({ "level": level }));
        }
        // 同步把电平推给底部胶囊，让 QA 录音也有跟主听写一致的可视反馈。
        emit_capsule(
            &inner_for_level,
            CapsuleState::Recording,
            level,
            0,
            None,
            None,
        );
    });

    let microphone_device_name = selected_microphone_device_name(inner);
    stop_microphone_preview_monitor(inner, "QA recorder");
    acquire_recording_mute(inner, "qa").await;
    // QA 默认不留痕（qa_save_history 默认 false），录音文件归档也跟着不开。
    // 调试 QA 麦克风请用主听写路径。
    match Recorder::start(microphone_device_name, consumer, level_handler, None) {
        Ok((rec, runtime_errors, archive_active)) => {
            // QA 路径不写 dictation 的 history，但仍把 archive 状态归零，避免 dictation
            // 接力时读到上一个 QA session 的过期值。
            inner
                .audio_archive_active
                .store(archive_active, std::sync::atomic::Ordering::Relaxed);
            *inner.qa_recorder.lock() = Some(rec);
            // QA 也跟主听写一样监听 cpal runtime error。设备中途消失 / panic 时
            // 不能让 QA 永远卡在 Recording 没反馈。详见 issue #168。
            spawn_qa_recorder_error_monitor(inner, runtime_errors);
        }
        Err(e) => {
            log::error!("[coord] QA recorder start failed: {e}");
            inner.qa_asr.lock().take();
            release_recording_mute(inner, "qa");
            finish_qa_with_error(inner, format!("录音启动失败: {e}"));
            return Err(e.to_string());
        }
    }

    if let Err(e) = asr.open_session().await {
        log::error!("[coord] QA: open ASR session failed: {e}");
        stop_qa_recorder(inner);
        if let Some(asr) = inner.qa_asr.lock().take() {
            asr.cancel();
        }
        finish_qa_with_error(inner, format!("ASR 连接失败: {e}"));
        return Err(e.to_string());
    }

    // cancel race：在 await 期间用户可能 dismiss 了浮窗。
    if inner.qa_state.lock().cancelled {
        log::info!("[coord] QA cancel raced during open_session — aborting begin");
        asr.cancel();
        stop_qa_recorder(inner);
        inner.qa_state.lock().phase = QaPhase::Idle;
        return Ok(());
    }

    let target: Arc<dyn crate::asr::AudioConsumer> = asr;
    let flushed = bridge.attach(target);
    log::info!("[coord] QA ASR connected; flushed {flushed} deferred audio bytes");

    // 显式弹胶囊到 Recording。level_handler 后续会持续推电平，胶囊里"录音中…"
    // 的视觉反馈跟主听写完全一致。
    emit_capsule(inner, CapsuleState::Recording, 0.0, 0, None, None);

    Ok(())
}

async fn end_qa_session(inner: &Arc<Inner>) -> Result<(), String> {
    {
        let mut state = inner.qa_state.lock();
        if state.phase != QaPhase::Recording {
            return Ok(());
        }
        state.phase = QaPhase::Processing;
    }

    // 胶囊进入 Transcribing：用户视觉上看到"识别中"。
    emit_capsule(inner, CapsuleState::Transcribing, 0.0, 0, None, None);

    if let Some(app) = inner.app.lock().clone() {
        let _ = app.emit_to("qa", "qa:state", serde_json::json!({ "kind": "loading" }));
    }

    stop_qa_recorder(inner);

    let asr = match inner.qa_asr.lock().take() {
        Some(a) => a,
        None => {
            inner.qa_state.lock().phase = QaPhase::Idle;
            return Ok(());
        }
    };

    if let Err(e) = asr.send_last_frame().await {
        log::error!("[coord] QA: send last frame failed: {e}");
    }
    // 添加全局超时保护：防止 await_final_result() 永远挂起
    let timeout_duration = std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS);
    let raw = match tokio::time::timeout(timeout_duration, asr.await_final_result()).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            log::error!("[coord] QA: await final failed: {e}");
            finish_qa_with_error(inner, format!("识别失败: {e}"));
            return Err(e.to_string());
        }
        Err(_) => {
            // 全局超时：最后的防线
            log::error!(
                "[coord] QA: 全局超时 {} 秒 - 强制恢复",
                COORDINATOR_GLOBAL_TIMEOUT_SECS
            );
            // 清理 ASR session，避免资源泄漏
            asr.cancel();
            finish_qa_with_error(inner, "识别超时".to_string());
            return Err("global timeout".to_string());
        }
    };

    // cancel race：用户在 transcribe 中按 Esc / dismiss → 静默退出。
    if inner.qa_state.lock().cancelled {
        log::info!("[coord] QA cancel detected after ASR — discarding transcript");
        finish_qa_idle_silently(inner);
        return Ok(());
    }

    let question = raw.text.trim().to_string();
    if question.is_empty() {
        // 静默录音：不调 LLM，不弹错误，直接关浮窗。
        log::info!("[coord] QA: empty transcript → silent dismiss");
        finish_qa_idle_silently(inner);
        return Ok(());
    }

    // 拼这一轮的 user 消息：第一轮（messages 还空）把选区原文嵌进去；
    // 之后的轮次只送提问，让 LLM 顺着上下文回答。详见 issue #118 v2。
    let user_content = {
        let st = inner.qa_state.lock();
        let is_first_turn = st.messages.is_empty();
        let sel_text = st
            .selection
            .as_ref()
            .map(|s| s.text.clone())
            .unwrap_or_default();
        if is_first_turn && !sel_text.trim().is_empty() {
            format!(
                "# 选区原文\n{}\n\n# 我的问题\n{}",
                sel_text.trim(),
                question
            )
        } else {
            question.clone()
        }
    };

    inner
        .qa_state
        .lock()
        .messages
        .push(crate::types::QaChatMessage {
            role: "user".to_string(),
            content: user_content,
        });

    if let Some(app) = inner.app.lock().clone() {
        let messages = inner.qa_state.lock().messages.clone();
        let _ = app.emit_to(
            "qa",
            "qa:state",
            serde_json::json!({
                "kind": "thinking",
                "messages": messages,
            }),
        );
    }

    // 胶囊：思考阶段（复用 dictation 的 Polishing 状态——视觉上是"润色中"，QA 借用一下）。
    emit_capsule(inner, CapsuleState::Polishing, 0.0, 0, None, None);

    let prefs = inner.prefs.get();
    let working_languages = prefs.working_languages.clone();
    let chinese_script_preference = prefs.chinese_script_preference;
    let output_language_preference = prefs.output_language_preference;
    let llm_thinking_enabled = prefs.llm_thinking_enabled;
    let (messages_for_llm, front_app) = {
        let st = inner.qa_state.lock();
        (st.messages.clone(), st.front_app.clone())
    };

    // 流式回调：每个 SSE delta 立刻推一帧 qa:state{kind:"answer_delta"} 给前端，
    // 浮窗里气泡边收边长。最终的 messages 由 answer 事件统一下发（保证一致性）。
    //
    // session_id 守卫（issue #161）：闭包捕获本会话 id；用户取消 → 关浮窗 → 开新浮窗
    // 开新一轮时，旧的 in-flight LLM 流仍可能 emit chunk，必须在 emit 前比对当前
    // qa_state.session_id == 捕获 id，否则跳过——避免旧会话的字漏进新气泡。
    let captured_session_id = inner.qa_state.lock().session_id;
    let inner_for_delta = Arc::clone(inner);
    let on_delta = move |chunk: &str| {
        let cur_id = inner_for_delta.qa_state.lock().session_id;
        if cur_id != captured_session_id {
            return; // 旧 session 漏来的 chunk，丢弃
        }
        if let Some(app) = inner_for_delta.app.lock().clone() {
            let _ = app.emit_to(
                "qa",
                "qa:state",
                serde_json::json!({
                    "kind": "answer_delta",
                    "chunk": chunk,
                }),
            );
        }
    };

    // SSE 流取消旗标：cancel_qa_session / close_qa_panel 会 set true，
    // polish 的 SSE loop 每帧检查 → break，释放 HTTP body。详见 issue #161。
    let cancel_flag = Arc::clone(&inner.qa_stream_cancelled);
    let should_cancel = move || cancel_flag.load(Ordering::Relaxed);

    let answer = match answer_chat_dispatch(
        &messages_for_llm,
        &working_languages,
        chinese_script_preference,
        output_language_preference,
        llm_thinking_enabled,
        front_app.as_deref(),
        on_delta,
        should_cancel,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            log::error!("[coord] QA: LLM answer failed: {e}");
            // 把刚 push 的 user 消息回滚，避免 retry 重复
            inner.qa_state.lock().messages.pop();
            finish_qa_with_error(inner, format!("回答失败: {e}"));
            return Err(e.to_string());
        }
    };

    if inner.qa_state.lock().cancelled {
        log::info!("[coord] QA cancel detected before answer — discarding");
        // 同样回滚未配对的 user 消息
        inner.qa_state.lock().messages.pop();
        finish_qa_idle_silently(inner);
        return Ok(());
    }

    inner
        .qa_state
        .lock()
        .messages
        .push(crate::types::QaChatMessage {
            role: "assistant".to_string(),
            content: answer.clone(),
        });

    if let Some(app) = inner.app.lock().clone() {
        let messages = inner.qa_state.lock().messages.clone();
        let _ = app.emit_to(
            "qa",
            "qa:state",
            serde_json::json!({
                "kind": "answer",
                "messages": messages,
            }),
        );
    }

    // 胶囊直接收掉。QA 不走 insertion，没"已粘贴 N 字"语义；浮窗里答案就是用户的反馈。
    // （之前用 Done 状态会被 capsule UI 错误地渲染上一次 dictation 残留的 message/insertedChars。）
    emit_capsule(inner, CapsuleState::Idle, 0.0, 0, None, None);

    // 可选：写一条 history（QA 类型）。当前 DictationSession schema 不能直接表达
    // "QuestionAnswer" 类型，因此简单做法：勾选 qa_save_history 时写一条
    // mode=Raw、error_code=Some("qaSession") 的 placeholder，避免污染 schema 同时
    // 让用户能在历史里翻到这次问答的字面值。详见 issue #118。
    if prefs.qa_save_history {
        let session = DictationSession {
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339(),
            raw_transcript: question.clone(),
            final_text: answer.clone(),
            mode: PolishMode::Raw,
            app_bundle_id: None,
            app_name: front_app.clone(),
            insert_status: InsertStatus::CopiedFallback,
            error_code: Some("qaSession".to_string()),
            duration_ms: Some(raw.duration_ms),
            dictionary_entry_count: None,
            has_audio_recording: None,
            embedded_audio_stats: None,
        };
        let prefs_snapshot = inner.prefs.get();
        if let Err(e) = inner.history.append_with_retention(
            session,
            prefs_snapshot.history_retention_days,
            prefs_snapshot.history_max_entries,
        ) {
            log::error!("[coord] QA history append failed: {e}");
        }
    }

    inner.qa_state.lock().phase = QaPhase::Idle;
    Ok(())
}

/// 把出错状态送到前端浮窗 + 胶囊错误闪一下 + 复位 phase。
/// 浮窗保持可见（v2：错误后用户可以再按 Option 重试）；messages 一并送过去
/// 让前端继续渲染历史对话。
fn finish_qa_with_error(inner: &Arc<Inner>, message: String) {
    stop_qa_recorder(inner);
    if let Some(app) = inner.app.lock().clone() {
        let messages = inner.qa_state.lock().messages.clone();
        let _ = app.emit_to(
            "qa",
            "qa:state",
            serde_json::json!({
                "kind": "error",
                "error": message,
                "messages": messages,
            }),
        );
    }
    emit_capsule(inner, CapsuleState::Error, 0.0, 0, Some(message), None);
    schedule_capsule_idle(inner, 1500, None);
    let mut state = inner.qa_state.lock();
    state.phase = QaPhase::Idle;
    state.cancelled = false;
}

/// 静默收尾：发 idle 事件给前端，phase 复位。**不关浮窗**（v2：浮窗只在用户
/// Esc/X 或再按 QA hotkey 时才关）；多轮对话历史保留。胶囊也即刻收掉。
fn finish_qa_idle_silently(inner: &Arc<Inner>) {
    if let Some(app) = inner.app.lock().clone() {
        let messages = inner.qa_state.lock().messages.clone();
        let _ = app.emit_to(
            "qa",
            "qa:state",
            serde_json::json!({
                "kind": "idle",
                "messages": messages,
            }),
        );
    }
    emit_capsule(inner, CapsuleState::Idle, 0.0, 0, None, None);
    let mut state = inner.qa_state.lock();
    state.phase = QaPhase::Idle;
    state.cancelled = false;
    state.selection = None;
}

fn cancel_qa_session(inner: &Arc<Inner>) {
    let phase = inner.qa_state.lock().phase;
    if phase == QaPhase::Idle {
        return;
    }
    inner.qa_state.lock().cancelled = true;
    // SSE 流取消旗标——polish::chat_completion_history_streaming 的 loop 每帧检查
    // 这个 flag，true 时立即 break 不再 drain HTTP body，避免取消后 LLM 仍烧 token。
    // 详见 issue #161。
    inner.qa_stream_cancelled.store(true, Ordering::SeqCst);
    stop_qa_recorder(inner);
    if let Some(asr) = inner.qa_asr.lock().take() {
        asr.cancel();
    }
    // Processing 阶段保持 phase 让 end_qa_session 自然走完 cancel 检查；
    // 否则直接复位。
    if phase != QaPhase::Processing {
        inner.qa_state.lock().phase = QaPhase::Idle;
    }
    log::info!("[coord] QA session cancelled (was {phase:?})");
}

async fn answer_chat_dispatch<F, C>(
    messages: &[crate::types::QaChatMessage],
    working_languages: &[String],
    chinese_script_preference: ChineseScriptPreference,
    output_language_preference: OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
    on_delta: F,
    should_cancel: C,
) -> anyhow::Result<String>
where
    F: Fn(&str) + Send + Sync,
    C: Fn() -> bool + Send + Sync,
{
    // 见 polish_text 顶部注释——同样的 Gemini / OpenAI-compatible 路由逻辑，
    // QA 流式回答走 Gemini 原生 :streamGenerateContent?alt=sse。
    let active_llm = CredentialsVault::get_active_llm();
    if active_llm == "gemini" {
        let (api_key, model, base_url) = read_gemini_credentials()?;
        let proxy_config = read_llm_proxy_config(&active_llm)?;
        let provider = GeminiProvider::new(
            GeminiConfig::new(api_key, model, base_url)
                .with_thinking_enabled(llm_thinking_enabled)
                .with_proxy_config(proxy_config),
        );
        return Ok(provider
            .answer_chat_streaming(
                messages,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                front_app,
                on_delta,
                should_cancel,
            )
            .await?);
    }

    let provider = build_active_llm_provider(llm_thinking_enabled)?;
    Ok(provider
        .answer_chat_streaming(
            messages,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            front_app,
            on_delta,
            should_cancel,
        )
        .await?)
}

/// 读 Gemini 凭据。所有 LLM provider 共用 ark.* 槽位（persistence 没做 per-provider
/// 隔离），所以这里也是从 `ArkApiKey` / `ArkModelId` / `ArkEndpoint` 三个槽读，
/// 但回退默认值改成谷歌的：base_url 默认 `https://generativelanguage.googleapis.com/v1beta`，
/// 模型默认 `gemini-2.5-flash`。Settings.tsx::onLlmProviderChange 在用户切到 gemini
/// 时会强制把 endpoint/model 覆盖为这两个默认值，所以 99% 情况下槽里读出来就是
/// 这两个；这里的 `unwrap_or_else` 是给极端情况兜底（如旧版本切换 bug 留下的脏数据）。
///
/// base_url 末尾去掉 `/`，让 `llm_gemini::generate_content_url` 拼接稳定。
/// 不去 `/chat/completions` 后缀——OpenAI 兼容路径才会有那个后缀，原生 Gemini 不会。
fn read_gemini_credentials() -> anyhow::Result<(String, String, String)> {
    let api_key = CredentialsVault::get(CredentialAccount::ArkApiKey)?.unwrap_or_default();
    let model = CredentialsVault::get(CredentialAccount::ArkModelId)?
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "gemini-2.5-flash".to_string());
    let base_url = CredentialsVault::get(CredentialAccount::ArkEndpoint)?
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string());
    if api_key.trim().is_empty() {
        anyhow::bail!("API Key 为空");
    }
    let base_url = base_url.trim_end_matches('/').to_string();
    Ok((api_key, model, base_url))
}

fn read_llm_proxy_config(provider_id: &str) -> anyhow::Result<ProviderProxyConfig> {
    read_provider_proxy_config(
        provider_id,
        CredentialAccount::LlmProxyMode,
        CredentialAccount::LlmProxyUrl,
    )
}

fn read_asr_proxy_config(provider_id: &str) -> anyhow::Result<ProviderProxyConfig> {
    read_provider_proxy_config(
        provider_id,
        CredentialAccount::AsrProxyMode,
        CredentialAccount::AsrProxyUrl,
    )
}

fn read_provider_proxy_config(
    provider_id: &str,
    mode_account: CredentialAccount,
    url_account: CredentialAccount,
) -> anyhow::Result<ProviderProxyConfig> {
    let mode = CredentialsVault::get(mode_account)?;
    let proxy_url = CredentialsVault::get(url_account)?;
    ProviderProxyConfig::from_stored(provider_id, mode.as_deref(), proxy_url.as_deref())
        .map_err(anyhow::Error::msg)
}

fn build_active_llm_provider(llm_thinking_enabled: bool) -> anyhow::Result<ActiveLLMProvider> {
    let active = CredentialsVault::get_active_llm();
    let model =
        CredentialsVault::get(CredentialAccount::ArkModelId)?.filter(|s| !s.trim().is_empty());
    if active == CODEX_OAUTH_PROVIDER_ID {
        let config =
            CodexOAuthConfig::new(model.unwrap_or_else(|| CODEX_DEFAULT_MODEL.to_string()))
                .with_thinking_enabled(llm_thinking_enabled)
                .with_proxy_config(read_llm_proxy_config(&active)?);
        return Ok(ActiveLLMProvider::Codex(CodexOAuthLLMProvider::new(config)));
    }

    let api_key = sanitize_llm_api_key(
        &CredentialsVault::get(CredentialAccount::ArkApiKey)?.unwrap_or_default(),
    );
    let model = model.unwrap_or_else(|| "deepseek-v3-2".to_string());
    let endpoint = resolve_ark_endpoint(&active, &api_key)?;
    let base_url = endpoint
        .trim_end_matches("/chat/completions")
        .trim_end_matches('/')
        .to_string();
    // Guardrail: never send a DeepSeek-shaped key to the Volcengine ARK host
    // (or vice versa). That combination yields 401 "API key format is
    // incorrect" and used to block every stop→Done until the auth circuit
    // opened. Prefer provider defaults when the stored endpoint disagrees.
    let base_url = reconcile_llm_base_url_for_key(&active, &api_key, &base_url);
    let proxy_config = read_llm_proxy_config(&active)?;
    let config = OpenAICompatibleConfig::new(active, "Listener Type LLM", base_url, api_key, model)
        .with_thinking_enabled(llm_thinking_enabled)
        .with_proxy_config(proxy_config);
    Ok(ActiveLLMProvider::OpenAI(OpenAICompatibleLLMProvider::new(
        config,
    )))
}

fn sanitize_llm_api_key(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let without_bearer = trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .unwrap_or(trimmed)
        .trim();
    without_bearer.to_string()
}

/// If the stored endpoint is clearly the wrong vendor for this key shape,
/// force the provider default so polish does not 401 on key-format mismatch.
fn reconcile_llm_base_url_for_key(provider_id: &str, api_key: &str, base_url: &str) -> String {
    let key = api_key.trim();
    let looks_deepseek = key.starts_with("sk-");
    let looks_openai = key.starts_with("sk-proj-") || key.starts_with("sk-or-");
    let endpoint_is_ark = base_url.contains("volces.com") || base_url.contains("volcengine");
    let endpoint_is_deepseek = base_url.contains("api.deepseek.com");
    if looks_deepseek && !looks_openai && endpoint_is_ark {
        if let Some(default) = llm_provider_default_endpoint("deepseek") {
            log::warn!(
                "[coord] LLM endpoint {base_url} is ARK but api key looks like DeepSeek; using {default}"
            );
            return default.to_string();
        }
    }
    if provider_id == "ark" && looks_deepseek && endpoint_is_ark {
        if let Some(default) = llm_provider_default_endpoint("deepseek") {
            log::warn!(
                "[coord] active LLM is ark but api key looks like DeepSeek; using {default}"
            );
            return default.to_string();
        }
    }
    if provider_id == "deepseek" && endpoint_is_ark {
        if let Some(default) = llm_provider_default_endpoint("deepseek") {
            log::warn!("[coord] active LLM is deepseek but endpoint is ARK; using {default}");
            return default.to_string();
        }
    }
    if provider_id == "ark" && endpoint_is_deepseek && !looks_deepseek {
        if let Some(default) = llm_provider_default_endpoint("ark") {
            log::warn!("[coord] active LLM is ark but endpoint is DeepSeek; using {default}");
            return default.to_string();
        }
    }
    base_url.to_string()
}

fn resolve_ark_endpoint(provider_id: &str, api_key: &str) -> anyhow::Result<String> {
    let endpoint =
        CredentialsVault::get(CredentialAccount::ArkEndpoint)?.filter(|s| !s.trim().is_empty());
    resolve_ark_endpoint_with_policy(provider_id, api_key, endpoint)
}

fn resolve_ark_endpoint_with_policy(
    provider_id: &str,
    api_key: &str,
    endpoint: Option<String>,
) -> anyhow::Result<String> {
    if api_key.trim().is_empty() {
        match endpoint.as_deref() {
            Some(value) if llm_default_endpoint_requires_api_key(provider_id, value) => {
                anyhow::bail!("API Key 为空");
            }
            None => anyhow::bail!("API Key 为空"),
            _ => {}
        }
    }
    Ok(endpoint
        .unwrap_or_else(|| "https://ark.cn-beijing.volces.com/api/v3/chat/completions".to_string()))
}

fn llm_default_endpoint_requires_api_key(provider_id: &str, endpoint: &str) -> bool {
    llm_provider_default_endpoint(provider_id)
        .map(|default| same_llm_endpoint(endpoint, default))
        .unwrap_or(false)
}

fn llm_provider_default_endpoint(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "ark" => Some("https://ark.cn-beijing.volces.com/api/v3"),
        "deepseek" => Some("https://api.deepseek.com/v1"),
        "siliconflow" => Some("https://api.siliconflow.cn/v1"),
        "openai" => Some("https://api.openai.com/v1"),
        "gemini" => Some("https://generativelanguage.googleapis.com/v1beta"),
        "mimo" => Some("https://api.xiaomimimo.com/v1"),
        "cometapi" => Some("https://api.cometapi.com/v1"),
        "openrouterFree" => Some("https://openrouter.ai/api/v1"),
        "alibabaCoding" => Some("https://coding-intl.dashscope.aliyuncs.com/v1"),
        "codingPlanX" => Some("https://api.codingplanx.ai/v1"),
        _ => None,
    }
}

fn same_llm_endpoint(a: &str, b: &str) -> bool {
    fn normalize(value: &str) -> &str {
        value
            .trim()
            .trim_end_matches('/')
            .trim_end_matches("/chat/completions")
            .trim_end_matches('/')
    }
    normalize(a).eq_ignore_ascii_case(normalize(b))
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
