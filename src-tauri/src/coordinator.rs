//! Dictation coordinator.
//!
//! Mirrors the Swift `DictationCoordinator` state machine. Single owner of
//! session state. Receives hotkey edges, drives recorder + ASR + polish +
//! insertion, persists history, emits `capsule:state` events to the capsule
//! window.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{async_runtime, AppHandle, Emitter, Manager};
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
    new_session_id, publish_abort_idle_after_restore, publishable_dictation_snapshot,
    startup_race_status, BeginOutcome, DictationSnapshot, DictationTransition, DictationUiState,
    SessionId, SessionPhase, SessionState, StartupRaceStatus,
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
    OpenAICompatibleConfig, OpenAICompatibleLLMProvider, ProviderProxyConfig, CODEX_DEFAULT_MODEL,
    CODEX_OAUTH_PROVIDER_ID,
};
use crate::qa_hotkey::{QaHotkeyError, QaHotkeyEvent, QaHotkeyMonitor};
use crate::recorder::{Recorder, RecorderError};
use crate::selection::capture_selection;
#[cfg(target_os = "windows")]
use crate::types::PasteShortcut;
use crate::types::{
    CapsulePayload, CapsuleState, ChineseScriptPreference, DeviceCustomKeyAction,
    DeviceCustomKeyGesture, DeviceCustomKeyId, DeviceCustomKeyMapping, DeviceKnobRotationAction,
    DictationInputSource, DictationSession, HotkeyCapability, HotkeyStatus, HotkeyStatusState,
    InsertStatus, OutputLanguagePreference, PolishMode, ShortcutBinding,
};
#[cfg(target_os = "windows")]
use crate::windows_ime_ipc::ImeSubmitTarget;
#[cfg(target_os = "windows")]
use crate::windows_ime_session::{PreparedWindowsImeSession, WindowsImeSessionController};

mod dictation;
mod qa;
mod resources;

const EMBEDDED_BLE_RETRY_FAST_DELAY: Duration = Duration::from_millis(500);
const EMBEDDED_BLE_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
const EMBEDDED_BLE_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);
const EMBEDDED_BLE_RETRY_LONG_DELAY: Duration = Duration::from_secs(3);
const EMBEDDED_BLE_RETRY_OFFLINE_DELAY: Duration = Duration::from_secs(180);
const EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD: u32 = 6;
const EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN: Duration = Duration::from_secs(600);
const EMBEDDED_BLE_PROBE_RECOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const EMBEDDED_BLE_PROBE_RECOVERY_POLL: Duration = Duration::from_millis(100);
const EMBEDDED_BLE_WAKE_RECOVERY_TIMEOUT: Duration = Duration::from_secs(12);
const EMBEDDED_BLE_RECORDING_CONTROL_READY_TIMEOUT: Duration = Duration::from_secs(5);
const EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const DEVICE_KEY_BLE_PENDING_ACTION_TTL: Duration = Duration::from_secs(15);
const EMBEDDED_BLE_WAKE_GUIDANCE_MESSAGE: &str =
    "Listener BLE 正在重连。若设备处于离线状态，请按 KEY4/唤醒键，再重试；仍失败可导出诊断。";

#[cfg(test)]
use dictation::dictation_error_code;
use dictation::{
    begin_session, cancel_session, end_session, handle_pressed, handle_pressed_edge,
    handle_released_edge, request_embedded_audio_stop_feedback,
    request_embedded_ble_recording_stop_from_host, request_stop_during_starting,
    submit_embedded_audio_ble_once, submit_embedded_audio_ble_stream,
    submit_embedded_audio_ble_stream_background, submit_embedded_audio_file,
    submit_embedded_audio_notifications, submit_embedded_audio_streaming_file,
    submit_embedded_audio_streaming_notifications, HOTKEY_DEBOUNCE,
};
use qa::{close_qa_panel, handle_qa_hotkey_pressed, QaPhase, QaSessionState};
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
    /// 嵌入式 BLE 流式 ASR 的最近一次 partial preview。只用于胶囊视觉反馈；
    /// 光标仍只在 final text 完成后写入。
    embedded_audio_partial_preview: Mutex<Option<String>>,
    /// 嵌入式 BLE 收到停止包后锁存胶囊的停止反馈。SessionPhase 仍保持 Listening，
    /// 让 end_session 接管最终处理，同时避免尾包 / partial preview 把 UI 刷回 Recording。
    embedded_audio_stop_feedback_latched: AtomicBool,
    /// Listener BLE 输入源的后台订阅代次。设置变化时递增，旧监听循环会自然退出。
    embedded_ble_listener_generation: AtomicU64,
    /// Firmware OTA 正在独占 BLE data plane。期间不要自动重启后台音频监听，避免抢占
    /// 同一个 Windows GATT device/session。
    embedded_ble_ota_active: AtomicBool,
    /// 当前 Listener BLE 后台订阅的取消旗标。刷新输入源或退出时主动置位，
    /// 避免旧 WinRT notify 订阅等待 60s 超时后才释放设备。
    embedded_ble_listener_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// 当前 Listener BLE 后台订阅已完成 CCCD notify 写入，可以接收设备音频。
    embedded_ble_listener_ready: AtomicBool,
    /// 最近一次非空闲的 Listener BLE 后台订阅错误。Overview 读取它来区分
    /// 启动阶段的 CCCD/notify/subscription 失败，这类失败不会生成历史会话。
    embedded_ble_listener_last_error: Mutex<Option<String>>,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedBleWakeRecoverySnapshot {
    pub status: EmbeddedBleWakeRecoveryStatus,
    pub user_guidance: String,
    pub recent_disconnect_reason: Option<String>,
    pub reconnect_attempts: u32,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddedBleSessionActorCommand {
    BlePacket,
    AsrPartial,
    AsrFinal,
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

#[derive(Debug, Default)]
struct EmbeddedBleSessionActorState {
    next_seq: u64,
    history: VecDeque<EmbeddedBleSessionActorRecord>,
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
            let history = HistoryStore::new().unwrap_or_else(|e| {
                log::error!("[coord] HistoryStore init failed: {e}; falling back to empty");
                HistoryStore::new().expect("history store init")
            });
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
                    asr: Mutex::new(None),
                    recorder: Mutex::new(None),
                    audio_archive_active: AtomicBool::new(false),
                    embedded_audio_stats: Mutex::new(None),
                    embedded_audio_partial_preview: Mutex::new(None),
                    embedded_audio_stop_feedback_latched: AtomicBool::new(false),
                    embedded_ble_listener_generation: AtomicU64::new(0),
                    embedded_ble_ota_active: AtomicBool::new(false),
                    embedded_ble_listener_cancel: Mutex::new(None),
                    embedded_ble_listener_ready: AtomicBool::new(false),
                    embedded_ble_listener_last_error: Mutex::new(None),
                    embedded_ble_wake_recovery: Mutex::new(
                        EmbeddedBleWakeRecoverySnapshot::default(),
                    ),
                    device_key_pending_ble_action: Mutex::new(None),
                    embedded_ble_session_actor: Mutex::new(EmbeddedBleSessionActorState::default()),
                    embedded_ble_cancel_flag: Mutex::new(None),
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
        let history = HistoryStore::new().unwrap_or_else(|e| {
            log::error!("[coord] HistoryStore init failed: {e}; falling back to empty");
            HistoryStore::new().expect("history store init")
        });
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
                windows_ime: WindowsImeSessionController::new(),
                prepared_windows_ime_session: Arc::new(Mutex::new(Vec::new())),
                state: Mutex::new(SessionState::default()),
                asr: Mutex::new(None),
                recorder: Mutex::new(None),
                audio_archive_active: AtomicBool::new(false),
                embedded_audio_stats: Mutex::new(None),
                embedded_audio_partial_preview: Mutex::new(None),
                embedded_audio_stop_feedback_latched: AtomicBool::new(false),
                embedded_ble_listener_generation: AtomicU64::new(0),
                embedded_ble_ota_active: AtomicBool::new(false),
                embedded_ble_listener_cancel: Mutex::new(None),
                embedded_ble_listener_ready: AtomicBool::new(false),
                embedded_ble_listener_last_error: Mutex::new(None),
                embedded_ble_wake_recovery: Mutex::new(EmbeddedBleWakeRecoverySnapshot::default()),
                device_key_pending_ble_action: Mutex::new(None),
                embedded_ble_session_actor: Mutex::new(EmbeddedBleSessionActorState::default()),
                embedded_ble_cancel_flag: Mutex::new(None),
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
                qa_asr: Mutex::new(None),
                qa_recorder: Mutex::new(None),
                qa_stream_cancelled: Arc::new(AtomicBool::new(false)),
                local_asr_cache: Arc::new(crate::asr::local::LocalAsrCache::new()),
                foundry_local_runtime,
                shutdown: AtomicBool::new(false),
            }),
        }
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
                "[embedded-ble] firmware power probe running for existing embedded BLE source user_overridden={}",
                prefs.dictation_input_source_user_overridden
            );
            async_runtime::spawn_blocking(move || {
                let firmware = crate::embedded_ble::firmware_ota_device_snapshot();
                record_embedded_ble_firmware_power_snapshot(
                    &inner,
                    &firmware,
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
        self.inner.shutdown.store(true, Ordering::SeqCst);
        cancel_embedded_ble_listener_capture(&self.inner, "shutdown");
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
        for gesture in DeviceCustomKeyGesture::ALL {
            for key in DeviceCustomKeyId::ALL {
                if !key.supports_gesture(gesture) {
                    continue;
                }
                let inner = Arc::clone(&self.inner);
                let name = format!(
                    "listener-type-{}-{}-hotkey-supervisor",
                    key.label().to_ascii_lowercase(),
                    gesture.label()
                );
                std::thread::Builder::new()
                    .name(name)
                    .spawn(move || {
                        action_hotkey_supervisor_loop(
                            inner,
                            ActionHotkeyKind::DeviceKey { key, gesture },
                        )
                    })
                    .ok();
            }
        }
    }

    pub fn stop_device_custom_key_hotkey_listeners(&self) {
        for gesture in DeviceCustomKeyGesture::ALL {
            for key in DeviceCustomKeyId::ALL {
                if !key.supports_gesture(gesture) {
                    continue;
                }
                take_action_hotkey_on_main_thread(
                    &self.inner,
                    ActionHotkeyKind::DeviceKey { key, gesture },
                );
            }
        }
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
        for gesture in DeviceCustomKeyGesture::ALL {
            for key in DeviceCustomKeyId::ALL {
                if !key.supports_gesture(gesture) {
                    continue;
                }
                self.update_action_hotkey_binding(ActionHotkeyKind::DeviceKey { key, gesture });
            }
        }
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

    pub async fn start_dictation(&self) -> Result<(), String> {
        if self.inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle {
            if embedded_ble_listener_capture_ready(&self.inner) {
                record_embedded_ble_notify_ready(&self.inner);
                emit_capsule(
                    &self.inner,
                    CapsuleState::Recording,
                    0.0,
                    0,
                    Some("Listener BLE 已连接，按设备语音键开始录音。".to_string()),
                    None,
                );
                schedule_capsule_idle(&self.inner, 1400, None);
                log::info!("[coord] start_dictation reused ready Listener BLE background listener");
                return Ok(());
            }
            self.refresh_embedded_ble_listener();
            record_embedded_ble_reconnect_attempt(&self.inner, "start_dictation");
            emit_capsule(
                &self.inner,
                CapsuleState::Recording,
                0.0,
                0,
                Some("正在重连 Listener BLE；若设备离线，请按 KEY4/唤醒键。".to_string()),
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
                    emit_capsule(
                        &self.inner,
                        CapsuleState::Recording,
                        0.0,
                        0,
                        Some("Listener BLE 已连接，按设备语音键开始录音。".to_string()),
                        None,
                    );
                    schedule_capsule_idle(&self.inner, 1400, None);
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
                        Some(message.clone()),
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
        match embedded_ble_foreground_probe_mode(&self.inner) {
            EmbeddedBleForegroundProbeMode::RefreshBackgroundListener => {
                log::info!("[embedded-ble] foreground BLE path probe refreshing active background listener");
                self.refresh_embedded_ble_listener();
                record_embedded_ble_reconnect_attempt(&self.inner, "foreground_probe_refresh");
                let result = wait_for_embedded_ble_listener_ready(&self.inner, timeout).await;
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
                let result = wait_for_embedded_ble_listener_ready(&self.inner, timeout).await;
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

    pub fn cancel_dictation(&self) {
        cancel_session(&self.inner);
    }

    pub fn pause_embedded_ble_listener_for_ota(&self) {
        pause_embedded_ble_listener_capture(&self.inner, "firmware OTA transfer");
    }

    pub fn begin_firmware_ota_transfer(&self) {
        self.inner
            .embedded_ble_ota_active
            .store(true, Ordering::SeqCst);
        pause_embedded_ble_listener_capture(&self.inner, "firmware OTA transfer");
    }

    pub fn end_firmware_ota_transfer(&self) {
        self.inner
            .embedded_ble_ota_active
            .store(false, Ordering::SeqCst);
    }

    pub fn refresh_embedded_ble_listener(&self) {
        refresh_embedded_ble_listener(&self.inner);
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

fn hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    let capability = HotkeyMonitor::capability();
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let prefs = inner.prefs.get();

        if inner.hotkey.lock().is_some() {
            return;
        }
        *inner.hotkey_status.lock() = HotkeyStatus {
            adapter: capability.adapter,
            state: HotkeyStatusState::Starting,
            message: Some(format!("正在安装全局快捷键监听（第 {} 次）", attempts + 1)),
            last_error: None,
        };
        let (tx, rx) = mpsc::channel::<HotkeyEvent>();
        let trigger = crate::shortcut_binding::legacy_modifier_trigger(&prefs.dictation_hotkey)
            .unwrap_or(crate::types::HotkeyTrigger::Custom);
        let binding = crate::types::HotkeyBinding {
            trigger,
            mode: prefs.hotkey.mode,
            keys: None,
        };
        match HotkeyMonitor::start(binding, tx) {
            Ok(monitor) => {
                let adapter = monitor.kind();
                *inner.hotkey.lock() = Some(monitor);
                if let Some(monitor) = inner.hotkey.lock().as_ref() {
                    let (qa_trigger, translation_trigger) = modifier_shortcut_triggers(&inner);
                    monitor.update_modifier_shortcuts(qa_trigger, translation_trigger);
                }
                *inner.hotkey_status.lock() = HotkeyStatus {
                    adapter,
                    state: HotkeyStatusState::Installed,
                    message: Some(format!("{} 已安装", adapter.display_name())),
                    last_error: None,
                };
                log::info!(
                    "[coord] hotkey listener installed (after {} attempt(s))",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-hotkey-bridge".into())
                    .spawn(move || hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                return;
            }
            Err(e) => {
                attempts += 1;
                let error_message = e.message.clone();
                *inner.hotkey_status.lock() = HotkeyStatus {
                    adapter: capability.adapter,
                    state: HotkeyStatusState::Failed,
                    message: Some(error_message.clone()),
                    last_error: Some(e),
                };
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] hotkey listener attempt #{attempts} failed: {}; retrying in 3s",
                        error_message
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

// ─────────────────────────── QA hotkey supervisor ───────────────────────────

fn qa_hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        // 用户已将 QA 关闭时会先进入待激活状态，prefs 改动通过 update_qa_hotkey_binding 唤醒逻辑恢复。
        let binding = match inner.prefs.get().qa_hotkey.clone() {
            Some(b) => b,
            None => {
                inner.qa_hotkey.lock().take();
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
        };
        if crate::shortcut_binding::legacy_modifier_trigger(&binding).is_some() {
            inner.qa_hotkey.lock().take();
            if let Some(monitor) = inner.hotkey.lock().as_ref() {
                let (qa_trigger, translation_trigger) = modifier_shortcut_triggers(&inner);
                monitor.update_modifier_shortcuts(qa_trigger, translation_trigger);
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if inner.qa_hotkey.lock().is_some() {
            // 已注册成功 → 不重复装；睡 5s 复查（ binding 变化由 update 路径手动触发 ）。
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        // global-hotkey crate 在 macOS 走 Carbon RegisterEventHotKey，要求 manager
        // 在主线程构造，否则 register() 看起来 Ok 但事件根本不会派发——这是 issue #118
        // PR #119 第一版漏掉的关键步骤，导致用户按了 hotkey 完全无反应。这里通过
        // run_on_main_thread 把 QaHotkeyMonitor::start 跳到主线程跑，结果再回 channel。
        let app = inner.app.lock().clone();
        let app = match app {
            Some(a) => a,
            None => {
                // 启动期 AppHandle 还没 bind，再等。
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<QaHotkeyEvent>();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<QaHotkeyMonitor, QaHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = QaHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        // run_on_main_thread 是 fire-and-forget；等主线程跑完结果回来。给 5s 上限避免
        // 主线程繁忙时 supervisor 永久阻塞。
        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] QA hotkey 第 {attempts} 次注册超时（主线程未回执）；3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *inner.qa_hotkey.lock() = Some(monitor);
                log::info!(
                    "[coord] QA hotkey listener installed on main thread (after {} attempt(s))",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-qa-hotkey-bridge".into())
                    .spawn(move || qa_hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!("[coord] QA hotkey 第 {attempts} 次注册失败: {e}; 3s 后重试");
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn qa_hotkey_bridge_loop(inner: Arc<Inner>, rx: mpsc::Receiver<QaHotkeyEvent>) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            continue;
        }
        let inner_cloned = Arc::clone(&inner);
        match evt {
            QaHotkeyEvent::Pressed => {
                async_runtime::spawn(async move { handle_qa_hotkey_pressed(&inner_cloned).await });
            }
        }
    }
}

// ─────────────────────────── combo hotkey supervisor ───────────────────────────

fn combo_hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        // 读当前 prefs
        let prefs = inner.prefs.get();
        if crate::shortcut_binding::legacy_modifier_trigger(&prefs.dictation_hotkey).is_some() {
            // 不是 Custom → 待唤醒状态，等待 prefs 改动触发。
            take_combo_hotkey_on_main_thread(&inner);
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let binding = prefs.dictation_hotkey.clone();
        if is_unconfigured_shortcut(&binding) {
            take_combo_hotkey_on_main_thread(&inner);
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if inner.combo_hotkey.lock().is_some() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let app = inner.app.lock().clone();
        let app = match app {
            Some(a) => a,
            None => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
        let (init_tx, init_rx) =
            mpsc::sync_channel::<Result<ComboHotkeyMonitor, ComboHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = ComboHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] combo hotkey 第 {attempts} 次注册超时（主线程未回执）；3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *inner.combo_hotkey.lock() = Some(monitor);
                log::info!(
                    "[coord] combo hotkey listener installed on main thread (after {} attempt(s))",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-combo-hotkey-bridge".into())
                    .spawn(move || combo_hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!("[coord] combo hotkey 第 {attempts} 次注册失败: {e}; 3s 后重试");
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn combo_hotkey_bridge_loop(inner: Arc<Inner>, rx: mpsc::Receiver<ComboHotkeyEvent>) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            continue;
        }
        let inner_cloned = Arc::clone(&inner);
        match evt {
            ComboHotkeyEvent::Pressed => {
                async_runtime::spawn(async move { handle_pressed_edge(&inner_cloned).await });
            }
            ComboHotkeyEvent::Released => {
                async_runtime::spawn(async move { handle_released_edge(&inner_cloned).await });
            }
        }
    }
}

fn translation_hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let binding = inner.prefs.get().translation_hotkey;
        if is_builtin_translation_shift(&binding)
            || crate::shortcut_binding::legacy_modifier_trigger(&binding).is_some()
        {
            take_translation_hotkey_on_main_thread(&inner);
            if let Some(monitor) = inner.hotkey.lock().as_ref() {
                let (qa_trigger, translation_trigger) = modifier_shortcut_triggers(&inner);
                monitor.update_modifier_shortcuts(qa_trigger, translation_trigger);
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if inner.translation_hotkey.lock().is_some() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let app = match inner.app.lock().clone() {
            Some(a) => a,
            None => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
        let (init_tx, init_rx) =
            mpsc::sync_channel::<Result<ComboHotkeyMonitor, ComboHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = ComboHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *inner.translation_hotkey.lock() = Some(monitor);
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-translation-hotkey-bridge".into())
                    .spawn(move || translation_hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] translation hotkey 第 {attempts} 次注册失败: {e}; 3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn update_translation_hotkey_on_main_thread(
    inner: Arc<Inner>,
    binding: crate::types::ShortcutBinding,
) -> Result<(), ComboHotkeyError> {
    if let Some(monitor) = inner.translation_hotkey.lock().as_ref() {
        return monitor.update_binding(binding);
    }
    let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
    let monitor = ComboHotkeyMonitor::start(binding, tx)?;
    *inner.translation_hotkey.lock() = Some(monitor);
    let bridge_inner = Arc::clone(&inner);
    std::thread::Builder::new()
        .name("listener-type-translation-hotkey-bridge".into())
        .spawn(move || translation_hotkey_bridge_loop(bridge_inner, rx))
        .map_err(|e| ComboHotkeyError::RegisterFailed(format!("spawn bridge thread: {e}")))?;
    Ok(())
}

fn translation_hotkey_bridge_loop(inner: Arc<Inner>, rx: mpsc::Receiver<ComboHotkeyEvent>) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            continue;
        }
        if matches!(evt, ComboHotkeyEvent::Pressed) {
            mark_translation_modifier_seen(&inner);
        }
    }
}

fn action_hotkey_supervisor_loop(inner: Arc<Inner>, kind: ActionHotkeyKind) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let binding = action_hotkey_binding(&inner, kind);
        if is_modifier_only_shortcut(&binding) {
            take_action_hotkey_on_main_thread(&inner, kind);
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if action_hotkey_slot(&inner, kind).lock().is_some() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let app = match inner.app.lock().clone() {
            Some(a) => a,
            None => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
        let (init_tx, init_rx) =
            mpsc::sync_channel::<Result<ComboHotkeyMonitor, ComboHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = ComboHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] action hotkey {kind:?} 第 {attempts} 次注册超时；3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *action_hotkey_slot(&inner, kind).lock() = Some(monitor);
                log::info!(
                    "[coord] action hotkey {kind:?} listener installed after {} attempt(s)",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name(action_hotkey_bridge_thread_name(kind).into())
                    .spawn(move || action_hotkey_bridge_loop(inner_clone, rx, kind))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] action hotkey {kind:?} 第 {attempts} 次注册失败: {e}; 3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn action_hotkey_bridge_loop(
    inner: Arc<Inner>,
    rx: mpsc::Receiver<ComboHotkeyEvent>,
    kind: ActionHotkeyKind,
) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            crate::timeline::mark(
                "backend.hotkey",
                "ignored_shortcut_recording_active",
                format!("kind={kind:?} event={evt:?}"),
            );
            continue;
        }
        crate::timeline::mark(
            "backend.hotkey",
            "event",
            format!("kind={kind:?} event={evt:?}"),
        );
        if matches!(evt, ComboHotkeyEvent::Pressed) {
            handle_action_hotkey_pressed(&inner, kind);
        }
    }
}

fn handle_action_hotkey_pressed(inner: &Arc<Inner>, kind: ActionHotkeyKind) {
    match kind {
        ActionHotkeyKind::SwitchStyle => switch_to_previous_style(inner),
        ActionHotkeyKind::OpenApp => {
            if let Some(app) = inner.app.lock().clone() {
                let app_for_main = app.clone();
                let _ = app.run_on_main_thread(move || {
                    crate::show_main_window(&app_for_main);
                });
            }
        }
        ActionHotkeyKind::DeviceKey { key, gesture } => {
            handle_device_custom_key_pressed(inner, key, gesture)
        }
    }
}

fn handle_device_custom_key_pressed(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) {
    let mapping = device_custom_key_mapping(inner, key, gesture);
    crate::timeline::mark(
        "backend.device_key",
        "pressed",
        format!(
            "key={} gesture={} action={:?}",
            key.label(),
            gesture.label(),
            mapping.action
        ),
    );
    if device_key_action_debounced(inner, key, gesture, &mapping) {
        return;
    }
    log::info!(
        "[device-key] {} {} pressed action={:?}",
        key.label(),
        gesture.label(),
        mapping.action
    );

    match mapping.action {
        DeviceCustomKeyAction::Disabled => {}
        DeviceCustomKeyAction::OpenApp => {
            if let Some(app) = inner.app.lock().clone() {
                let app_for_main = app.clone();
                let app_page = mapping.app_page;
                let _ = app.run_on_main_thread(move || {
                    crate::show_main_window(&app_for_main);
                    let _ = app_for_main.emit("device-key:open-app-page", app_page);
                });
                crate::timeline::mark(
                    "backend.device_key",
                    "open_app_page",
                    format!(
                        "key={} gesture={} page={:?}",
                        key.label(),
                        gesture.label(),
                        app_page
                    ),
                );
            }
        }
        DeviceCustomKeyAction::OpenExternalApp => {
            let path = mapping.external_app_path.trim();
            if path.is_empty() {
                log::warn!("[device-key] {} external app path is empty", key.label());
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some("设备键打开应用失败：路径为空".to_string()),
                    None,
                );
                schedule_capsule_idle(inner, 2200, None);
                return;
            }
            if let Err(error) = open_external_app_path(path) {
                log::warn!(
                    "[device-key] {} failed to open external app {path}: {error}",
                    key.label()
                );
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some(format!("打开应用失败：{error}")),
                    None,
                );
                schedule_capsule_idle(inner, 3000, None);
            } else {
                crate::timeline::mark(
                    "backend.device_key",
                    "open_external_app",
                    format!(
                        "key={} gesture={} path={path}",
                        key.label(),
                        gesture.label()
                    ),
                );
                log::info!(
                    "[device-key] {} opened external app path={path}",
                    key.label()
                );
            }
        }
        DeviceCustomKeyAction::Dictation => {
            let inner = Arc::clone(inner);
            async_runtime::spawn(async move {
                handle_device_dictation_action(inner, key, gesture).await;
            });
        }
        DeviceCustomKeyAction::CopyShortcut => {
            send_builtin_shortcut(inner, key, gesture, "C", "copy");
        }
        DeviceCustomKeyAction::PasteShortcut => {
            send_builtin_shortcut(inner, key, gesture, "V", "paste");
        }
        DeviceCustomKeyAction::UndoShortcut => {
            send_builtin_shortcut(inner, key, gesture, "Z", "undo");
        }
        DeviceCustomKeyAction::SwitchStyle => switch_to_previous_style(inner),
        DeviceCustomKeyAction::SelectionAsk => {
            let inner = Arc::clone(inner);
            async_runtime::spawn(async move { handle_qa_hotkey_pressed(&inner).await });
        }
        DeviceCustomKeyAction::Translation => {
            let inner = Arc::clone(inner);
            async_runtime::spawn(async move { handle_device_translation_action(inner).await });
        }
        DeviceCustomKeyAction::PasteTemplate => {
            let text = mapping.paste_template.trim();
            if text.is_empty() {
                log::warn!("[device-key] {} paste template is empty", key.label());
                return;
            }
            let prefs = inner.prefs.get();
            let status = inner.inserter.insert(
                text,
                prefs.restore_clipboard_after_paste,
                prefs.paste_shortcut,
            );
            log::info!(
                "[device-key] {} pasted template chars={} status={:?}",
                key.label(),
                text.chars().count(),
                status
            );
        }
        DeviceCustomKeyAction::SendShortcut => {
            let Some(shortcut) = mapping.shortcut.as_ref() else {
                log::warn!(
                    "[device-key] {} shortcut action has no binding",
                    key.label()
                );
                return;
            };
            match crate::shortcut_dispatch::send_shortcut(shortcut) {
                Ok(()) => log::info!(
                    "[device-key] {} sent shortcut {}",
                    key.label(),
                    shortcut.display_label()
                ),
                Err(error) => log::warn!(
                    "[device-key] {} failed to send shortcut {}: {error}",
                    key.label(),
                    shortcut.display_label()
                ),
            }
        }
    }
}

fn device_custom_key_mapping(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) -> DeviceCustomKeyMapping {
    let prefs = inner.prefs.get();
    match gesture {
        DeviceCustomKeyGesture::SingleClick => prefs.device_custom_keys.get(key).clone(),
        DeviceCustomKeyGesture::DoubleClick => {
            prefs.device_custom_key_double_clicks.get(key).clone()
        }
        DeviceCustomKeyGesture::LongPress => prefs.device_custom_key_long_presses.get(key).clone(),
    }
}

fn device_key_action_debounced(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    mapping: &DeviceCustomKeyMapping,
) -> bool {
    let window = match mapping.action {
        DeviceCustomKeyAction::Disabled => return false,
        DeviceCustomKeyAction::CopyShortcut
        | DeviceCustomKeyAction::PasteShortcut
        | DeviceCustomKeyAction::UndoShortcut
        | DeviceCustomKeyAction::SendShortcut => Duration::from_millis(160),
        DeviceCustomKeyAction::Dictation => HOTKEY_DEBOUNCE,
        DeviceCustomKeyAction::OpenApp | DeviceCustomKeyAction::OpenExternalApp => {
            Duration::from_millis(900)
        }
        _ => Duration::from_millis(350),
    };
    let now = Instant::now();
    let mut last_dispatch = inner.device_key_last_dispatch_at.lock();
    let key_tuple = (gesture, key);
    if let Some(last) = last_dispatch.get(&key_tuple) {
        if now.duration_since(*last) < window {
            crate::timeline::mark(
                "backend.device_key",
                "debounced",
                format!(
                    "key={} gesture={} action={:?} window_ms={}",
                    key.label(),
                    gesture.label(),
                    mapping.action,
                    window.as_millis()
                ),
            );
            return true;
        }
    }
    last_dispatch.insert(key_tuple, now);
    false
}

fn builtin_shortcut(primary: &str) -> ShortcutBinding {
    ShortcutBinding {
        primary: primary.into(),
        modifiers: vec![if cfg!(target_os = "macos") {
            "cmd".into()
        } else {
            "ctrl".into()
        }],
    }
}

fn send_builtin_shortcut(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    primary: &str,
    label: &str,
) {
    let shortcut = builtin_shortcut(primary);
    crate::timeline::mark(
        "backend.device_key",
        "send_builtin_shortcut",
        format!(
            "key={} gesture={} label={} binding={}",
            key.label(),
            gesture.label(),
            label,
            shortcut.display_label()
        ),
    );
    match crate::shortcut_dispatch::send_shortcut(&shortcut) {
        Ok(()) => log::info!("[device-key] {} sent {label}", key.label()),
        Err(error) => {
            log::warn!(
                "[device-key] {} failed to send {label}: {error}",
                key.label()
            );
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                0,
                Some(format!("{label} 快捷键发送失败：{error}")),
                None,
            );
            schedule_capsule_idle(inner, 2200, None);
        }
    }
}

#[cfg(target_os = "windows")]
fn open_external_app_path(path: &str) -> Result<(), String> {
    let path = validate_external_app_path(path)?;
    shell_execute_open(&path)
}

#[cfg(target_os = "macos")]
fn open_external_app_path(path: &str) -> Result<(), String> {
    let path = validate_external_app_path(path)?;
    std::process::Command::new("/usr/bin/open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn open_external_app_path(path: &str) -> Result<(), String> {
    let path = validate_external_app_path(path)?;
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"))
    {
        return std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(|err| err.to_string());
    }
    std::process::Command::new(&path)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn validate_external_app_path(path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("应用路径为空".into());
    }
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err("应用路径不存在，请从已安装应用中选择或填写完整应用路径".into());
    }
    if !is_supported_external_app_path(&path) {
        return Err("仅支持已安装应用或应用快捷方式路径，不支持命令或脚本".into());
    }
    Ok(path)
}

#[cfg(target_os = "windows")]
fn is_supported_external_app_path(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "exe" | "lnk" | "appref-ms"
                )
            })
}

#[cfg(target_os = "macos")]
fn is_supported_external_app_path(path: &Path) -> bool {
    path.is_dir()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn is_supported_external_app_path(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"))
}

#[cfg(target_os = "windows")]
fn shell_execute_open(path: &Path) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    let operation = wide(OsStr::new("open"));
    let file = wide(path.as_os_str());
    let result = unsafe {
        ShellExecuteW(
            HWND(std::ptr::null_mut()),
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed code={}", result.0 as isize))
    }
}

async fn handle_device_dictation_action(
    inner: Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) {
    let input_source = inner.prefs.get().dictation_input_source;
    let phase = inner.state.lock().phase;
    crate::timeline::mark(
        "backend.device_key",
        "dictation_action",
        format!(
            "key={} gesture={} source={input_source:?} phase={phase:?}",
            key.label(),
            gesture.label()
        ),
    );

    if input_source == DictationInputSource::EmbeddedBle {
        let mut queued_pending_start = false;
        if !embedded_ble_listener_capture_ready(&inner) {
            if matches!(
                device_key_ble_recording_control_decision(&inner),
                DeviceKeyBleRecordingControlDecision::Start
            ) {
                queue_pending_device_key_ble_start(&inner, key, gesture, "listener_not_ready");
                queued_pending_start = true;
            }
            record_embedded_ble_reconnect_attempt(&inner, "device_key_recording_control");
            refresh_embedded_ble_listener(&inner);
            emit_capsule(
                &inner,
                CapsuleState::Reconnecting,
                0.0,
                0,
                Some("设备键已触发，正在恢复 Listener BLE 音频通道...".to_string()),
                None,
            );
        }
        if let Err(error) = wait_for_embedded_ble_listener_ready(
            &inner,
            EMBEDDED_BLE_RECORDING_CONTROL_READY_TIMEOUT,
        )
        .await
        {
            record_embedded_ble_listener_last_error(&inner, &error);
            record_embedded_ble_recovery_failure(&inner, &error);
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_wait_failed",
                format!(
                    "key={} gesture={} pending_start={} error={error}",
                    key.label(),
                    gesture.label(),
                    queued_pending_start
                ),
            );
            if queued_pending_start {
                emit_capsule(
                    &inner,
                    CapsuleState::Reconnecting,
                    0.0,
                    0,
                    Some("设备键已触发，Listener 恢复后会自动开始录音。".to_string()),
                    None,
                );
                return;
            }
            emit_capsule(
                &inner,
                CapsuleState::Error,
                0.0,
                0,
                Some(format!(
                    "设备键已触发，但 Listener BLE 音频通道未恢复：{}",
                    embedded_ble_wake_guidance_for_error(&error)
                )),
                None,
            );
            schedule_capsule_idle(&inner, 6000, None);
            return;
        }

        let control_decision = device_key_ble_recording_control_decision(&inner);
        if queued_pending_start {
            clear_pending_device_key_ble_start(&inner, key, gesture, "inline_ready");
            if !matches!(
                control_decision,
                DeviceKeyBleRecordingControlDecision::Start
            ) {
                crate::timeline::mark(
                    "backend.device_key",
                    "ble_recording_control_pending_dropped_state_changed",
                    format!(
                        "key={} gesture={} decision={control_decision:?}",
                        key.label(),
                        gesture.label()
                    ),
                );
                log::info!(
                    "[device-key] pending BLE recording start dropped after inline recovery because state changed decision={control_decision:?}"
                );
                return;
            }
        }
        if let DeviceKeyBleRecordingControlDecision::IgnoreStarting {
            session_id,
            elapsed_ms,
        } = control_decision
        {
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_ignored_starting",
                format!(
                    "key={} gesture={} session_id={session_id} elapsed_ms={elapsed_ms}",
                    key.label(),
                    gesture.label()
                ),
            );
            log::info!(
                "[device-key] {} {} recording control ignored while embedded BLE session is starting elapsed_ms={elapsed_ms}",
                key.label(),
                gesture.label()
            );
            emit_device_key_recording_control_capsule(
                &inner,
                Some((session_id, SessionPhase::Starting)),
                DictationUiState::Recording,
                CapsuleState::Reconnecting,
                "正在等待 Listener 音频...".to_string(),
            );
            return;
        }

        let control_session = control_decision.control_session();
        let waiting_message = if control_session.is_some() {
            "正在发送设备录音停止控制..."
        } else {
            "正在启动 Listener 录音..."
        };

        let stop_feedback_requested =
            if matches!(control_session, Some((_, SessionPhase::Listening))) {
                request_embedded_audio_stop_feedback(&inner, "device_key_stop_control_pending")
            } else {
                false
            };
        let pending_ui_state = if stop_feedback_requested {
            DictationUiState::Transcribing
        } else {
            DictationUiState::Recording
        };
        emit_device_key_recording_control_capsule(
            &inner,
            control_session,
            pending_ui_state,
            if control_session.is_some() {
                CapsuleState::Reconnecting
            } else {
                CapsuleState::Recording
            },
            waiting_message.to_string(),
        );
        let send_stop_control = matches!(
            control_decision,
            DeviceKeyBleRecordingControlDecision::Stop {
                phase: SessionPhase::Listening,
                ..
            }
        );
        let result = async_runtime::spawn_blocking(move || {
            if send_stop_control {
                crate::embedded_ble::send_recording_control_stop(
                    EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                )
            } else {
                crate::embedded_ble::send_recording_control_toggle(
                    EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                )
            }
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|value| value);
        match result {
            Ok(()) => {
                clear_embedded_ble_listener_last_error(&inner);
                clear_pending_device_key_ble_start(&inner, key, gesture, "control_sent");
                crate::timeline::mark(
                    "backend.device_key",
                    "ble_recording_control_sent",
                    format!("key={} gesture={}", key.label(), gesture.label()),
                );
                if matches!(control_session, Some((_, SessionPhase::Listening))) {
                    return;
                }
                emit_device_key_recording_control_capsule(
                    &inner,
                    control_session,
                    DictationUiState::Recording,
                    CapsuleState::Recording,
                    "Listener 录音已启动，正在接收音频...".to_string(),
                );
            }
            Err(error) => {
                record_embedded_ble_listener_last_error(&inner, &error);
                record_embedded_ble_recovery_failure(&inner, &error);
                refresh_embedded_ble_listener(&inner);
                crate::timeline::mark(
                    "backend.device_key",
                    "ble_recording_control_failed",
                    format!(
                        "key={} gesture={} error={error}",
                        key.label(),
                        gesture.label()
                    ),
                );
                if let Some(kind) =
                    should_keep_device_key_ble_action_pending_after_error(control_decision, &error)
                {
                    match kind {
                        PendingDeviceKeyBleActionKind::Start => {
                            queue_pending_device_key_ble_start(
                                &inner,
                                key,
                                gesture,
                                "control_write_retryable_failure",
                            );
                            emit_capsule(
                                &inner,
                                CapsuleState::Reconnecting,
                                0.0,
                                0,
                                Some(
                                    "设备键已触发，Listener BLE 控制通道恢复后会自动开始录音。"
                                        .to_string(),
                                ),
                                None,
                            );
                        }
                        PendingDeviceKeyBleActionKind::Stop => {
                            queue_pending_device_key_ble_stop(
                                &inner,
                                key,
                                gesture,
                                "control_write_retryable_failure",
                            );
                            emit_device_key_recording_control_capsule(
                                &inner,
                                control_session,
                                DictationUiState::Transcribing,
                                CapsuleState::Reconnecting,
                                "设备停止控制会在 Listener BLE 控制通道恢复后自动补发。"
                                    .to_string(),
                            );
                        }
                    }
                    return;
                }
                let idle_session = emit_device_key_recording_control_capsule(
                    &inner,
                    control_session,
                    DictationUiState::Error,
                    CapsuleState::Error,
                    embedded_ble_recording_control_guidance(&error),
                );
                schedule_capsule_idle(&inner, 6000, idle_session);
            }
        }
        return;
    }

    match phase {
        SessionPhase::Idle => {
            let _ = begin_session(&inner).await;
        }
        SessionPhase::Listening => {
            let _ = end_session(&inner).await;
        }
        SessionPhase::Starting => {
            request_stop_during_starting(&inner, "device key dictation toggle");
        }
        _ => {}
    }
}

fn device_key_ble_recording_control_decision(
    inner: &Arc<Inner>,
) -> DeviceKeyBleRecordingControlDecision {
    let state = inner.state.lock();
    match state.phase {
        SessionPhase::Starting => DeviceKeyBleRecordingControlDecision::IgnoreStarting {
            session_id: state.session_id,
            elapsed_ms: state.started_at.elapsed().as_millis() as u64,
        },
        SessionPhase::Listening => DeviceKeyBleRecordingControlDecision::Stop {
            session_id: state.session_id,
            phase: state.phase,
        },
        _ => DeviceKeyBleRecordingControlDecision::Start,
    }
}

fn pending_device_key_ble_action_age(action: PendingDeviceKeyBleAction, now: Instant) -> Duration {
    now.checked_duration_since(action.queued_at)
        .unwrap_or_default()
}

fn pending_device_key_ble_action_is_fresh(action: PendingDeviceKeyBleAction, now: Instant) -> bool {
    pending_device_key_ble_action_age(action, now) <= DEVICE_KEY_BLE_PENDING_ACTION_TTL
}

fn queue_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    kind: PendingDeviceKeyBleActionKind,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) {
    let action = PendingDeviceKeyBleAction {
        kind,
        key,
        gesture,
        queued_at: Instant::now(),
    };
    let previous = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        slot.replace(action)
    };
    crate::timeline::mark(
        "backend.device_key",
        "ble_recording_control_pending_queued",
        format!(
            "kind={} key={} gesture={} reason={reason} replaced={}",
            kind.label(),
            key.label(),
            gesture.label(),
            previous.is_some()
        ),
    );
    log::info!(
        "[device-key] queued pending BLE recording {} key={} gesture={} reason={reason} replaced={}",
        kind.label(),
        key.label(),
        gesture.label(),
        previous.is_some()
    );
}

fn queue_pending_device_key_ble_start(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) {
    queue_pending_device_key_ble_action(
        inner,
        PendingDeviceKeyBleActionKind::Start,
        key,
        gesture,
        reason,
    );
}

fn queue_pending_device_key_ble_stop(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) {
    queue_pending_device_key_ble_action(
        inner,
        PendingDeviceKeyBleActionKind::Stop,
        key,
        gesture,
        reason,
    );
}

fn clear_pending_device_key_ble_start(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) -> bool {
    let removed = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        if slot.as_ref().is_some_and(|action| {
            action.kind == PendingDeviceKeyBleActionKind::Start
                && action.key == key
                && action.gesture == gesture
        }) {
            slot.take()
        } else {
            None
        }
    };
    if let Some(action) = removed {
        let age_ms = pending_device_key_ble_action_age(action, Instant::now()).as_millis();
        crate::timeline::mark(
            "backend.device_key",
            "ble_recording_control_pending_cleared",
            format!(
                "key={} gesture={} reason={reason} age_ms={age_ms}",
                key.label(),
                gesture.label()
            ),
        );
        log::info!(
            "[device-key] cleared pending BLE recording start key={} gesture={} reason={reason} age_ms={age_ms}",
            key.label(),
            gesture.label()
        );
        true
    } else {
        false
    }
}

fn take_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Option<PendingDeviceKeyBleAction> {
    let action = inner.device_key_pending_ble_action.lock().take()?;
    let now = Instant::now();
    let age = pending_device_key_ble_action_age(action, now);
    let age_ms = age.as_millis();
    if !pending_device_key_ble_action_is_fresh(action, now) {
        crate::timeline::mark(
            "backend.device_key",
            "ble_recording_control_pending_expired",
            format!(
                "kind={} key={} gesture={} reason={reason} age_ms={age_ms}",
                action.kind.label(),
                action.key.label(),
                action.gesture.label()
            ),
        );
        log::info!(
            "[device-key] expired pending BLE recording {} key={} gesture={} reason={reason} age_ms={age_ms}",
            action.kind.label(),
            action.key.label(),
            action.gesture.label()
        );
        return None;
    }
    crate::timeline::mark(
        "backend.device_key",
        "ble_recording_control_pending_taken",
        format!(
            "kind={} key={} gesture={} reason={reason} age_ms={age_ms}",
            action.kind.label(),
            action.key.label(),
            action.gesture.label()
        ),
    );
    Some(action)
}

fn take_pending_device_key_ble_start(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Option<PendingDeviceKeyBleAction> {
    let action = take_pending_device_key_ble_action(inner, reason)?;
    if action.kind == PendingDeviceKeyBleActionKind::Start {
        Some(action)
    } else {
        let mut slot = inner.device_key_pending_ble_action.lock();
        *slot = Some(action);
        None
    }
}

fn drop_pending_device_key_ble_action_for_state(
    action: PendingDeviceKeyBleAction,
    decision: DeviceKeyBleRecordingControlDecision,
    reason: &'static str,
) {
    let age_ms = pending_device_key_ble_action_age(action, Instant::now()).as_millis();
    crate::timeline::mark(
        "backend.device_key",
        "ble_recording_control_pending_dropped_state_changed",
        format!(
            "kind={} key={} gesture={} reason={reason} decision={decision:?} age_ms={age_ms}",
            action.kind.label(),
            action.key.label(),
            action.gesture.label()
        ),
    );
    log::info!(
        "[device-key] pending BLE recording {} dropped because state changed key={} gesture={} reason={reason} decision={decision:?} age_ms={age_ms}",
        action.kind.label(),
        action.key.label(),
        action.gesture.label()
    );
}

fn should_keep_device_key_ble_action_pending_after_error(
    decision: DeviceKeyBleRecordingControlDecision,
    error: &str,
) -> Option<PendingDeviceKeyBleActionKind> {
    if !crate::embedded_ble::classify_ble_failure(error).automatic_recovery {
        return None;
    }
    match decision {
        DeviceKeyBleRecordingControlDecision::Start => Some(PendingDeviceKeyBleActionKind::Start),
        DeviceKeyBleRecordingControlDecision::Stop {
            phase: SessionPhase::Listening,
            ..
        } => Some(PendingDeviceKeyBleActionKind::Stop),
        _ => None,
    }
}

fn should_keep_device_key_ble_start_pending_after_error(
    decision: DeviceKeyBleRecordingControlDecision,
    error: &str,
) -> bool {
    should_keep_device_key_ble_action_pending_after_error(decision, error)
        == Some(PendingDeviceKeyBleActionKind::Start)
}

fn restore_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let previous = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        slot.replace(action)
    };
    log::info!(
        "[device-key] pending BLE recording {} restored reason={reason} replaced={} age_ms={}",
        action.kind.label(),
        previous.is_some(),
        pending_device_key_ble_action_age(action, Instant::now()).as_millis()
    );
}

fn flush_pending_device_key_ble_start_action(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let decision = device_key_ble_recording_control_decision(inner);
    if !matches!(decision, DeviceKeyBleRecordingControlDecision::Start) {
        drop_pending_device_key_ble_action_for_state(action, decision, reason);
        return;
    }
    if !embedded_ble_listener_capture_ready(inner) {
        restore_pending_device_key_ble_action(inner, action, reason);
        log::info!(
            "[device-key] pending BLE recording start restored because notify is no longer ready reason={reason}"
        );
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        handle_device_dictation_action(inner, action.key, action.gesture).await;
    });
}

fn flush_pending_device_key_ble_start(inner: &Arc<Inner>, reason: &'static str) {
    let Some(action) = take_pending_device_key_ble_start(inner, reason) else {
        return;
    };
    flush_pending_device_key_ble_start_action(inner, action, reason);
}

fn flush_pending_device_key_ble_stop_action(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let decision = device_key_ble_recording_control_decision(inner);
    if !matches!(
        decision,
        DeviceKeyBleRecordingControlDecision::Stop {
            phase: SessionPhase::Listening,
            ..
        }
    ) {
        drop_pending_device_key_ble_action_for_state(action, decision, reason);
        return;
    }
    if !embedded_ble_listener_capture_ready(inner) {
        restore_pending_device_key_ble_action(inner, action, reason);
        log::info!(
            "[device-key] pending BLE recording stop restored because notify is no longer ready reason={reason}"
        );
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        send_pending_device_key_ble_stop(inner, action, reason).await;
    });
}

fn flush_pending_device_key_ble_action(inner: &Arc<Inner>, reason: &'static str) {
    let Some(action) = take_pending_device_key_ble_action(inner, reason) else {
        return;
    };
    match action.kind {
        PendingDeviceKeyBleActionKind::Start => {
            flush_pending_device_key_ble_start_action(inner, action, reason);
        }
        PendingDeviceKeyBleActionKind::Stop => {
            flush_pending_device_key_ble_stop_action(inner, action, reason);
        }
    }
}

async fn send_pending_device_key_ble_stop(
    inner: Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let control_decision = device_key_ble_recording_control_decision(&inner);
    let control_session = control_decision.control_session();
    let Some((_, SessionPhase::Listening)) = control_session else {
        drop_pending_device_key_ble_action_for_state(action, control_decision, reason);
        return;
    };

    let _ = request_embedded_audio_stop_feedback(&inner, "device_key_stop_control_retry");
    emit_device_key_recording_control_capsule(
        &inner,
        control_session,
        DictationUiState::Transcribing,
        CapsuleState::Reconnecting,
        "正在补发设备录音停止控制...".to_string(),
    );

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
            clear_embedded_ble_listener_last_error(&inner);
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_pending_stop_sent",
                format!(
                    "key={} gesture={} reason={reason}",
                    action.key.label(),
                    action.gesture.label()
                ),
            );
            log::info!(
                "[device-key] pending BLE recording stop sent key={} gesture={} reason={reason}",
                action.key.label(),
                action.gesture.label()
            );
        }
        Err(error) => {
            record_embedded_ble_listener_last_error(&inner, &error);
            record_embedded_ble_recovery_failure(&inner, &error);
            refresh_embedded_ble_listener(&inner);
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_pending_stop_failed",
                format!(
                    "key={} gesture={} reason={reason} error={error}",
                    action.key.label(),
                    action.gesture.label()
                ),
            );
            if should_keep_device_key_ble_action_pending_after_error(control_decision, &error)
                == Some(PendingDeviceKeyBleActionKind::Stop)
                && pending_device_key_ble_action_is_fresh(action, Instant::now())
            {
                restore_pending_device_key_ble_action(&inner, action, "stop_retryable_failure");
                emit_device_key_recording_control_capsule(
                    &inner,
                    control_session,
                    DictationUiState::Transcribing,
                    CapsuleState::Reconnecting,
                    "设备停止控制会在 Listener BLE 控制通道恢复后自动补发。".to_string(),
                );
                return;
            }
            let idle_session = emit_device_key_recording_control_capsule(
                &inner,
                control_session,
                DictationUiState::Error,
                CapsuleState::Error,
                embedded_ble_recording_control_guidance(&error),
            );
            schedule_capsule_idle(&inner, 6000, idle_session);
        }
    }
}

fn current_device_key_recording_control_session(
    inner: &Arc<Inner>,
) -> Option<(SessionId, SessionPhase)> {
    device_key_ble_recording_control_decision(inner).control_session()
}

fn emit_device_key_recording_control_capsule(
    inner: &Arc<Inner>,
    session: Option<(SessionId, SessionPhase)>,
    ui_state: DictationUiState,
    fallback_state: CapsuleState,
    message: String,
) -> Option<SessionId> {
    if let Some((session_id, _)) = session {
        if publish_dictation_capsule(
            inner,
            session_id,
            ui_state,
            0.0,
            Some(message.clone()),
            None,
        ) {
            return Some(session_id);
        }
        emit_capsule_for_session(
            inner,
            session_id,
            fallback_state,
            0.0,
            0,
            Some(message),
            None,
        );
        return Some(session_id);
    }

    emit_capsule(inner, fallback_state, 0.0, 0, Some(message), None);
    None
}

async fn handle_device_translation_action(inner: Arc<Inner>) {
    let phase = inner.state.lock().phase;
    if matches!(phase, SessionPhase::Idle) {
        let _ = begin_session(&inner).await;
        mark_translation_modifier_seen(&inner);
        return;
    }
    mark_translation_modifier_seen(&inner);
    handle_pressed(&inner).await;
}

fn switch_to_previous_style(inner: &Arc<Inner>) {
    let mut prefs = inner.prefs.get();
    let packs = match inner.style_packs.list() {
        Ok(packs) => packs,
        Err(error) => {
            log::warn!("[coord] switch style hotkey failed to load style packs: {error}");
            return;
        }
    };
    let enabled: Vec<crate::types::StylePack> =
        packs.into_iter().filter(|pack| pack.enabled).collect();
    if enabled.len() <= 1 {
        log::info!("[coord] switch style hotkey ignored: enabled style count <= 1");
        return;
    }
    let current_index = enabled
        .iter()
        .position(|pack| pack.id == prefs.active_style_pack_id)
        .unwrap_or(0);
    let next_index = if current_index == 0 {
        enabled.len() - 1
    } else {
        current_index - 1
    };
    prefs.active_style_pack_id = enabled[next_index].id.clone();
    sync_style_pack_preferences(&mut prefs, &enabled);
    if let Err(e) = inner.prefs.set(prefs.clone()) {
        log::warn!("[coord] switch style hotkey 保存失败: {e}");
    } else {
        log::info!(
            "[coord] switch style hotkey changed active style pack to {}",
            prefs.active_style_pack_id
        );
        if let Some(app) = inner.app.lock().clone() {
            let _ = app.emit("prefs:changed", &prefs);
            let _ = app.emit_to("main", "prefs:changed", &prefs);
            let app_for_main = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(err) = crate::refresh_tray_microphone_menu(&app_for_main) {
                    log::warn!("[tray] refresh style menu after switch style hotkey failed: {err}");
                }
            });
        }
    }
}

fn take_combo_hotkey_on_main_thread(inner: &Arc<Inner>) {
    let app = inner.app.lock().clone();
    if let Some(app) = app {
        let inner = Arc::clone(inner);
        let _ = app.run_on_main_thread(move || {
            inner.combo_hotkey.lock().take();
        });
    } else {
        inner.combo_hotkey.lock().take();
    }
}

fn take_translation_hotkey_on_main_thread(inner: &Arc<Inner>) {
    let app = inner.app.lock().clone();
    if let Some(app) = app {
        let inner = Arc::clone(inner);
        let _ = app.run_on_main_thread(move || {
            inner.translation_hotkey.lock().take();
        });
    } else {
        inner.translation_hotkey.lock().take();
    }
}

fn take_action_hotkey_on_main_thread(inner: &Arc<Inner>, kind: ActionHotkeyKind) {
    let app = inner.app.lock().clone();
    if let Some(app) = app {
        let inner = Arc::clone(inner);
        let _ = app.run_on_main_thread(move || {
            action_hotkey_slot(&inner, kind).lock().take();
        });
    } else {
        action_hotkey_slot(inner, kind).lock().take();
    }
}

fn action_hotkey_slot(
    inner: &Arc<Inner>,
    kind: ActionHotkeyKind,
) -> &Mutex<Option<ComboHotkeyMonitor>> {
    match kind {
        ActionHotkeyKind::SwitchStyle => &inner.switch_style_hotkey,
        ActionHotkeyKind::OpenApp => &inner.open_app_hotkey,
        ActionHotkeyKind::DeviceKey { key, gesture } => {
            &inner.device_key_hotkeys[device_key_hotkey_index(key, gesture)]
        }
    }
}

fn device_key_hotkey_index(key: DeviceCustomKeyId, gesture: DeviceCustomKeyGesture) -> usize {
    if key == DeviceCustomKeyId::Knob {
        return 12;
    }
    let gesture_offset = match gesture {
        DeviceCustomKeyGesture::SingleClick => 0,
        DeviceCustomKeyGesture::DoubleClick => 4,
        DeviceCustomKeyGesture::LongPress => 8,
    };
    let key_offset = match key {
        DeviceCustomKeyId::Key1 => 0,
        DeviceCustomKeyId::Key2 => 1,
        DeviceCustomKeyId::Key3 => 2,
        DeviceCustomKeyId::Key4 => 3,
        DeviceCustomKeyId::Knob => 0,
    };
    gesture_offset + key_offset
}

fn action_hotkey_binding(
    inner: &Arc<Inner>,
    kind: ActionHotkeyKind,
) -> crate::types::ShortcutBinding {
    let prefs = inner.prefs.get();
    match kind {
        ActionHotkeyKind::SwitchStyle => prefs.switch_style_hotkey,
        ActionHotkeyKind::OpenApp => prefs.open_app_hotkey,
        ActionHotkeyKind::DeviceKey { key, gesture } => crate::types::ShortcutBinding {
            primary: key.fallback_primary_for(gesture).into(),
            modifiers: if key == DeviceCustomKeyId::Knob {
                vec!["shift".into()]
            } else {
                Vec::new()
            },
        },
    }
}

fn is_modifier_only_shortcut(binding: &crate::types::ShortcutBinding) -> bool {
    binding.modifiers.is_empty()
        && (binding.primary.eq_ignore_ascii_case("shift")
            || crate::shortcut_binding::legacy_modifier_trigger(binding).is_some())
}

fn is_unconfigured_shortcut(binding: &crate::types::ShortcutBinding) -> bool {
    binding.primary.trim().is_empty()
}

fn action_hotkey_bridge_thread_name(kind: ActionHotkeyKind) -> &'static str {
    match kind {
        ActionHotkeyKind::SwitchStyle => "listener-type-switch-style-hotkey-bridge",
        ActionHotkeyKind::OpenApp => "listener-type-open-app-hotkey-bridge",
        ActionHotkeyKind::DeviceKey { .. } => "listener-type-device-key-hotkey-bridge",
    }
}

fn is_builtin_translation_shift(binding: &crate::types::ShortcutBinding) -> bool {
    binding.modifiers.is_empty() && binding.primary.eq_ignore_ascii_case("shift")
}

fn modifier_shortcut_triggers(
    inner: &Arc<Inner>,
) -> (
    Option<crate::types::HotkeyTrigger>,
    Option<crate::types::HotkeyTrigger>,
) {
    let prefs = inner.prefs.get();
    let qa_trigger = prefs
        .qa_hotkey
        .as_ref()
        .and_then(crate::shortcut_binding::legacy_modifier_trigger);
    let translation_trigger = if is_builtin_translation_shift(&prefs.translation_hotkey) {
        None
    } else {
        crate::shortcut_binding::legacy_modifier_trigger(&prefs.translation_hotkey)
    };
    (qa_trigger, translation_trigger)
}

fn embedded_ble_listener_last_error(inner: &Arc<Inner>) -> Option<String> {
    inner.embedded_ble_listener_last_error.lock().clone()
}

fn record_embedded_ble_listener_last_error(inner: &Arc<Inner>, err: &str) {
    *inner.embedded_ble_listener_last_error.lock() = Some(err.to_string());
}

fn clear_embedded_ble_listener_last_error(inner: &Arc<Inner>) {
    *inner.embedded_ble_listener_last_error.lock() = None;
}

fn record_embedded_ble_session_actor_command(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
) -> u64 {
    dispatch_embedded_ble_session_actor_command(inner, command, session_id, detail, |seq| seq)
}

fn dispatch_embedded_ble_session_actor_command<T>(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
    handle: impl FnOnce(u64) -> T,
) -> T {
    let detail = detail.into();
    let (seq, result) = {
        let mut actor = inner.embedded_ble_session_actor.lock();
        actor.next_seq = actor.next_seq.saturating_add(1);
        let seq = actor.next_seq;
        actor.history.push_back(EmbeddedBleSessionActorRecord {
            seq,
            command,
            session_id,
            detail: detail.clone(),
        });
        while actor.history.len() > EMBEDDED_BLE_SESSION_ACTOR_HISTORY_LIMIT {
            actor.history.pop_front();
        }
        let result = handle(seq);
        (seq, result)
    };
    crate::timeline::mark(
        "backend.embedded_ble_session_actor",
        command.as_str(),
        format!("seq={seq} session_id={session_id:?} {detail}"),
    );
    result
}

#[cfg(test)]
fn embedded_ble_session_actor_history(inner: &Arc<Inner>) -> Vec<EmbeddedBleSessionActorRecord> {
    inner
        .embedded_ble_session_actor
        .lock()
        .history
        .iter()
        .cloned()
        .collect()
}

fn embedded_ble_session_actor_diagnostics(
    inner: &Arc<Inner>,
) -> Vec<EmbeddedBleSessionActorDiagnosticRecord> {
    inner
        .embedded_ble_session_actor
        .lock()
        .history
        .iter()
        .map(EmbeddedBleSessionActorRecord::diagnostic)
        .collect()
}

fn refresh_embedded_ble_listener(inner: &Arc<Inner>) {
    if inner.embedded_ble_ota_active.load(Ordering::SeqCst) {
        log::info!("[embedded-ble] background listener refresh skipped during firmware OTA");
        return;
    }
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    cancel_embedded_ble_listener_capture(inner, "refresh");
    if std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
        .ok()
        .as_deref()
        == Some("1")
    {
        log::info!(
            "[embedded-ble] background listener disabled by LISTENER_TYPE_DISABLE_BACKGROUND_BLE"
        );
        clear_embedded_ble_listener_last_error(inner);
        return;
    }
    let source = inner.prefs.get().dictation_input_source;
    if source != DictationInputSource::EmbeddedBle {
        log::info!("[embedded-ble] background listener disabled (source={source:?})");
        clear_embedded_ble_listener_last_error(inner);
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        embedded_ble_background_listener_loop(inner, generation).await;
    });
}

fn embedded_ble_wake_recovery_snapshot(inner: &Arc<Inner>) -> EmbeddedBleWakeRecoverySnapshot {
    inner.embedded_ble_wake_recovery.lock().clone()
}

fn embedded_ble_background_listener_disabled_by_env() -> bool {
    std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
        .ok()
        .is_some_and(|value| value == "1")
}

fn should_auto_select_embedded_ble_input_source(
    prefs: &crate::types::UserPreferences,
    firmware: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
) -> bool {
    !prefs.dictation_input_source_user_overridden
        && prefs.dictation_input_source != DictationInputSource::EmbeddedBle
        && firmware.connected
}

fn auto_select_embedded_ble_input_source_from_snapshot(
    inner: &Arc<Inner>,
    firmware: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
) -> bool {
    let mut prefs = inner.prefs.get();
    if !should_auto_select_embedded_ble_input_source(&prefs, firmware) {
        log::info!(
            "[embedded-ble] auto input source selection skipped source={:?} connected={} detail={:?}",
            prefs.dictation_input_source,
            firmware.connected,
            firmware.detail.as_deref()
        );
        return false;
    }

    prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    if let Err(err) = inner.prefs.set(prefs.clone()) {
        log::warn!("[embedded-ble] auto input source selection persist failed: {err}");
        return false;
    }

    log::info!(
        "[embedded-ble] auto selected Listener BLE input source hardware={:?} firmware={:?}",
        firmware.hardware_revision,
        firmware.firmware_version
    );
    if let Some(app) = inner.app.lock().clone() {
        let _ = app.emit("prefs:changed", &prefs);
        let _ = app.emit_to("main", "prefs:changed", &prefs);
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(err) = crate::refresh_tray_microphone_menu(&app_for_main) {
                log::warn!(
                    "[tray] refresh after embedded BLE auto input source selection failed: {err}"
                );
            }
        });
    }
    refresh_embedded_ble_listener(inner);
    sync_device_knob_rotation_action_to_firmware(inner, "auto_embedded_ble_input_source");
    true
}

fn record_embedded_ble_firmware_power_snapshot(
    inner: &Arc<Inner>,
    firmware: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    reason: &'static str,
) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    if firmware.usb_powered.is_some() {
        snapshot.usb_powered = firmware.usb_powered;
    }
    if firmware.battery_percent.is_some() {
        snapshot.battery_percent = firmware.battery_percent;
    }
    log::info!(
        "[embedded-ble] cached firmware power state reason={reason} usb_powered={:?} battery_percent={:?} detail={:?}",
        snapshot.usb_powered,
        snapshot.battery_percent,
        firmware.detail.as_deref(),
    );
}

fn record_embedded_ble_reconnect_attempt(inner: &Arc<Inner>, reason: &str) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    snapshot.status = EmbeddedBleWakeRecoveryStatus::Reconnecting;
    snapshot.user_guidance =
        "正在重连 Listener BLE 并恢复音频 notify；如果设备离线，请按 KEY4/唤醒键。".to_string();
    snapshot.reconnect_attempts = snapshot.reconnect_attempts.saturating_add(1);
    snapshot.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Opening;
    snapshot.last_attempt_at = Some(now_rfc3339());
    log::info!(
        "[embedded-ble] wake recovery attempt #{} reason={reason}",
        snapshot.reconnect_attempts
    );
}

fn record_embedded_ble_notify_ready(inner: &Arc<Inner>) -> bool {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    let recent_disconnect_reason = snapshot.recent_disconnect_reason.clone();
    let notify_was_recovering = matches!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Lost | EmbeddedBleNotifySubscriptionState::Failed
    );
    let previous_status = snapshot.status.clone();
    let previous_notify_state = snapshot.notify_subscription_state.clone();
    let usb_powered = snapshot.usb_powered;
    let battery_percent = snapshot.battery_percent;
    let reconnect_attempts = snapshot.reconnect_attempts;
    let recent_disconnect_failure = recent_disconnect_reason
        .as_deref()
        .map(crate::embedded_ble::classify_ble_failure);
    let recent_disconnect_low_power_idle = recent_disconnect_reason
        .as_deref()
        .is_some_and(is_embedded_ble_low_power_idle_candidate);
    let recovered = recent_disconnect_reason.is_some() || notify_was_recovering;
    let emit_recovered_capsule = recovered
        && recent_disconnect_reason
            .as_deref()
            .map(|reason| {
                should_emit_embedded_ble_recovered_capsule_for_reason(
                    reason,
                    usb_powered,
                    reconnect_attempts,
                )
            })
            .unwrap_or(true);
    log::info!(
        "[embedded-ble] notify ready recovery decision recovered={} emit_recovered_capsule={} previous_status={:?} previous_notify_state={:?} notify_was_recovering={} reconnect_attempts={} usb_powered={:?} battery_percent={:?} recent_disconnect_kind={:?} recent_disconnect_automatic_recovery={} recent_disconnect_low_power_idle={} recent_disconnect_reason={}",
        recovered,
        emit_recovered_capsule,
        previous_status,
        previous_notify_state,
        notify_was_recovering,
        reconnect_attempts,
        usb_powered,
        battery_percent,
        recent_disconnect_failure.as_ref().map(|failure| failure.kind),
        recent_disconnect_failure
            .as_ref()
            .is_some_and(|failure| failure.automatic_recovery),
        recent_disconnect_low_power_idle,
        recent_disconnect_reason
            .as_deref()
            .map(embedded_ble_log_preview)
            .unwrap_or_else(|| "-".to_string()),
    );
    snapshot.status = EmbeddedBleWakeRecoveryStatus::Ready;
    snapshot.user_guidance = "Listener BLE 已连接，音频 notify 已订阅。".to_string();
    snapshot.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Subscribed;
    snapshot.last_ready_at = Some(now_rfc3339());
    if recovered {
        snapshot.recent_disconnect_reason = None;
    }
    emit_recovered_capsule
}

fn firmware_mode_for_device_knob_rotation_action(action: DeviceKnobRotationAction) -> &'static str {
    match action {
        DeviceKnobRotationAction::SystemVolume => "system_volume",
        DeviceKnobRotationAction::ScreenBrightness => "screen_brightness",
        DeviceKnobRotationAction::Disabled => "disabled",
    }
}

fn legacy_ec11_mode_for_device_knob_rotation_action(
    action: DeviceKnobRotationAction,
) -> &'static str {
    match action {
        DeviceKnobRotationAction::SystemVolume => "VOLUME",
        DeviceKnobRotationAction::ScreenBrightness => "BRIGHTNESS",
        DeviceKnobRotationAction::Disabled => "DISABLED",
    }
}

fn sync_device_knob_rotation_action_to_firmware(inner: &Arc<Inner>, reason: &'static str) {
    let action = inner.prefs.get().device_knob_rotation_action;
    let mode = firmware_mode_for_device_knob_rotation_action(action);
    let legacy_mode = legacy_ec11_mode_for_device_knob_rotation_action(action);
    async_runtime::spawn_blocking(move || {
        let command = format!("DEVICE:SET knob_rotation={mode}");
        match crate::embedded_ble::send_device_settings_command(&command, Duration::from_secs(2)) {
            Ok(()) => {
                log::info!("[device-knob] synced knob_rotation setting mode={mode} reason={reason}")
            }
            Err(err) => {
                log::warn!(
                    "[device-knob] knob_rotation setting sync failed reason={reason}: {err}; trying legacy EC11 control"
                );
                match crate::embedded_ble::send_ec11_rotation_mode(legacy_mode, Duration::from_secs(2)) {
                    Ok(()) => log::info!(
                        "[device-knob] synced legacy EC11 rotation mode={legacy_mode} reason={reason}"
                    ),
                    Err(legacy_err) => log::warn!(
                        "[device-knob] EC11 rotation mode sync skipped reason={reason}: {legacy_err}"
                    ),
                }
            }
        }
    });
}

fn record_embedded_ble_listener_cancelled(inner: &Arc<Inner>, reason: &str) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    snapshot.status = EmbeddedBleWakeRecoveryStatus::Idle;
    snapshot.user_guidance = "Listener BLE 后台监听已暂停。".to_string();
    snapshot.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
    snapshot.recent_disconnect_reason = Some(reason.to_string());
}

fn record_embedded_ble_recovery_failure(inner: &Arc<Inner>, err: &str) {
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    let failure = crate::embedded_ble::classify_ble_failure(err);
    snapshot.status = match failure.kind {
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
        | crate::embedded_ble::BleFailureKind::PairedButDisconnected => {
            EmbeddedBleWakeRecoveryStatus::Reconnecting
        }
        crate::embedded_ble::BleFailureKind::DeviceAsleep
        | crate::embedded_ble::BleFailureKind::DeviceMissing => {
            EmbeddedBleWakeRecoveryStatus::NeedsWakeKey
        }
        _ if is_embedded_ble_wake_or_sleep_error(err) => {
            EmbeddedBleWakeRecoveryStatus::NeedsWakeKey
        }
        _ => EmbeddedBleWakeRecoveryStatus::Failed,
    };
    snapshot.user_guidance =
        embedded_ble_wake_guidance_for_error_with_power(err, snapshot.usb_powered);
    snapshot.recent_disconnect_reason = Some(err.to_string());
    snapshot.notify_subscription_state = if is_embedded_ble_cancelled_error(err) {
        EmbeddedBleNotifySubscriptionState::Cancelled
    } else if err.to_ascii_lowercase().contains("notify")
        || err.to_ascii_lowercase().contains("cccd")
        || err.to_ascii_lowercase().contains("subscription")
    {
        EmbeddedBleNotifySubscriptionState::Failed
    } else {
        EmbeddedBleNotifySubscriptionState::Lost
    };
    log::warn!(
        "[embedded-ble] recovery failure recorded kind={:?} automatic_recovery={} retryable={} status={:?} notify_state={:?} usb_powered={:?} battery_percent={:?} guidance={} err={}",
        failure.kind,
        failure.automatic_recovery,
        failure.retryable,
        snapshot.status,
        snapshot.notify_subscription_state,
        snapshot.usb_powered,
        snapshot.battery_percent,
        snapshot.user_guidance,
        embedded_ble_log_preview(err),
    );
}

fn emit_embedded_ble_recovery_capsule(
    inner: &Arc<Inner>,
    state_label: &str,
    message: impl Into<String>,
    idle_after_ms: Option<u64>,
) {
    let state = if state_label == "reconnected" {
        CapsuleState::Done
    } else {
        CapsuleState::Reconnecting
    };
    emit_capsule(inner, state, 0.0, 0, Some(message.into()), None);
    log::info!("[embedded-ble] recovery capsule state={state_label} emitted=true");
    if let Some(delay_ms) = idle_after_ms {
        schedule_capsule_idle(inner, delay_ms, None);
    }
}

fn embedded_ble_wake_guidance_for_error(err: &str) -> String {
    embedded_ble_wake_guidance_for_error_with_power(err, None)
}

fn embedded_ble_wake_guidance_for_error_with_power(err: &str, usb_powered: Option<bool>) -> String {
    if is_embedded_ble_cancelled_error(err) {
        return "Listener BLE 连接已暂停；请稍后重试。".to_string();
    }
    let failure = crate::embedded_ble::classify_ble_failure(err);
    match failure.kind {
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
            if embedded_ble_usb_power_allows_low_power_idle(usb_powered) =>
        {
            return "Listener BLE 因离线状态断开，正在重连音频 notify；若设备离线，请按 KEY4/唤醒键。".to_string();
        }
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect => {
            return "Listener BLE 正在重连音频 notify；当前未确认处于离线状态场景，若持续失败请重新连接或导出诊断。".to_string();
        }
        crate::embedded_ble::BleFailureKind::PairedButDisconnected => {
            return "Listener BLE 连接临时中断，Type 正在自动重连音频 notify；请保持设备唤醒。"
                .to_string();
        }
        crate::embedded_ble::BleFailureKind::MissingPairing
        | crate::embedded_ble::BleFailureKind::StaleGattService => {
            return "Listener BLE 配对或 GATT 缓存需要恢复。请在 Windows 蓝牙中重新连接或重新配对后重试。".to_string();
        }
        crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded
        | crate::embedded_ble::BleFailureKind::AccessDenied => {
            return "Windows 蓝牙暂时不可用。请打开 Windows 蓝牙设置，确认 Listener 已连接后重试。"
                .to_string();
        }
        crate::embedded_ble::BleFailureKind::DeviceAsleep
        | crate::embedded_ble::BleFailureKind::DeviceMissing => {
            return EMBEDDED_BLE_WAKE_GUIDANCE_MESSAGE.to_string();
        }
        _ => {}
    }
    if is_embedded_ble_wake_or_sleep_error(err) {
        return EMBEDDED_BLE_WAKE_GUIDANCE_MESSAGE.to_string();
    }
    "Listener BLE 暂时不可用。请重新连接设备，或按 KEY4/唤醒键后重试；仍失败可导出诊断。"
        .to_string()
}

fn embedded_ble_recording_control_guidance(err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    if lower.contains("audio control")
        || lower.contains("characteristic")
        || lower.contains("no characteristics")
        || lower.contains("element not found")
        || lower.contains("not found")
    {
        return format!(
            "设备键已触发，但当前固件或 Windows GATT 缓存没有 BLE 录音控制特征：{err}。请刷支持录音控制的新固件，或在 Windows 蓝牙中删除 Listener 后重新配对。"
        );
    }

    format!(
        "设备键录音控制失败：{err}。{}",
        embedded_ble_wake_guidance_for_error(err)
    )
}

fn is_embedded_ble_wake_or_sleep_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("not found")
        || lower.contains("no subscribable")
        || lower.contains("not recover")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("unreachable")
        || lower.contains("device is unreachable")
        || lower.contains("disconnected")
        || lower.contains("asleep")
        || lower.contains("deep sleep")
        || lower.contains("wake key")
        || lower.contains("key4")
        || lower.contains("transport_not_ready")
        || lower.contains("transport not ready")
        || lower.contains("reason=546")
        || lower.contains("reason: 546")
        || lower.contains("reason 546")
        || lower.contains("low-power idle")
        || lower.contains("low power idle")
        || lower.contains("idle disconnect")
}

fn is_embedded_ble_cancelled_error(err: &str) -> bool {
    err.contains("后台监听已取消") || err.to_ascii_lowercase().contains("cancel")
}

fn now_rfc3339() -> String {
    DateTime::<Utc>::from(std::time::SystemTime::now()).to_rfc3339()
}

fn mark_translation_modifier_seen(inner: &Arc<Inner>) {
    let phase = inner.state.lock().phase;
    if matches!(phase, SessionPhase::Starting | SessionPhase::Listening) {
        inner
            .translation_modifier_seen
            .store(true, Ordering::SeqCst);
        log::info!("[coord] translation modifier seen during {phase:?}");
    }
}

async fn embedded_ble_background_listener_loop(inner: Arc<Inner>, generation: u64) {
    log::info!("[embedded-ble] background listener started generation={generation}");
    let mut retry_delay = EMBEDDED_BLE_RETRY_BASE_DELAY;
    let mut last_stale_cleanup_at: Option<Instant> = None;
    loop {
        if inner.shutdown.load(Ordering::SeqCst)
            || inner
                .embedded_ble_listener_generation
                .load(Ordering::SeqCst)
                != generation
            || inner.embedded_ble_ota_active.load(Ordering::SeqCst)
        {
            break;
        }
        if inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle {
            break;
        }

        let cancel_capture = install_embedded_ble_listener_cancel(&inner, generation);
        record_embedded_ble_reconnect_attempt(&inner, "background_listener_loop");
        match submit_embedded_audio_ble_stream_background(&inner, Arc::clone(&cancel_capture)).await
        {
            Ok(result) => {
                log::info!(
                    "[embedded-ble] background session completed pcm_bytes={} missing_packets={}",
                    result.reconstructed_pcm_bytes,
                    result.stats.missing_packet_count
                );
                clear_embedded_ble_listener_last_error(&inner);
                retry_delay = EMBEDDED_BLE_RETRY_BASE_DELAY;
            }
            Err(err) => {
                clear_embedded_ble_listener_cancel(&inner, &cancel_capture);
                if inner.shutdown.load(Ordering::SeqCst)
                    || inner
                        .embedded_ble_listener_generation
                        .load(Ordering::SeqCst)
                        != generation
                    || inner.embedded_ble_ota_active.load(Ordering::SeqCst)
                    || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
                {
                    break;
                }
                if is_embedded_ble_idle_timeout_error(&err) {
                    clear_embedded_ble_listener_last_error(&inner);
                    retry_delay = next_embedded_ble_background_retry_delay(&err, retry_delay);
                } else {
                    record_embedded_ble_listener_last_error(&inner, &err);
                    record_embedded_ble_recovery_failure(&inner, &err);
                    if should_emit_embedded_ble_background_recovery_capsule(&inner, &err) {
                        emit_embedded_ble_recovery_capsule(
                            &inner,
                            "reconnecting",
                            "Listener BLE 正在自动重连音频通道...",
                            Some(1800),
                        );
                    } else if is_embedded_ble_automatic_recovery_error(&err) {
                        log::info!(
                            "[embedded-ble] background recovery capsule suppressed; see recovery decision log"
                        );
                    }
                    let stale_cleanup_attempted =
                        maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
                            &inner,
                            &err,
                            &mut last_stale_cleanup_at,
                        )
                        .await;
                    let stale_cleanup_backoff = stale_cleanup_attempted
                        || should_throttle_embedded_ble_background_stale_pairing_cleanup(
                            &err,
                            &embedded_ble_wake_recovery_snapshot(&inner),
                            last_stale_cleanup_at,
                            Instant::now(),
                        );
                    if stale_cleanup_backoff && !stale_cleanup_attempted {
                        log::warn!(
                            "[embedded-ble] background stale pairing cleanup cooldown active; using long retry backoff err={}",
                            embedded_ble_log_preview(&err),
                        );
                    }
                    retry_delay = if stale_cleanup_backoff {
                        EMBEDDED_BLE_RETRY_OFFLINE_DELAY
                    } else {
                        next_embedded_ble_background_retry_delay(&err, retry_delay)
                    };
                }
                log::warn!(
                    "[embedded-ble] background listen retrying in {} ms after: {err}",
                    retry_delay.as_millis()
                );
                tokio::time::sleep(retry_delay).await;
                continue;
            }
        }
        clear_embedded_ble_listener_cancel(&inner, &cancel_capture);
    }
    log::info!("[embedded-ble] background listener stopped generation={generation}");
}

async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
    inner: &Arc<Inner>,
    err: &str,
    last_cleanup_at: &mut Option<Instant>,
) -> bool {
    let snapshot = embedded_ble_wake_recovery_snapshot(inner);
    let now = Instant::now();
    if !should_attempt_embedded_ble_background_stale_pairing_cleanup(
        err,
        &snapshot,
        *last_cleanup_at,
        now,
    ) {
        return false;
    }
    *last_cleanup_at = Some(now);

    log::warn!(
        "[embedded-ble] background stale pairing cleanup triggered reconnect_attempts={} notify_state={:?} usb_powered={:?} err={}",
        snapshot.reconnect_attempts,
        snapshot.notify_subscription_state,
        snapshot.usb_powered,
        embedded_ble_log_preview(err),
    );
    emit_embedded_ble_recovery_capsule(
        inner,
        "reconnecting",
        "Listener BLE 配对缓存异常，正在清理旧连接...",
        Some(2600),
    );

    let cleanup = async_runtime::spawn_blocking(crate::embedded_ble::unpair_listener_devices).await;
    match cleanup {
        Ok(unpair) => {
            log::warn!(
                "[embedded-ble] background stale pairing cleanup result status={:?} matched={} removed={} already_clean={} failed={} user_action={}",
                unpair.status,
                unpair.matched_devices,
                unpair.unpaired_devices,
                unpair.already_unpaired_devices,
                unpair.failed_devices,
                unpair.needs_user_action,
            );
            {
                let mut wake = inner.embedded_ble_wake_recovery.lock();
                wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
                wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Failed;
                wake.recent_disconnect_reason = Some(format!(
                    "background stale pairing cleanup status={:?}; previous error: {}",
                    unpair.status,
                    embedded_ble_log_preview(err),
                ));
                wake.user_guidance = match unpair.status {
                    crate::embedded_ble::BleDeviceUnpairStatus::Removed => {
                        "旧的 Listener 蓝牙配对已清理。请在 Windows 蓝牙设置里重新配对 Listener，Type 会自动恢复。".to_string()
                    }
                    crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean
                    | crate::embedded_ble::BleDeviceUnpairStatus::NotFound => {
                        "Type 没找到可继续自动清理的旧配对。请在 Windows 蓝牙设置里确认 Listener 已重新配对。".to_string()
                    }
                    crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction => {
                        "Windows 需要手动确认移除旧 Listener。请在蓝牙设置里删除 Listener 后重新配对。".to_string()
                    }
                };
            }
            emit_embedded_ble_recovery_capsule(
                inner,
                "reconnecting",
                "请在 Windows 蓝牙里重新配对 Listener，Type 会自动恢复。",
                Some(4200),
            );
            true
        }
        Err(err) => {
            log::warn!("[embedded-ble] background stale pairing cleanup task failed: {err}");
            false
        }
    }
}

fn should_attempt_embedded_ble_background_stale_pairing_cleanup(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    if !is_embedded_ble_background_stale_pairing_cleanup_candidate(err, snapshot) {
        return false;
    }
    if last_cleanup_at.is_some_and(|last| {
        now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
    }) {
        return false;
    }
    true
}

fn should_throttle_embedded_ble_background_stale_pairing_cleanup(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    is_embedded_ble_background_stale_pairing_cleanup_candidate(err, snapshot)
        && last_cleanup_at.is_some_and(|last| {
            now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
        })
}

fn is_embedded_ble_background_stale_pairing_cleanup_candidate(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
) -> bool {
    if snapshot.reconnect_attempts < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD {
        return false;
    }
    if snapshot.usb_powered == Some(false) {
        return false;
    }
    if !matches!(
        snapshot.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Failed
            | EmbeddedBleNotifySubscriptionState::Opening
            | EmbeddedBleNotifySubscriptionState::Lost
            | EmbeddedBleNotifySubscriptionState::Unknown
    ) {
        return false;
    }

    let failure = crate::embedded_ble::classify_ble_failure(err);
    matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::CccdProtocolError
            | crate::embedded_ble::BleFailureKind::StaleGattService
            | crate::embedded_ble::BleFailureKind::MissingPairing
    )
}

fn next_embedded_ble_background_retry_delay(err: &str, current: Duration) -> Duration {
    if is_embedded_ble_idle_timeout_error(err) {
        return EMBEDDED_BLE_RETRY_BASE_DELAY;
    }

    if is_embedded_ble_link_loss_error(err) {
        return EMBEDDED_BLE_RETRY_FAST_DELAY;
    }

    if is_embedded_ble_background_offline_backoff_error(err) {
        return EMBEDDED_BLE_RETRY_OFFLINE_DELAY;
    }

    if is_embedded_ble_transient_reopen_error(err) {
        return EMBEDDED_BLE_RETRY_LONG_DELAY
            .max(current)
            .min(EMBEDDED_BLE_RETRY_MAX_DELAY);
    }

    current
        .saturating_mul(2)
        .clamp(EMBEDDED_BLE_RETRY_BASE_DELAY, EMBEDDED_BLE_RETRY_MAX_DELAY)
}

fn is_embedded_ble_automatic_recovery_error(err: &str) -> bool {
    is_embedded_ble_link_loss_error(err)
        || crate::embedded_ble::classify_ble_failure(err).automatic_recovery
}

fn is_embedded_ble_background_offline_backoff_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("gatt session did not become active")
        || lower.contains("paired device disconnected")
        || lower.contains("stale gatt/cache")
        || lower.contains("device is asleep")
        || lower.contains("device asleep")
        || lower.contains("wake key")
        || lower.contains("not found from service selector")
}

fn embedded_ble_usb_power_allows_low_power_idle(usb_powered: Option<bool>) -> bool {
    usb_powered == Some(false)
}

fn embedded_ble_log_preview(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.chars().take(240).collect()
}

fn is_embedded_ble_low_power_idle_candidate(err: &str) -> bool {
    matches!(
        crate::embedded_ble::classify_ble_failure(err).kind,
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
    )
}

fn should_emit_embedded_ble_background_recovery_capsule(inner: &Arc<Inner>, err: &str) -> bool {
    let failure = crate::embedded_ble::classify_ble_failure(err);
    let link_loss = is_embedded_ble_link_loss_error(err);
    let automatic_recovery = link_loss || failure.automatic_recovery;
    let offline_backoff = is_embedded_ble_background_offline_backoff_error(err);
    let low_power_idle = matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
    );
    let snapshot = inner.embedded_ble_wake_recovery.lock();
    let reconnect_attempts = snapshot.reconnect_attempts;
    let usb_powered = snapshot.usb_powered;
    drop(snapshot);
    let low_power_recovery_capsule_allowed = !low_power_idle || usb_powered == Some(true);
    let decision = automatic_recovery
        && !offline_backoff
        && reconnect_attempts <= 1
        && low_power_recovery_capsule_allowed;
    let decision_reason = if !automatic_recovery {
        "not_automatic_recovery"
    } else if offline_backoff {
        "offline_backoff"
    } else if reconnect_attempts > 1 {
        "repeat_attempt_suppressed"
    } else if low_power_idle && usb_powered != Some(true) {
        "low_power_idle_power_unknown_or_battery_suppressed"
    } else {
        "emit"
    };
    log::info!(
        "[embedded-ble] background recovery capsule decision emit={} reason={} kind={:?} automatic_recovery={} link_loss={} offline_backoff={} reconnect_attempts={} usb_powered={:?} low_power_idle={} err={}",
        decision,
        decision_reason,
        failure.kind,
        failure.automatic_recovery,
        link_loss,
        offline_backoff,
        reconnect_attempts,
        usb_powered,
        low_power_idle,
        embedded_ble_log_preview(err),
    );
    decision
}

fn should_emit_embedded_ble_recovered_capsule_for_reason(
    reason: &str,
    usb_powered: Option<bool>,
    reconnect_attempts: u32,
) -> bool {
    let normalized = reason.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "refresh" | "shutdown" | "test" | "test cleanup"
    ) {
        return false;
    }
    let failure = crate::embedded_ble::classify_ble_failure(reason);
    let automatic_recovery = is_embedded_ble_link_loss_error(reason) || failure.automatic_recovery;
    let repeated_automatic_recovery = automatic_recovery && reconnect_attempts > 1;
    !repeated_automatic_recovery
        && (!is_embedded_ble_low_power_idle_candidate(reason) || usb_powered == Some(true))
}

fn is_embedded_ble_link_loss_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("connection status changed")
        || lower.contains("gatt session status changed")
        || (lower.contains("notification wait failed") && lower.contains("disconnected"))
        || lower.contains("transport_not_ready")
        || lower.contains("transport not ready")
        || lower.contains("reason=546")
        || lower.contains("reason: 546")
        || lower.contains("reason 546")
        || lower.contains("low-power idle")
        || lower.contains("low power idle")
        || lower.contains("idle disconnect")
}

fn is_embedded_ble_transient_reopen_error(err: &str) -> bool {
    err.contains("GattCommunicationStatus(3)")
        || err.contains("HRESULT(0x800706BA)")
        || err.contains("BLE characteristic discovery returned status")
        || err.contains("BLE service open wait failed")
}

fn is_embedded_ble_idle_timeout_error(err: &str) -> bool {
    err.contains("BLE embedded audio capture timed out")
}

fn embedded_ble_listener_capture_active(inner: &Arc<Inner>) -> bool {
    inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|cancel| !cancel.load(Ordering::SeqCst))
}

fn embedded_ble_listener_capture_ready(inner: &Arc<Inner>) -> bool {
    embedded_ble_listener_capture_active(inner)
        && inner.embedded_ble_listener_ready.load(Ordering::SeqCst)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EmbeddedBleForegroundProbeMode {
    RefreshBackgroundListener,
    StartBackgroundListener,
    ForegroundProbe,
}

fn embedded_ble_foreground_probe_mode(inner: &Arc<Inner>) -> EmbeddedBleForegroundProbeMode {
    if embedded_ble_listener_capture_active(inner) {
        EmbeddedBleForegroundProbeMode::RefreshBackgroundListener
    } else if embedded_ble_background_listener_expected(inner) {
        EmbeddedBleForegroundProbeMode::StartBackgroundListener
    } else {
        EmbeddedBleForegroundProbeMode::ForegroundProbe
    }
}

fn embedded_ble_background_listener_expected(inner: &Arc<Inner>) -> bool {
    std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
        .ok()
        .as_deref()
        != Some("1")
        && inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
}

async fn wait_for_embedded_ble_listener_ready(
    inner: &Arc<Inner>,
    timeout: Duration,
) -> Result<(), String> {
    if !embedded_ble_background_listener_expected(inner)
        && !embedded_ble_listener_capture_active(inner)
    {
        return Ok(());
    }
    let deadline = Instant::now() + timeout;
    loop {
        if embedded_ble_listener_capture_ready(inner) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let last_error = inner.embedded_ble_listener_last_error.lock().clone();
            let suffix = last_error
                .as_deref()
                .map(|err| format!("; last error: {err}"))
                .unwrap_or_default();
            return Err(format!(
                "Listener BLE notify subscription did not recover within {} ms after foreground probe{suffix}",
                timeout.as_millis()
            ));
        }
        tokio::time::sleep(EMBEDDED_BLE_PROBE_RECOVERY_POLL).await;
    }
}

async fn wait_for_embedded_ble_listener_inactive(inner: &Arc<Inner>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !embedded_ble_listener_capture_active(inner) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(EMBEDDED_BLE_PROBE_RECOVERY_POLL).await;
    }
}

fn mark_embedded_ble_listener_ready(inner: &Arc<Inner>, cancel: &Arc<AtomicBool>) {
    if inner
        .embedded_ble_listener_cancel
        .lock()
        .as_ref()
        .is_some_and(|active| Arc::ptr_eq(active, cancel))
        && !cancel.load(Ordering::SeqCst)
    {
        inner
            .embedded_ble_listener_ready
            .store(true, Ordering::SeqCst);
        let recovered = record_embedded_ble_notify_ready(inner);
        record_embedded_ble_session_actor_command(
            inner,
            EmbeddedBleSessionActorCommand::NotifyReady,
            None,
            "background notify subscription ready",
        );
        if recovered {
            emit_embedded_ble_recovery_capsule(
                inner,
                "reconnected",
                "Listener BLE 已重连，音频通道已恢复。",
                Some(1400),
            );
        }
        flush_pending_device_key_ble_action(inner, "notify_ready");
        log::info!("[embedded-ble] background listener notify ready");
    }
}

fn install_embedded_ble_listener_cancel(inner: &Arc<Inner>, generation: u64) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    inner
        .embedded_ble_listener_ready
        .store(false, Ordering::SeqCst);
    let previous = {
        let mut slot = inner.embedded_ble_listener_cancel.lock();
        slot.replace(Arc::clone(&cancel))
    };
    if let Some(previous) = previous {
        previous.store(true, Ordering::SeqCst);
        log::warn!(
            "[embedded-ble] replaced an active background capture cancel flag (generation={generation})"
        );
    }
    log::info!("[embedded-ble] background capture armed generation={generation}");
    cancel
}

fn clear_embedded_ble_listener_cancel(inner: &Arc<Inner>, cancel: &Arc<AtomicBool>) {
    let mut slot = inner.embedded_ble_listener_cancel.lock();
    if slot
        .as_ref()
        .is_some_and(|active| Arc::ptr_eq(active, cancel))
    {
        *slot = None;
        inner
            .embedded_ble_listener_ready
            .store(false, Ordering::SeqCst);
        log::info!("[embedded-ble] background capture cancel flag cleared");
    }
}

fn cancel_embedded_ble_listener_capture(inner: &Arc<Inner>, reason: &str) {
    inner
        .embedded_ble_listener_ready
        .store(false, Ordering::SeqCst);
    let previous = inner.embedded_ble_listener_cancel.lock().take();
    if let Some(cancel) = previous {
        record_embedded_ble_session_actor_command(
            inner,
            EmbeddedBleSessionActorCommand::NotifyCleanupDelay,
            None,
            format!("reason={reason}"),
        );
        cancel.store(true, Ordering::SeqCst);
        record_embedded_ble_listener_cancelled(inner, reason);
        log::info!("[embedded-ble] requested active background capture stop ({reason})");
    }
}

fn pause_embedded_ble_listener_capture(inner: &Arc<Inner>, reason: &str) {
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    log::info!("[embedded-ble] paused background listener generation={generation} ({reason})");
    cancel_embedded_ble_listener_capture(inner, reason);
}

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
                cancel_session(&inner_cloned);
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
    polished: &str,
    restore_clipboard: bool,
    allow_non_tsf_insertion_fallback: bool,
    paste_shortcut: PasteShortcut,
    ime_target: Option<ImeSubmitTarget>,
) -> InsertStatus {
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
        return InsertStatus::Failed;
    };

    let request = crate::windows_ime_ipc::ImeSubmitRequest {
        session_id: Uuid::new_v4().to_string(),
        text: polished.to_string(),
        created_at: Utc::now().to_rfc3339(),
        target: ime_target,
    };

    let ime_status = match inner.windows_ime.submit_prepared(&prepared, request).await {
        Ok(status) => status,
        Err(error) => {
            log::warn!("[windows-ime] TSF submit failed: {error}");
            InsertStatus::Failed
        }
    };
    inner.windows_ime.restore_session(prepared);

    if ime_status == InsertStatus::Inserted {
        ime_status
    } else if should_try_non_tsf_insertion_fallback(allow_non_tsf_insertion_fallback, ime_status) {
        insert_via_non_tsf_fallback(inner, polished, restore_clipboard, paste_shortcut)
    } else {
        log::warn!("[windows-ime] TSF did not insert; non-TSF insertion fallback is disabled");
        InsertStatus::Failed
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
) -> InsertStatus {
    if inner.inserter.insert_via_unicode_keystrokes(polished) == InsertStatus::Inserted {
        log::info!("[windows-ime] TSF unavailable; inserted via Unicode SendInput");
        InsertStatus::Inserted
    } else {
        inner
            .inserter
            .insert_via_clipboard_fallback(polished, restore_clipboard, paste_shortcut)
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

fn ensure_asr_credentials() -> Result<(), String> {
    let active_asr = CredentialsVault::get_active_asr();

    // 本地 Qwen3-ASR 没有"凭据"概念，但需要：(a) macOS 平台 (b) 模型已下载。
    if crate::asr::local::is_local_qwen3(&active_asr) {
        #[cfg(not(target_os = "macos"))]
        {
            return Err("本地 ASR 当前仅支持 macOS（Windows 见 issue #256）".to_string());
        }
        #[cfg(target_os = "macos")]
        {
            return ensure_local_qwen3_model_ready();
        }
    }

    if crate::asr::local::foundry::is_foundry_local_whisper(&active_asr) {
        #[cfg(not(target_os = "windows"))]
        {
            return Err("Foundry Local Whisper 当前仅支持 Windows".to_string());
        }
        #[cfg(target_os = "windows")]
        {
            return Ok(());
        }
    }

    if is_whisper_compatible_provider(&active_asr) || is_bailian_provider(&active_asr) {
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

    let provider = build_active_llm_provider(llm_thinking_enabled)?;
    Ok(provider
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
        .await?)
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

fn read_whisper_credentials() -> anyhow::Result<(String, String, String, ProviderProxyConfig)> {
    let active_asr = CredentialsVault::get_active_asr();
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
    inner
        .vocab
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|e| DictionaryHotword {
            phrase: e.phrase,
            enabled: e.enabled,
        })
        .collect()
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

    let api_key = CredentialsVault::get(CredentialAccount::ArkApiKey)?.unwrap_or_default();
    let model = model.unwrap_or_else(|| "deepseek-v3-2".to_string());
    let endpoint = resolve_ark_endpoint(&api_key)?;
    let base_url = endpoint
        .trim_end_matches("/chat/completions")
        .trim_end_matches('/')
        .to_string();
    let proxy_config = read_llm_proxy_config(&active)?;
    let config = OpenAICompatibleConfig::new(active, "Listener Type LLM", base_url, api_key, model)
        .with_thinking_enabled(llm_thinking_enabled)
        .with_proxy_config(proxy_config);
    Ok(ActiveLLMProvider::OpenAI(OpenAICompatibleLLMProvider::new(
        config,
    )))
}

fn resolve_ark_endpoint(api_key: &str) -> anyhow::Result<String> {
    let endpoint = CredentialsVault::get(CredentialAccount::ArkEndpoint)?.filter(|s| !s.is_empty());
    resolve_ark_endpoint_with_policy(api_key, endpoint)
}

fn resolve_ark_endpoint_with_policy(
    api_key: &str,
    endpoint: Option<String>,
) -> anyhow::Result<String> {
    if api_key.trim().is_empty() && endpoint.is_none() {
        anyhow::bail!("API Key 为空");
    }
    Ok(endpoint
        .unwrap_or_else(|| "https://ark.cn-beijing.volces.com/api/v3/chat/completions".to_string()))
}

#[cfg(test)]
mod tests {
    use super::dictation::abort_recording_with_error;
    use super::*;
    use crate::types::{DictationInputSource, HotkeyMode, HotkeyTrigger};
    use once_cell::sync::Lazy;

    static ENV_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

    fn temp_path_for_test(name: &str) -> std::path::PathBuf {
        let path = std::path::Path::new(name);
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(name);
        let suffix = format!("{}-{}", std::process::id(), Uuid::new_v4());
        let file_name = match path.extension().and_then(|value| value.to_str()) {
            Some(ext) => format!("listener-type-{stem}-{suffix}.{ext}"),
            None => format!("listener-type-{stem}-{suffix}"),
        };
        std::env::temp_dir().join(file_name)
    }

    fn session_id(n: u128) -> SessionId {
        Uuid::from_u128(n)
    }

    #[test]
    fn external_app_path_validation_rejects_command_text() {
        assert!(validate_external_app_path("code").is_err());
        assert!(validate_external_app_path("cmd /C start notepad").is_err());
    }

    #[test]
    fn external_app_path_validation_rejects_script_files() {
        #[cfg(target_os = "windows")]
        let path = temp_path_for_test("device-key-script.cmd");
        #[cfg(target_os = "macos")]
        let path = temp_path_for_test("device-key-script.command");
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        let path = temp_path_for_test("device-key-script.sh");

        std::fs::write(&path, b"echo unsafe").unwrap();
        let result = validate_external_app_path(&path.display().to_string());
        let _ = std::fs::remove_file(&path);

        assert!(result.is_err());
    }

    #[test]
    fn external_app_path_validation_accepts_platform_app_entry() {
        #[cfg(target_os = "windows")]
        {
            let path = temp_path_for_test("device-key-app.exe");
            std::fs::write(&path, b"").unwrap();
            let result = validate_external_app_path(&path.display().to_string());
            let _ = std::fs::remove_file(&path);
            assert!(result.is_ok());
        }

        #[cfg(target_os = "macos")]
        {
            let path = temp_path_for_test("DeviceKeyTest.app");
            std::fs::create_dir(&path).unwrap();
            let result = validate_external_app_path(&path.display().to_string());
            let _ = std::fs::remove_dir(&path);
            assert!(result.is_ok());
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            let path = temp_path_for_test("device-key-app.desktop");
            std::fs::write(&path, b"[Desktop Entry]\nType=Application\nName=Test\n").unwrap();
            let result = validate_external_app_path(&path.display().to_string());
            let _ = std::fs::remove_file(&path);
            assert!(result.is_ok());
        }
    }

    fn force_microphone_input_for_test(coordinator: &Coordinator) {
        let mut prefs = coordinator.inner.prefs.get();
        prefs.dictation_input_source = DictationInputSource::Microphone;
        coordinator.inner.prefs.replace_for_tests(prefs);
    }

    fn force_embedded_ble_input_for_test(coordinator: &Coordinator) {
        let mut prefs = coordinator.inner.prefs.get();
        prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
        coordinator.inner.prefs.replace_for_tests(prefs);
    }

    fn firmware_snapshot_for_auto_input_test(
        connected: bool,
    ) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected,
            hardware_revision: connected.then(|| "keyboard-v2".to_string()),
            firmware_version: connected.then(|| "v-test".to_string()),
            capabilities: connected
                .then(|| vec!["firmware_ota_v1".to_string()])
                .unwrap_or_default(),
            battery_percent: connected.then_some(91),
            usb_powered: connected.then_some(true),
            detail: (!connected).then(|| "No paired Listener device".to_string()),
        }
    }

    #[test]
    fn embedded_ble_auto_input_source_prefers_connected_device() {
        let mut prefs = crate::types::UserPreferences {
            dictation_input_source: DictationInputSource::Microphone,
            ..crate::types::UserPreferences::default()
        };
        assert!(should_auto_select_embedded_ble_input_source(
            &prefs,
            &firmware_snapshot_for_auto_input_test(true)
        ));

        prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
        assert!(!should_auto_select_embedded_ble_input_source(
            &prefs,
            &firmware_snapshot_for_auto_input_test(true)
        ));

        prefs.dictation_input_source = DictationInputSource::Microphone;
        assert!(!should_auto_select_embedded_ble_input_source(
            &prefs,
            &firmware_snapshot_for_auto_input_test(false)
        ));

        prefs.dictation_input_source_user_overridden = true;
        assert!(!should_auto_select_embedded_ble_input_source(
            &prefs,
            &firmware_snapshot_for_auto_input_test(true)
        ));
    }

    #[test]
    fn device_key_dictation_debounce_matches_hotkey_edge_debounce() {
        let coordinator = Coordinator::new();
        let mapping = DeviceCustomKeyMapping {
            action: DeviceCustomKeyAction::Dictation,
            ..DeviceCustomKeyMapping::default()
        };

        assert_eq!(HOTKEY_DEBOUNCE, Duration::from_millis(250));
        assert!(!device_key_action_debounced(
            &coordinator.inner,
            DeviceCustomKeyId::Key3,
            DeviceCustomKeyGesture::SingleClick,
            &mapping
        ));
        assert!(device_key_action_debounced(
            &coordinator.inner,
            DeviceCustomKeyId::Key3,
            DeviceCustomKeyGesture::SingleClick,
            &mapping
        ));

        coordinator.inner.device_key_last_dispatch_at.lock().insert(
            (DeviceCustomKeyGesture::SingleClick, DeviceCustomKeyId::Key3),
            Instant::now() - HOTKEY_DEBOUNCE - Duration::from_millis(1),
        );
        assert!(!device_key_action_debounced(
            &coordinator.inner,
            DeviceCustomKeyId::Key3,
            DeviceCustomKeyGesture::SingleClick,
            &mapping
        ));
    }

    #[test]
    fn device_key_ble_recording_control_feedback_uses_active_session() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        let session = current_device_key_recording_control_session(&coordinator.inner);
        assert_eq!(session, Some((session_id, SessionPhase::Listening)));
        assert_eq!(
            emit_device_key_recording_control_capsule(
                &coordinator.inner,
                session,
                DictationUiState::Transcribing,
                CapsuleState::Reconnecting,
                "正在发送设备录音停止控制...".to_string(),
            ),
            Some(session_id)
        );

        coordinator.inner.state.lock().phase = SessionPhase::Idle;
        let idle_session = current_device_key_recording_control_session(&coordinator.inner);
        assert_eq!(idle_session, None);
        assert_eq!(
            emit_device_key_recording_control_capsule(
                &coordinator.inner,
                idle_session,
                DictationUiState::Recording,
                CapsuleState::Reconnecting,
                "正在发送设备录音控制，等待 Listener 音频...".to_string(),
            ),
            None
        );
    }

    #[test]
    fn device_key_ble_recording_control_ignores_starting_retry() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Starting;
            state.started_at = Instant::now() - Duration::from_millis(375);
            state.cancelled = false;
        }

        match device_key_ble_recording_control_decision(&coordinator.inner) {
            DeviceKeyBleRecordingControlDecision::IgnoreStarting {
                session_id: actual_session_id,
                elapsed_ms,
            } => {
                assert_eq!(actual_session_id, session_id);
                assert!(elapsed_ms >= 300);
            }
            other => panic!("unexpected decision: {other:?}"),
        }

        {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Listening;
        }
        assert_eq!(
            device_key_ble_recording_control_decision(&coordinator.inner),
            DeviceKeyBleRecordingControlDecision::Stop {
                session_id,
                phase: SessionPhase::Listening
            }
        );

        coordinator.inner.state.lock().phase = SessionPhase::Idle;
        assert_eq!(
            device_key_ble_recording_control_decision(&coordinator.inner),
            DeviceKeyBleRecordingControlDecision::Start
        );
    }

    #[test]
    fn device_key_ble_pending_start_clears_exact_action_only() {
        let coordinator = Coordinator::new();
        queue_pending_device_key_ble_start(
            &coordinator.inner,
            DeviceCustomKeyId::Key1,
            DeviceCustomKeyGesture::SingleClick,
            "test",
        );

        assert!(!clear_pending_device_key_ble_start(
            &coordinator.inner,
            DeviceCustomKeyId::Key2,
            DeviceCustomKeyGesture::SingleClick,
            "wrong_key",
        ));
        assert!(coordinator
            .inner
            .device_key_pending_ble_action
            .lock()
            .is_some());

        assert!(clear_pending_device_key_ble_start(
            &coordinator.inner,
            DeviceCustomKeyId::Key1,
            DeviceCustomKeyGesture::SingleClick,
            "right_key",
        ));
        assert!(coordinator
            .inner
            .device_key_pending_ble_action
            .lock()
            .is_none());
    }

    #[test]
    fn device_key_ble_pending_start_expires() {
        let coordinator = Coordinator::new();
        *coordinator.inner.device_key_pending_ble_action.lock() = Some(PendingDeviceKeyBleAction {
            kind: PendingDeviceKeyBleActionKind::Start,
            key: DeviceCustomKeyId::Key1,
            gesture: DeviceCustomKeyGesture::SingleClick,
            queued_at: Instant::now()
                - DEVICE_KEY_BLE_PENDING_ACTION_TTL
                - Duration::from_millis(1),
        });

        assert!(take_pending_device_key_ble_start(&coordinator.inner, "test").is_none());
        assert!(coordinator
            .inner
            .device_key_pending_ble_action
            .lock()
            .is_none());
    }

    #[test]
    fn device_key_ble_retry_pending_only_for_start_recoverable_errors() {
        assert!(should_keep_device_key_ble_start_pending_after_error(
            DeviceKeyBleRecordingControlDecision::Start,
            "Listener BLE low-power idle disconnect"
        ));
        assert!(!should_keep_device_key_ble_start_pending_after_error(
            DeviceKeyBleRecordingControlDecision::Start,
            "no characteristics found for audio control"
        ));
        assert!(!should_keep_device_key_ble_start_pending_after_error(
            DeviceKeyBleRecordingControlDecision::Stop {
                session_id: new_session_id(),
                phase: SessionPhase::Listening,
            },
            "Listener BLE low-power idle disconnect"
        ));
        assert_eq!(
            should_keep_device_key_ble_action_pending_after_error(
                DeviceKeyBleRecordingControlDecision::Stop {
                    session_id: new_session_id(),
                    phase: SessionPhase::Listening,
                },
                "Listener BLE low-power idle disconnect"
            ),
            Some(PendingDeviceKeyBleActionKind::Stop)
        );
    }

    #[test]
    fn device_key_ble_pending_start_drops_if_state_changed_before_flush() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        queue_pending_device_key_ble_start(
            &coordinator.inner,
            DeviceCustomKeyId::Key1,
            DeviceCustomKeyGesture::SingleClick,
            "test",
        );

        flush_pending_device_key_ble_start(&coordinator.inner, "test");

        assert!(coordinator
            .inner
            .device_key_pending_ble_action
            .lock()
            .is_none());
    }

    #[test]
    fn device_key_ble_pending_stop_restores_until_notify_ready() {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        queue_pending_device_key_ble_stop(
            &coordinator.inner,
            DeviceCustomKeyId::Key1,
            DeviceCustomKeyGesture::SingleClick,
            "test",
        );

        flush_pending_device_key_ble_action(&coordinator.inner, "test");

        let pending = coordinator.inner.device_key_pending_ble_action.lock();
        assert!(pending.as_ref().is_some_and(|action| {
            action.kind == PendingDeviceKeyBleActionKind::Stop
                && action.key == DeviceCustomKeyId::Key1
                && action.gesture == DeviceCustomKeyGesture::SingleClick
        }));
    }

    #[test]
    fn embedded_ble_listener_cancel_replacement_is_pointer_safe() {
        let coordinator = Coordinator::new();
        let first = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
        assert!(!first.load(Ordering::SeqCst));
        assert!(embedded_ble_listener_capture_active(&coordinator.inner));
        assert!(!embedded_ble_listener_capture_ready(&coordinator.inner));
        mark_embedded_ble_listener_ready(&coordinator.inner, &first);
        assert!(embedded_ble_listener_capture_ready(&coordinator.inner));

        let second = install_embedded_ble_listener_cancel(&coordinator.inner, 2);
        assert!(first.load(Ordering::SeqCst));
        assert!(embedded_ble_listener_capture_active(&coordinator.inner));
        assert!(!embedded_ble_listener_capture_ready(&coordinator.inner));

        clear_embedded_ble_listener_cancel(&coordinator.inner, &first);
        assert!(coordinator
            .inner
            .embedded_ble_listener_cancel
            .lock()
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &second)));

        cancel_embedded_ble_listener_capture(&coordinator.inner, "test");
        assert!(second.load(Ordering::SeqCst));
        assert!(coordinator
            .inner
            .embedded_ble_listener_cancel
            .lock()
            .is_none());
        assert!(!embedded_ble_listener_capture_active(&coordinator.inner));
        assert!(!embedded_ble_listener_capture_ready(&coordinator.inner));
    }

    #[tokio::test]
    async fn embedded_ble_foreground_probe_refreshes_ready_background_capture() {
        let coordinator = Coordinator::new();
        let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
        mark_embedded_ble_listener_ready(&coordinator.inner, &active);
        record_embedded_ble_listener_last_error(
            &coordinator.inner,
            "BLE CCCD write async error: Some(HRESULT(0x800706BA))",
        );

        let _ = coordinator
            .probe_embedded_audio_ble_subscription(Some(1_000))
            .await;

        assert!(active.load(Ordering::SeqCst));
        assert!(!coordinator
            .inner
            .embedded_ble_listener_cancel
            .lock()
            .as_ref()
            .is_some_and(|cancel| Arc::ptr_eq(cancel, &active)));
    }

    #[tokio::test]
    async fn embedded_ble_start_dictation_reuses_ready_background_without_reopen() {
        let coordinator = Coordinator::new();
        force_embedded_ble_input_for_test(&coordinator);
        let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
        mark_embedded_ble_listener_ready(&coordinator.inner, &active);
        let generation_before = coordinator.embedded_ble_listener_generation();

        coordinator.start_dictation().await.unwrap();

        assert_eq!(
            coordinator.embedded_ble_listener_generation(),
            generation_before
        );
        assert!(!active.load(Ordering::SeqCst));
        assert!(embedded_ble_listener_capture_ready(&coordinator.inner));
        assert!(coordinator
            .inner
            .embedded_ble_listener_cancel
            .lock()
            .as_ref()
            .is_some_and(|cancel| Arc::ptr_eq(cancel, &active)));
    }

    #[test]
    fn embedded_ble_foreground_probe_refreshes_active_background_even_when_unready() {
        let coordinator = Coordinator::new();
        let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);

        assert_eq!(
            embedded_ble_foreground_probe_mode(&coordinator.inner),
            EmbeddedBleForegroundProbeMode::RefreshBackgroundListener
        );

        cancel_embedded_ble_listener_capture(&coordinator.inner, "test cleanup");
        assert!(active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn embedded_ble_foreground_probe_routes_selected_source_to_background_listener() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE");

        let coordinator = Coordinator::new();
        force_microphone_input_for_test(&coordinator);
        assert_eq!(
            embedded_ble_foreground_probe_mode(&coordinator.inner),
            EmbeddedBleForegroundProbeMode::ForegroundProbe
        );

        let mut prefs = coordinator.inner.prefs.get();
        prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
        coordinator.inner.prefs.replace_for_tests(prefs);
        assert_eq!(
            embedded_ble_foreground_probe_mode(&coordinator.inner),
            EmbeddedBleForegroundProbeMode::StartBackgroundListener
        );

        std::env::set_var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE", "1");
        assert_eq!(
            embedded_ble_foreground_probe_mode(&coordinator.inner),
            EmbeddedBleForegroundProbeMode::ForegroundProbe
        );

        std::env::remove_var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE");
        let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
        assert_eq!(
            embedded_ble_foreground_probe_mode(&coordinator.inner),
            EmbeddedBleForegroundProbeMode::RefreshBackgroundListener
        );
        cancel_embedded_ble_listener_capture(&coordinator.inner, "test cleanup");
        assert!(active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn embedded_ble_repair_refreshes_ready_background_status() {
        let coordinator = Coordinator::new();
        let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
        mark_embedded_ble_listener_ready(&coordinator.inner, &active);

        let result = coordinator
            .repair_embedded_ble_connection(Some(1_000))
            .await;

        assert!(active.load(Ordering::SeqCst));
        assert!(!coordinator
            .inner
            .embedded_ble_listener_cancel
            .lock()
            .as_ref()
            .is_some_and(|cancel| Arc::ptr_eq(cancel, &active)));
        if let Ok(snapshot) = result {
            assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Ready);
            assert_eq!(
                snapshot.notify_subscription_state,
                EmbeddedBleNotifySubscriptionState::Subscribed
            );
        }
    }

    #[test]
    fn embedded_ble_background_retry_backs_off_for_transient_reopen_errors() {
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                "listener: BLE characteristic discovery returned status=GattCommunicationStatus(3)",
                EMBEDDED_BLE_RETRY_BASE_DELAY,
            ),
            EMBEDDED_BLE_RETRY_LONG_DELAY
        );
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                "BLE CCCD write async error: Some(HRESULT(0x800706BA))",
                Duration::from_secs(10),
            ),
            EMBEDDED_BLE_RETRY_MAX_DELAY
        );
    }

    #[test]
    fn embedded_ble_background_retry_backs_off_for_offline_gatt_failures() {
        let message = "Embedded audio BLE service not found from service selector; device-address fallback failed: BLE device notify path D3B1B3DAC206 failed: BluetoothCacheMode(0): BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected";

        assert!(is_embedded_ble_background_offline_backoff_error(message));
        assert_eq!(
            next_embedded_ble_background_retry_delay(message, EMBEDDED_BLE_RETRY_BASE_DELAY),
            EMBEDDED_BLE_RETRY_OFFLINE_DELAY
        );
    }

    #[test]
    fn embedded_ble_background_retry_caps_generic_errors() {
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                "BLE embedded audio notification wait failed: channel closed unexpectedly",
                EMBEDDED_BLE_RETRY_BASE_DELAY,
            ),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                "BLE embedded audio notification wait failed: channel closed unexpectedly",
                Duration::from_secs(10),
            ),
            EMBEDDED_BLE_RETRY_MAX_DELAY
        );
    }

    #[test]
    fn embedded_ble_background_retry_is_fast_for_link_loss_events() {
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                "BLE device connection status changed to Disconnected; transport_not_ready",
                Duration::from_secs(4),
            ),
            EMBEDDED_BLE_RETRY_FAST_DELAY
        );
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                "BLE embedded audio notification wait failed: disconnected",
                Duration::from_secs(4),
            ),
            EMBEDDED_BLE_RETRY_FAST_DELAY
        );
        assert!(is_embedded_ble_automatic_recovery_error(
            "BLE GATT session status changed to Some(GattSessionStatus(0)); transport_not_ready"
        ));
    }

    #[test]
    fn embedded_ble_background_retry_keeps_idle_timeout_responsive() {
        assert_eq!(
            next_embedded_ble_background_retry_delay(
                "BLE embedded audio capture timed out after 60000 ms",
                Duration::from_secs(10),
            ),
            EMBEDDED_BLE_RETRY_BASE_DELAY
        );
    }

    #[test]
    fn embedded_ble_listener_error_snapshot_records_and_clears_setup_failures() {
        let coordinator = Coordinator::new();
        assert_eq!(coordinator.embedded_ble_listener_last_error(), None);

        record_embedded_ble_listener_last_error(
            &coordinator.inner,
            "BLE CCCD write async error: Some(HRESULT(0x800706BA))",
        );
        assert_eq!(
            coordinator.embedded_ble_listener_last_error(),
            Some("BLE CCCD write async error: Some(HRESULT(0x800706BA))".to_string())
        );

        clear_embedded_ble_listener_last_error(&coordinator.inner);
        assert_eq!(coordinator.embedded_ble_listener_last_error(), None);
    }

    #[test]
    fn embedded_ble_recording_control_guidance_calls_out_stale_firmware_or_gatt_cache() {
        let message = embedded_ble_recording_control_guidance(
            "audio control characteristic not found in Listener BLE service",
        );

        assert!(message.contains("BLE 录音控制特征"));
        assert!(message.contains("新固件"));
        assert!(message.contains("重新配对"));
    }

    #[test]
    fn embedded_ble_wake_recovery_snapshot_guides_idle_recovery() {
        let coordinator = Coordinator::new();

        record_embedded_ble_reconnect_attempt(&coordinator.inner, "test");
        let reconnecting = coordinator.embedded_ble_wake_recovery_snapshot();
        assert_eq!(
            reconnecting.status,
            EmbeddedBleWakeRecoveryStatus::Reconnecting
        );
        assert_eq!(reconnecting.reconnect_attempts, 1);
        assert_eq!(
            reconnecting.notify_subscription_state,
            EmbeddedBleNotifySubscriptionState::Opening
        );

        record_embedded_ble_recovery_failure(
            &coordinator.inner,
            "Listener BLE notify subscription did not recover within 1000 ms after foreground probe; last error: service not found",
        );
        let failed = coordinator.embedded_ble_wake_recovery_snapshot();
        assert_eq!(failed.status, EmbeddedBleWakeRecoveryStatus::NeedsWakeKey);
        assert!(failed.user_guidance.contains("KEY4"));
        assert_eq!(failed.firmware_wake_policy.policy, "key4_only");
        assert!(!failed.firmware_wake_policy.voice_key_deep_sleep_wake);

        record_embedded_ble_notify_ready(&coordinator.inner);
        let ready = coordinator.embedded_ble_wake_recovery_snapshot();
        assert_eq!(ready.status, EmbeddedBleWakeRecoveryStatus::Ready);
        assert_eq!(
            ready.notify_subscription_state,
            EmbeddedBleNotifySubscriptionState::Subscribed
        );
        assert!(ready.last_ready_at.is_some());
    }

    #[test]
    fn embedded_ble_wake_recovery_tracks_idle_disconnect_as_reconnecting() {
        let coordinator = Coordinator::new();
        coordinator
            .inner
            .embedded_ble_wake_recovery
            .lock()
            .usb_powered = Some(false);

        record_embedded_ble_recovery_failure(
            &coordinator.inner,
            "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
        );
        let snapshot = coordinator.embedded_ble_wake_recovery_snapshot();

        assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Reconnecting);
        assert_eq!(
            snapshot.notify_subscription_state,
            EmbeddedBleNotifySubscriptionState::Lost
        );
        assert!(snapshot.user_guidance.contains("离线状态断开"));
        assert!(snapshot
            .recent_disconnect_reason
            .as_deref()
            .unwrap_or_default()
            .contains("reason=546"));
    }

    #[test]
    fn embedded_ble_notify_ready_suppresses_battery_idle_recovery_capsule() {
        let coordinator = Coordinator::new();
        coordinator
            .inner
            .embedded_ble_wake_recovery
            .lock()
            .usb_powered = Some(false);

        record_embedded_ble_recovery_failure(
            &coordinator.inner,
            "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
        );
        assert!(!record_embedded_ble_notify_ready(&coordinator.inner));
        let snapshot = coordinator.embedded_ble_wake_recovery_snapshot();

        assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Ready);
        assert_eq!(
            snapshot.notify_subscription_state,
            EmbeddedBleNotifySubscriptionState::Subscribed
        );
        assert!(snapshot.recent_disconnect_reason.is_none());
    }

    #[test]
    fn embedded_ble_notify_ready_suppresses_unknown_power_idle_recovery_capsule() {
        let coordinator = Coordinator::new();

        record_embedded_ble_recovery_failure(
            &coordinator.inner,
            "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
        );
        assert!(!record_embedded_ble_notify_ready(&coordinator.inner));
    }

    #[test]
    fn embedded_ble_startup_power_snapshot_keeps_plugged_recovery_context() {
        let coordinator = Coordinator::new();
        let firmware = firmware_snapshot_for_auto_input_test(true);

        record_embedded_ble_firmware_power_snapshot(
            &coordinator.inner,
            &firmware,
            "startup_embedded_ble_power_probe",
        );
        record_embedded_ble_recovery_failure(
            &coordinator.inner,
            "BLE device connection status changed to Disconnected; transport_not_ready",
        );
        let snapshot = coordinator.embedded_ble_wake_recovery_snapshot();

        assert_eq!(snapshot.usb_powered, Some(true));
        assert_eq!(snapshot.status, EmbeddedBleWakeRecoveryStatus::Reconnecting);
        assert!(snapshot.user_guidance.contains("临时中断"));
    }

    #[test]
    fn embedded_ble_notify_ready_reports_recovered_for_powered_disconnect() {
        let coordinator = Coordinator::new();
        coordinator
            .inner
            .embedded_ble_wake_recovery
            .lock()
            .usb_powered = Some(true);

        record_embedded_ble_recovery_failure(
            &coordinator.inner,
            "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
        );
        assert!(record_embedded_ble_notify_ready(&coordinator.inner));
    }

    #[test]
    fn embedded_ble_notify_ready_suppresses_internal_refresh_capsule() {
        let coordinator = Coordinator::new();

        record_embedded_ble_listener_cancelled(&coordinator.inner, "refresh");

        assert!(!record_embedded_ble_notify_ready(&coordinator.inner));
    }

    #[test]
    fn embedded_ble_background_recovery_capsule_respects_power_state() {
        let coordinator = Coordinator::new();

        assert!(should_emit_embedded_ble_background_recovery_capsule(
            &coordinator.inner,
            "BLE device connection status changed to Disconnected; transport_not_ready",
        ));

        coordinator
            .inner
            .embedded_ble_wake_recovery
            .lock()
            .usb_powered = Some(false);
        assert!(should_emit_embedded_ble_background_recovery_capsule(
            &coordinator.inner,
            "BLE device connection status changed to Disconnected; transport_not_ready",
        ));

        assert!(!should_emit_embedded_ble_background_recovery_capsule(
            &coordinator.inner,
            "Windows BLE disconnected; reason=546; audio path returned transport_not_ready",
        ));

        coordinator
            .inner
            .embedded_ble_wake_recovery
            .lock()
            .usb_powered = Some(true);
        assert!(should_emit_embedded_ble_background_recovery_capsule(
            &coordinator.inner,
            "BLE device connection status changed to Disconnected; transport_not_ready",
        ));

        assert!(should_emit_embedded_ble_background_recovery_capsule(
            &coordinator.inner,
            "BLE CCCD write timed out after 8000 ms",
        ));

        record_embedded_ble_reconnect_attempt(&coordinator.inner, "background_retry_test");
        record_embedded_ble_reconnect_attempt(&coordinator.inner, "background_retry_test");
        assert!(!should_emit_embedded_ble_background_recovery_capsule(
            &coordinator.inner,
            "BLE device connection status changed to Disconnected; transport_not_ready",
        ));

        assert!(!should_emit_embedded_ble_background_recovery_capsule(
            &coordinator.inner,
            "BLE GATT session did not become active after 8000 ms initial=Some(GattSessionStatus(0)) current=Some(GattSessionStatus(0)); stale GATT/cache or paired device disconnected",
        ));
    }

    #[test]
    fn embedded_ble_background_stale_cleanup_requires_powered_repeated_cache_failure() {
        let now = Instant::now();
        let snapshot = EmbeddedBleWakeRecoverySnapshot {
            reconnect_attempts: EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD,
            notify_subscription_state: EmbeddedBleNotifySubscriptionState::Failed,
            usb_powered: Some(true),
            recent_disconnect_reason: Some(
                "BLE CCCD write async error: Some(HRESULT(0x800704C7))".to_string(),
            ),
            ..Default::default()
        };
        let cccd_error = "BLE CCCD write async error: Some(HRESULT(0x800704C7))";

        assert!(
            should_attempt_embedded_ble_background_stale_pairing_cleanup(
                cccd_error, &snapshot, None, now,
            )
        );

        let too_early = EmbeddedBleWakeRecoverySnapshot {
            reconnect_attempts: EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD - 1,
            ..snapshot.clone()
        };
        assert!(
            !should_attempt_embedded_ble_background_stale_pairing_cleanup(
                cccd_error, &too_early, None, now,
            )
        );

        let battery = EmbeddedBleWakeRecoverySnapshot {
            usb_powered: Some(false),
            ..snapshot.clone()
        };
        assert!(
            !should_attempt_embedded_ble_background_stale_pairing_cleanup(
                cccd_error, &battery, None, now,
            )
        );

        let unknown_power = EmbeddedBleWakeRecoverySnapshot {
            usb_powered: None,
            ..snapshot.clone()
        };
        assert!(
            should_attempt_embedded_ble_background_stale_pairing_cleanup(
                cccd_error,
                &unknown_power,
                None,
                now,
            )
        );

        let link_loss = "BLE device connection status changed to Disconnected; transport_not_ready";
        assert!(
            !should_attempt_embedded_ble_background_stale_pairing_cleanup(
                link_loss, &snapshot, None, now,
            )
        );

        assert!(
            !should_attempt_embedded_ble_background_stale_pairing_cleanup(
                cccd_error,
                &snapshot,
                Some(now - Duration::from_secs(60)),
                now,
            )
        );
        assert!(
            should_throttle_embedded_ble_background_stale_pairing_cleanup(
                cccd_error,
                &snapshot,
                Some(now - Duration::from_secs(60)),
                now,
            )
        );
        assert!(
            should_attempt_embedded_ble_background_stale_pairing_cleanup(
                cccd_error,
                &snapshot,
                Some(now - EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN - Duration::from_secs(1)),
                now,
            )
        );
        assert!(
            !should_throttle_embedded_ble_background_stale_pairing_cleanup(
                cccd_error,
                &snapshot,
                Some(now - EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN - Duration::from_secs(1)),
                now,
            )
        );
        assert!(
            !should_throttle_embedded_ble_background_stale_pairing_cleanup(
                link_loss,
                &snapshot,
                Some(now - Duration::from_secs(60)),
                now,
            )
        );
    }

    #[test]
    fn embedded_ble_repeated_notification_disconnects_stay_fast_and_diagnostic() {
        let coordinator = Coordinator::new();

        for attempt in 1..=5 {
            record_embedded_ble_reconnect_attempt(&coordinator.inner, "rapid_reconnect_test");
            record_embedded_ble_recovery_failure(
                &coordinator.inner,
                "BLE device connection status changed to Disconnected; transport_not_ready",
            );
            let reconnecting = coordinator.embedded_ble_wake_recovery_snapshot();

            assert_eq!(reconnecting.reconnect_attempts, attempt);
            assert_eq!(
                reconnecting.status,
                EmbeddedBleWakeRecoveryStatus::Reconnecting
            );
            assert_eq!(
                reconnecting.notify_subscription_state,
                EmbeddedBleNotifySubscriptionState::Lost
            );
            assert!(reconnecting
                .recent_disconnect_reason
                .as_deref()
                .unwrap_or_default()
                .contains("connection status changed"));
            assert_eq!(
                next_embedded_ble_background_retry_delay(
                    reconnecting.recent_disconnect_reason.as_deref().unwrap(),
                    Duration::from_secs(4),
                ),
                EMBEDDED_BLE_RETRY_FAST_DELAY
            );

            let recovered_capsule_visible = record_embedded_ble_notify_ready(&coordinator.inner);
            assert_eq!(
                recovered_capsule_visible,
                attempt == 1,
                "only the first repeated automatic recovery should show a reconnected capsule"
            );
            let ready = coordinator.embedded_ble_wake_recovery_snapshot();
            assert_eq!(ready.status, EmbeddedBleWakeRecoveryStatus::Ready);
            assert_eq!(
                ready.notify_subscription_state,
                EmbeddedBleNotifySubscriptionState::Subscribed
            );
            assert!(ready.recent_disconnect_reason.is_none());
        }
    }

    #[tokio::test]
    async fn hotkey_injection_gate_logs_pressed_and_cancels() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Info)
            .is_test(false)
            .try_init();
        let _guard = ENV_LOCK.lock().await;
        std::env::set_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN", "1");

        let coordinator = Coordinator::new();
        force_microphone_input_for_test(&coordinator);
        coordinator.inject_hotkey_click_for_dev().await.unwrap();

        assert_eq!(coordinator.inner.state.lock().phase, SessionPhase::Idle);
        std::env::remove_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN");
    }

    #[tokio::test]
    async fn begin_session_dry_run_enters_listening_and_clears_stale_edges() {
        let _guard = ENV_LOCK.lock().await;
        std::env::set_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN", "1");

        let coordinator = Coordinator::new();
        force_microphone_input_for_test(&coordinator);
        let old_session_id = coordinator.inner.state.lock().session_id;
        {
            let mut state = coordinator.inner.state.lock();
            state.pending_stop = true;
            state.cancelled = true;
        }

        coordinator.start_dictation().await.unwrap();

        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Listening);
        assert!(!state.pending_stop);
        assert!(!state.cancelled);
        assert_ne!(state.session_id, old_session_id);

        std::env::remove_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN");
    }

    #[tokio::test]
    async fn begin_session_ignores_non_idle_phase() {
        let _guard = ENV_LOCK.lock().await;
        std::env::set_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN", "1");

        let coordinator = Coordinator::new();
        force_microphone_input_for_test(&coordinator);
        let old_session_id = {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Processing;
            state.session_id = session_id(99);
            state.session_id
        };

        coordinator.start_dictation().await.unwrap();

        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Processing);
        assert_eq!(state.session_id, old_session_id);

        std::env::remove_var("LISTENER_TYPE_HOTKEY_INJECTION_DRY_RUN");
    }

    #[test]
    fn window_key_matcher_mirrors_windows_trigger_aliases() {
        let cases = [
            (HotkeyTrigger::RightControl, "Control", "ControlRight"),
            (HotkeyTrigger::LeftControl, "Control", "ControlLeft"),
            (HotkeyTrigger::RightOption, "Alt", "AltRight"),
            (HotkeyTrigger::RightAlt, "AltGraph", "AltRight"),
            (HotkeyTrigger::RightCommand, "Meta", "MetaRight"),
            (HotkeyTrigger::LeftOption, "Alt", "AltLeft"),
            // Mirrors Windows trigger_to_vk_code aliases.
            (HotkeyTrigger::Fn, "Control", "ControlRight"),
        ];
        for (trigger, key, code) in cases {
            assert!(
                window_key_matches_trigger(trigger, key, code),
                "{trigger:?} should match {key}/{code}"
            );
        }

        assert!(!window_key_matches_trigger(
            HotkeyTrigger::RightControl,
            "Control",
            "ControlLeft"
        ));
        assert!(!window_key_matches_trigger(
            HotkeyTrigger::LeftOption,
            "Alt",
            "AltRight"
        ));
        assert!(!window_key_matches_trigger(HotkeyTrigger::Fn, "Fn", "Fn"));
    }

    #[test]
    fn foundry_local_provider_is_keyless_and_not_whisper_compatible() {
        #[cfg(target_os = "windows")]
        assert!(is_keyless_local_asr_provider(
            crate::asr::local::foundry::PROVIDER_ID
        ));
        #[cfg(not(target_os = "windows"))]
        assert!(!is_keyless_local_asr_provider(
            crate::asr::local::foundry::PROVIDER_ID
        ));
        assert!(!is_whisper_compatible_provider(
            crate::asr::local::foundry::PROVIDER_ID
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn coordinator_shares_app_foundry_runtime() {
        let runtime = Arc::new(crate::asr::local::FoundryLocalRuntime::new());
        let coordinator = Coordinator::new_with_foundry_runtime(Arc::clone(&runtime));

        assert!(Arc::ptr_eq(
            &runtime,
            &coordinator.inner.foundry_local_runtime
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn foundry_transcribe_skips_global_timeout_for_first_run_provisioning() {
        let provider = Arc::new(crate::asr::local::FoundryLocalWhisperAsr::new(
            Arc::new(crate::asr::local::FoundryLocalRuntime::new()),
            crate::asr::local::foundry::DEFAULT_MODEL_ALIAS.to_string(),
            "auto".to_string(),
            None,
        ));
        let active_asr = ActiveAsr::FoundryLocalWhisper(provider);

        assert!(!asr_transcribe_uses_global_timeout(&active_asr));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn foundry_audio_transcribe_timeout_is_separate_from_prepare() {
        let timeout = foundry_audio_transcribe_timeout_duration();

        assert_eq!(
            timeout,
            std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
        );
    }

    #[test]
    fn local_qwen_timeout_floors_at_global_timeout_for_short_audio() {
        // 5s 录音：5 × 0.6 = 3, +10 = 13, max(15) = 15。短录音保留 15s 兜底。
        assert_eq!(
            local_qwen_transcribe_timeout(5.0),
            std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
        );
    }

    #[test]
    fn local_qwen_timeout_scales_with_audio_duration() {
        // 60s 录音：60 × 0.6 = 36, +10 = 46s。覆盖 RTF ≈ 0.5 的边界。
        assert_eq!(
            local_qwen_transcribe_timeout(60.0),
            std::time::Duration::from_secs(46)
        );
    }

    #[test]
    fn local_qwen_timeout_ceils_partial_seconds() {
        // 10.1s 录音：10.1 × 0.6 = 6.06, ceil = 7, +10 = 17, max(15) = 17。
        assert_eq!(
            local_qwen_transcribe_timeout(10.1),
            std::time::Duration::from_secs(17)
        );
    }

    #[test]
    fn local_qwen_timeout_handles_zero_duration() {
        // 0 时长（空 buffer 边界）：0 × 0.6 = 0, +10 = 10, max(15) = 15。
        assert_eq!(
            local_qwen_transcribe_timeout(0.0),
            std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn foundry_release_uses_foundry_keep_loaded_preference() {
        let runtime = Arc::new(crate::asr::local::FoundryLocalRuntime::new());
        let coordinator = Coordinator::new_with_foundry_runtime(runtime);
        let mut prefs = coordinator.inner.prefs.get();
        prefs.local_asr_keep_loaded_secs = 3;
        prefs.foundry_local_asr_keep_loaded_secs = 7;
        coordinator.inner.prefs.set(prefs).unwrap();

        assert_eq!(foundry_local_asr_release_keep_secs(&coordinator.inner), 7);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn foundry_release_guard_rejects_stale_session() {
        let runtime = Arc::new(crate::asr::local::FoundryLocalRuntime::new());
        let coordinator = Coordinator::new_with_foundry_runtime(runtime);
        let old_session_id = coordinator.inner.state.lock().session_id;

        assert!(foundry_release_session_is_current(
            &coordinator.inner,
            old_session_id
        ));

        coordinator.inner.state.lock().session_id = new_session_id();

        assert!(!foundry_release_session_is_current(
            &coordinator.inner,
            old_session_id
        ));
    }

    #[test]
    fn resolve_ark_endpoint_rejects_blank_key_without_custom_endpoint() {
        assert_eq!(
            resolve_ark_endpoint_with_policy("", None)
                .unwrap_err()
                .to_string(),
            "API Key 为空"
        );
    }

    #[test]
    fn resolve_ark_endpoint_allows_blank_key_with_custom_endpoint() {
        let endpoint = resolve_ark_endpoint_with_policy(
            "",
            Some("https://example.com/v1/chat/completions".to_string()),
        )
        .unwrap();
        assert_eq!(endpoint, "https://example.com/v1/chat/completions");
    }

    #[test]
    fn deferred_asr_bridge_flushes_startup_audio_before_live_chunks() {
        #[derive(Default)]
        struct RecordingConsumer {
            bytes: Mutex<Vec<u8>>,
        }

        impl crate::asr::AudioConsumer for RecordingConsumer {
            fn consume_pcm_chunk(&self, pcm: &[u8]) {
                self.bytes.lock().extend_from_slice(pcm);
            }
        }

        let bridge = DeferredAsrBridge::new();
        crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[1, 2]);
        crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[3, 4]);

        let target = Arc::new(RecordingConsumer::default());
        let target_for_attach: Arc<dyn crate::asr::AudioConsumer> = target.clone();
        assert_eq!(bridge.attach(target_for_attach), 4);

        crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[5, 6]);
        assert_eq!(&*target.bytes.lock(), &[1, 2, 3, 4, 5, 6]);
    }

    #[tokio::test]
    async fn manual_stop_during_starting_is_queued() {
        let coordinator = Coordinator::new();
        {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Starting;
            state.pending_stop = false;
        }

        coordinator.stop_dictation().await.unwrap();

        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Starting);
        assert!(state.pending_stop);
    }

    #[tokio::test]
    async fn stop_dictation_from_listening_without_asr_returns_idle() {
        let coordinator = Coordinator::new();
        {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Listening;
            state.session_id = session_id(123);
        }

        coordinator.stop_dictation().await.unwrap();

        assert_eq!(coordinator.inner.state.lock().phase, SessionPhase::Idle);
    }

    #[test]
    fn cancel_session_state_machine_is_table_driven() {
        let cases = [
            (SessionPhase::Idle, SessionPhase::Idle, false),
            (SessionPhase::Starting, SessionPhase::Idle, true),
            (SessionPhase::Listening, SessionPhase::Idle, true),
            (SessionPhase::Processing, SessionPhase::Processing, true),
            (SessionPhase::Inserting, SessionPhase::Inserting, false),
        ];

        for (initial, expected_phase, expected_cancelled) in cases {
            let coordinator = Coordinator::new();
            {
                let mut state = coordinator.inner.state.lock();
                state.phase = initial;
                state.cancelled = false;
                state.focus_target = Some(1);
            }

            coordinator.cancel_dictation();

            let state = coordinator.inner.state.lock();
            assert_eq!(state.phase, expected_phase, "initial={initial:?}");
            assert_eq!(state.cancelled, expected_cancelled, "initial={initial:?}");
            if matches!(initial, SessionPhase::Starting | SessionPhase::Listening) {
                assert!(state.focus_target.is_none(), "initial={initial:?}");
            }
        }
    }

    #[test]
    fn recorder_runtime_error_aborts_active_session() {
        let coordinator = Coordinator::new();
        {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }

        abort_recording_with_error(&coordinator.inner, "录音中断: stream failed".to_string());

        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Idle);
        assert!(state.cancelled);
        assert!(coordinator.inner.recorder.lock().is_none());
        assert!(coordinator.inner.asr.lock().is_none());
    }

    #[test]
    fn abort_recording_keeps_session_non_idle_until_restore_can_run() {
        let mut state = SessionState::default();
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
        state.session_id = session_id(7);

        let abort = begin_recording_abort_before_restore(&mut state).unwrap();

        assert_eq!(abort.session_id, session_id(7));
        assert!(state.cancelled);
        assert_eq!(state.phase, SessionPhase::Listening);

        publish_abort_idle_after_restore(&mut state, abort.session_id);

        assert_eq!(state.phase, SessionPhase::Idle);
    }

    #[tokio::test]
    async fn pressed_edge_during_inserting_does_not_start_new_session() {
        let coordinator = Coordinator::new();
        {
            let mut state = coordinator.inner.state.lock();
            state.phase = SessionPhase::Inserting;
            state.session_id = session_id(41);
        }

        handle_pressed_edge(&coordinator.inner).await;

        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Inserting);
        assert_eq!(state.session_id, session_id(41));
    }

    #[tokio::test]
    async fn repeated_pressed_edge_during_hold_session_does_not_restart() {
        let coordinator = Coordinator::new();
        coordinator
            .inner
            .prefs
            .set(crate::types::UserPreferences {
                hotkey: crate::types::HotkeyBinding {
                    trigger: HotkeyTrigger::RightControl,
                    mode: HotkeyMode::Hold,
                    keys: None,
                },
                ..Default::default()
            })
            .unwrap();
        coordinator.inner.state.lock().phase = SessionPhase::Listening;
        coordinator
            .inner
            .hotkey_trigger_held
            .store(true, Ordering::SeqCst);

        handle_pressed_edge(&coordinator.inner).await;

        assert_eq!(
            coordinator.inner.state.lock().phase,
            SessionPhase::Listening
        );
        assert!(coordinator.inner.hotkey_trigger_held.load(Ordering::SeqCst));
    }

    #[test]
    fn enabling_shortcut_recording_clears_dictation_hold_latch() {
        let coordinator = Coordinator::new();
        coordinator
            .inner
            .hotkey_trigger_held
            .store(true, Ordering::SeqCst);

        coordinator.set_shortcut_recording_active(true);

        assert!(!coordinator.inner.hotkey_trigger_held.load(Ordering::SeqCst));
    }

    #[test]
    fn window_hotkey_fallback_is_disabled_when_no_explicit_fallback_is_advertised() {
        assert_eq!(
            window_hotkey_fallback_enabled(),
            crate::types::HotkeyCapability::current().explicit_fallback_available
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn prepared_windows_ime_slot_is_taken_only_for_matching_session() {
        let mut slots = vec![PreparedWindowsImeSessionSlot {
            session_id: session_id(2),
            prepared: PreparedWindowsImeSession::unavailable(),
        }];

        assert!(take_matching_prepared_windows_ime_session(&mut slots, session_id(1)).is_none());
        assert_eq!(
            slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
            vec![session_id(2)]
        );

        assert!(take_matching_prepared_windows_ime_session(&mut slots, session_id(2)).is_some());
        assert!(slots.is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn prepared_windows_ime_sessions_keep_overlapping_snapshots() {
        let mut slots = Vec::new();
        store_prepared_windows_ime_session(
            &mut slots,
            session_id(1),
            PreparedWindowsImeSession::unavailable(),
        );
        store_prepared_windows_ime_session(
            &mut slots,
            session_id(2),
            PreparedWindowsImeSession::unavailable(),
        );

        assert_eq!(
            slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
            vec![session_id(1), session_id(2)]
        );

        assert!(take_matching_prepared_windows_ime_session(&mut slots, session_id(1)).is_some());
        assert_eq!(
            slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
            vec![session_id(2)]
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn stale_prepared_windows_ime_restore_discards_old_snapshot_without_restoring() {
        let mut slots = Vec::new();
        store_prepared_windows_ime_session(
            &mut slots,
            session_id(1),
            PreparedWindowsImeSession::unavailable(),
        );
        store_prepared_windows_ime_session(
            &mut slots,
            session_id(2),
            PreparedWindowsImeSession::unavailable(),
        );

        assert!(take_current_prepared_windows_ime_session_for_restore(
            &mut slots,
            session_id(1),
            session_id(2)
        )
        .is_none());
        assert_eq!(
            slots.iter().map(|slot| slot.session_id).collect::<Vec<_>>(),
            vec![session_id(2)]
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn non_tsf_insertion_fallback_gate_blocks_only_when_disabled() {
        assert!(should_try_non_tsf_insertion_fallback(
            true,
            InsertStatus::CopiedFallback
        ));
        assert!(should_try_non_tsf_insertion_fallback(
            true,
            InsertStatus::Failed
        ));
        assert!(!should_try_non_tsf_insertion_fallback(
            true,
            InsertStatus::Inserted
        ));
        assert!(!should_try_non_tsf_insertion_fallback(
            false,
            InsertStatus::CopiedFallback
        ));
        assert!(!should_try_non_tsf_insertion_fallback(
            false,
            InsertStatus::Failed
        ));
    }

    #[test]
    fn focus_restore_failure_uses_specific_error_code_when_insert_fails() {
        assert_eq!(
            dictation_error_code(InsertStatus::Failed, false, false, false, false),
            Some("focusRestoreFailed")
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn missing_windows_hwnd_is_not_present() {
        use windows::Win32::Foundation::HWND;

        assert!(!windows_hwnd_is_present(HWND::default()));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn tsf_required_failure_keeps_tsf_error_when_focus_was_ready() {
        assert_eq!(
            dictation_error_code(InsertStatus::Failed, false, true, false, false),
            Some("windowsImeTsfRequired")
        );
    }

    #[test]
    fn startup_race_check_treats_newer_session_as_stale() {
        let mut state = SessionState::default();
        state.phase = SessionPhase::Starting;
        state.cancelled = false;
        state.session_id = session_id(2);

        assert_eq!(
            startup_race_status(&state, session_id(1)),
            StartupRaceStatus::StaleContinuation
        );
    }

    #[test]
    fn startup_race_check_is_table_driven_for_begin_session_edges() {
        let cases = [
            (
                SessionPhase::Starting,
                false,
                session_id(7),
                StartupRaceStatus::ActiveStarting,
            ),
            (
                SessionPhase::Starting,
                true,
                session_id(7),
                StartupRaceStatus::CancelRaced,
            ),
            (
                SessionPhase::Idle,
                false,
                session_id(7),
                StartupRaceStatus::CancelRaced,
            ),
            (
                SessionPhase::Listening,
                false,
                session_id(7),
                StartupRaceStatus::CancelRaced,
            ),
            (
                SessionPhase::Starting,
                false,
                session_id(8),
                StartupRaceStatus::StaleContinuation,
            ),
        ];

        for (phase, cancelled, actual_session_id, expected) in cases {
            let mut state = SessionState::default();
            state.phase = phase;
            state.cancelled = cancelled;
            state.session_id = actual_session_id;

            assert_eq!(
                startup_race_status(&state, session_id(7)),
                expected,
                "phase={phase:?} cancelled={cancelled} actual_session={actual_session_id}"
            );
        }
    }

    #[test]
    fn begin_recording_abort_is_noop_after_prior_cancel_or_idle() {
        let cases = [
            (SessionPhase::Idle, false),
            (SessionPhase::Processing, false),
            (SessionPhase::Listening, true),
        ];

        for (phase, cancelled) in cases {
            let mut state = SessionState::default();
            state.phase = phase;
            state.cancelled = cancelled;

            assert!(begin_recording_abort_before_restore(&mut state).is_none());
            assert_eq!(state.phase, phase);
            assert_eq!(state.cancelled, cancelled);
        }
    }

    #[test]
    fn stale_startup_cleanup_keeps_newer_asr_resource() {
        let coordinator = Coordinator::new();
        let newer_asr = Arc::new(WhisperBatchASR::new(
            "key".to_string(),
            "http://localhost".to_string(),
            "model".to_string(),
            None,
        ));
        *coordinator.inner.asr.lock() = Some(SessionResource::new(
            session_id(2),
            ActiveAsr::Whisper(Arc::clone(&newer_asr)),
        ));

        discard_startup_resources_for_session(&coordinator.inner, session_id(1));

        assert_eq!(
            coordinator
                .inner
                .asr
                .lock()
                .as_ref()
                .map(|resource| resource.session_id),
            Some(session_id(2))
        );

        discard_startup_resources_for_session(&coordinator.inner, session_id(2));

        assert!(coordinator.inner.asr.lock().is_none());
    }

    #[test]
    fn capsule_recording_window_ops_are_low_frequency() {
        let mut throttle = CapsuleUiThrottleState::default();
        let start = Instant::now();
        let mut runs = 0;

        for tick in 0..300 {
            let now = start + Duration::from_millis(tick * 33);
            if throttle.should_run_window_ops(
                CapsuleWindowRequest {
                    session_id: Some("session-1".to_string()),
                    state: CapsuleState::Recording,
                    visible: true,
                    translation: false,
                    show_capsule: true,
                },
                now,
            ) {
                runs += 1;
            }
        }

        assert!(
            runs <= 10,
            "30 Hz Recording ticks over ~10s should produce <= 10 window ops, got {runs}"
        );
    }

    #[test]
    fn capsule_state_transition_bypasses_recording_window_throttle() {
        let mut throttle = CapsuleUiThrottleState::default();
        let start = Instant::now();

        assert!(throttle.should_run_window_ops(
            CapsuleWindowRequest {
                session_id: Some("session-1".to_string()),
                state: CapsuleState::Recording,
                visible: true,
                translation: false,
                show_capsule: true,
            },
            start,
        ));
        assert!(!throttle.should_run_window_ops(
            CapsuleWindowRequest {
                session_id: Some("session-1".to_string()),
                state: CapsuleState::Recording,
                visible: true,
                translation: false,
                show_capsule: true,
            },
            start + Duration::from_millis(100),
        ));
        assert!(throttle.should_run_window_ops(
            CapsuleWindowRequest {
                session_id: Some("session-1".to_string()),
                state: CapsuleState::Transcribing,
                visible: true,
                translation: false,
                show_capsule: true,
            },
            start + Duration::from_millis(100),
        ));
    }
}

fn enabled_phrases(inner: &Arc<Inner>) -> Vec<String> {
    inner
        .vocab
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.enabled)
        .map(|e| e.phrase)
        .collect()
}

/// 终止态（Done / Cancelled / Error）后延迟 N ms 把胶囊改回 Idle，让浮窗自动消失。
/// 硬件 BLE 听写的日常路径需要按键结束后立刻收起；详细结果可在历史记录里复盘。
const CAPSULE_AUTO_HIDE_DELAY_MS: u64 = 0;
const CAPSULE_ACTIONABLE_ERROR_HIDE_DELAY_MS: u64 = 6_000;
const CAPSULE_EMPTY_TRANSCRIPT_HIDE_DELAY_MS: u64 = 1_500;
const CAPSULE_STREAM_ERROR_HIDE_DELAY_MS: u64 = 6_000;
const CAPSULE_RECORDING_WINDOW_KEEPALIVE_MS: u64 = 1_000;

/// Coordinator 全局超时保护：防止 ASR await_final_result() 永远挂起。
/// 设置为 15 秒（比 ASR 的 12 秒 FINAL_RESULT_TIMEOUT 稍长），
/// 只在 ASR 超时机制失效时作为最后的防线触发。
const COORDINATOR_GLOBAL_TIMEOUT_SECS: u64 = 15;

#[cfg(target_os = "windows")]
fn foundry_audio_transcribe_timeout_duration() -> std::time::Duration {
    std::time::Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS)
}

/// 本地 Qwen3-ASR 的动态转写超时。固定 15 秒在长录音（≥ 30s）+ 慢机器
/// （RTF ≈ 0.3–0.5）上必然超时把整段内容丢掉。改用 max(15, ceil(audio_s
/// × 0.6) + 10)：基础保留 15s 兜住短录音；长录音按音频长度的 0.6 倍 +
/// 10s 余量，覆盖 RTF ≤ 0.5 的机器。
fn local_qwen_transcribe_timeout(audio_secs: f64) -> std::time::Duration {
    let secs = ((audio_secs * 0.6).ceil() as u64)
        .saturating_add(10)
        .max(COORDINATOR_GLOBAL_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

/// 检查 begin_session 的 await 间隙是否被 cancel_session 打断。
/// 必须在持有 state lock 的瞬间读，结果一拿就过期，所以用 helper 名字提醒只在
/// 「准备做下一步副作用前」用。
fn startup_race_status_for_starting(
    inner: &Arc<Inner>,
    captured_session_id: SessionId,
) -> StartupRaceStatus {
    let state = inner.state.lock();
    startup_race_status(&state, captured_session_id)
}

fn set_phase_idle_if_session_matches(inner: &Arc<Inner>, session_id: SessionId) {
    let mut state = inner.state.lock();
    if state.session_id == session_id {
        state.phase = SessionPhase::Idle;
    }
}

fn listening_session_has_no_current_asr(inner: &Arc<Inner>) -> bool {
    let (phase, session_id) = {
        let state = inner.state.lock();
        (state.phase, state.session_id)
    };
    phase == SessionPhase::Listening
        && inner
            .asr
            .lock()
            .as_ref()
            .map(|resource| resource.session_id != session_id)
            .unwrap_or(true)
}

fn schedule_capsule_idle(inner: &Arc<Inner>, delay_ms: u64, session_id: Option<SessionId>) {
    let inner_clone = Arc::clone(inner);
    async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        // 必须 dictation **和** QA 同时空闲才能隐藏胶囊。否则旧 dictation Done timer
        // 的尾巴会在新 QA 录音/思考中把胶囊意外收掉（issue #118 v2 复现）。
        let dictation_idle = inner_clone.state.lock().phase == SessionPhase::Idle;
        let qa_idle = inner_clone.qa_state.lock().phase == QaPhase::Idle;
        if dictation_idle && qa_idle {
            emit_capsule_with_session(
                &inner_clone,
                session_id,
                CapsuleState::Idle,
                0.0,
                0,
                None,
                None,
            );
        }
    });
}

#[cfg(target_os = "windows")]
fn capture_focus_target() -> Option<usize> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        None
    } else {
        Some(foreground.0 as usize)
    }
}

#[cfg(not(target_os = "windows"))]
fn capture_focus_target() -> Option<usize> {
    None
}

/// 捕获用户开始 dictation 时的前台 app 标签（"localizedName (bundle.id)"），用作 LLM
/// polish/translate 的上下文前提，让模型按 app 调风格。详见 issue #116。
///
/// macOS 走 NSWorkspace.frontmostApplication（公开 API，无需额外权限）；
/// Windows 复用前台 HWND 拿窗口标题；Linux/其他平台返回 None。
#[cfg(target_os = "macos")]
fn capture_frontmost_app() -> Option<String> {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    unsafe {
        let cls = AnyClass::get("NSWorkspace")?;
        let workspace: *mut AnyObject = msg_send![cls, sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let name_obj: *mut AnyObject = msg_send![app, localizedName];
        let bundle_obj: *mut AnyObject = msg_send![app, bundleIdentifier];
        let name = nsstring_to_string(name_obj);
        let bundle = nsstring_to_string(bundle_obj);
        match (name, bundle) {
            (Some(n), Some(b)) => Some(format!("{n} ({b})")),
            (Some(n), None) => Some(n),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn nsstring_to_string(ns_string: *mut objc2::runtime::AnyObject) -> Option<String> {
    use objc2::msg_send;
    if ns_string.is_null() {
        return None;
    }
    let utf8: *const std::os::raw::c_char = unsafe { msg_send![ns_string, UTF8String] };
    if utf8.is_null() {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(utf8) };
    let s = cstr.to_string_lossy().into_owned();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(target_os = "windows")]
fn capture_frontmost_app() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let copied = GetWindowTextW(hwnd, &mut buf);
        if copied <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..copied as usize]);
        if title.is_empty() {
            None
        } else {
            Some(title)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn capture_frontmost_app() -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn restore_focus_target_if_possible(target: Option<usize>) -> bool {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let Some(raw_target) = target else {
        log::warn!("[coord] no original Windows insertion target captured");
        return false;
    };
    let hwnd = HWND(raw_target as *mut c_void);
    if hwnd.0.is_null() {
        return false;
    }
    if !unsafe { IsWindow(hwnd).as_bool() } {
        log::warn!("[coord] original Windows insertion target is no longer a valid window");
        return false;
    }

    let foreground = unsafe { GetForegroundWindow() };
    if foreground == hwnd {
        return true;
    }

    if unsafe { IsIconic(hwnd).as_bool() } {
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }
    let _ = unsafe { SetForegroundWindow(hwnd) };
    std::thread::sleep(std::time::Duration::from_millis(60));

    let foreground = unsafe { GetForegroundWindow() };
    if foreground != hwnd {
        log::warn!("[coord] failed to restore original Windows insertion target before paste");
        return false;
    }
    true
}

#[cfg(not(target_os = "windows"))]
fn restore_focus_target_if_possible(_target: Option<usize>) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn windows_hwnd_is_present(hwnd: windows::Win32::Foundation::HWND) -> bool {
    hwnd != windows::Win32::Foundation::HWND::default()
}

#[cfg(target_os = "windows")]
fn capture_ime_submit_target() -> Option<ImeSubmitTarget> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };

    let foreground = unsafe { GetForegroundWindow() };
    if !windows_hwnd_is_present(foreground) {
        return None;
    }

    let mut foreground_process_id = 0;
    let foreground_thread_id =
        unsafe { GetWindowThreadProcessId(foreground, Some(&mut foreground_process_id)) };
    if foreground_thread_id == 0 {
        return None;
    }

    let mut gui_info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let target_window = if unsafe { GetGUIThreadInfo(foreground_thread_id, &mut gui_info).is_ok() }
        && windows_hwnd_is_present(gui_info.hwndFocus)
    {
        gui_info.hwndFocus
    } else {
        foreground
    };

    let mut process_id = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(target_window, Some(&mut process_id)) };
    if process_id == 0 || thread_id == 0 {
        return None;
    }

    Some(ImeSubmitTarget {
        process_id,
        thread_id,
    })
}

#[cfg(target_os = "windows")]
fn show_capsule_window_no_activate<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
    };

    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);

    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

// macOS / Linux 上不走 no-activate 路径：胶囊由 emit_capsule 的 fallback
// `window.show()` 直接显示，再用 restore_main_window_key_if_active 把焦点还给
// 主窗口。这是 1.2.11 的实现 — 单独走 orderFrontRegardless 会让胶囊在 webview
// 未完整初始化时偶发不可见。
#[cfg(not(target_os = "windows"))]
fn show_capsule_window_no_activate<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    _window: &tauri::WebviewWindow<R>,
) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn hide_capsule_window_if_present() {
    use std::iter::once;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, ShowWindow, HWND_NOTOPMOST, SWP_HIDEWINDOW, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    };

    let title: Vec<u16> = "Listener Type Capsule"
        .encode_utf16()
        .chain(once(0))
        .collect();
    let hwnd = match unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) } {
        Ok(hwnd) => hwnd,
        Err(_) => return,
    };
    if hwnd == HWND::default() || hwnd.0.is_null() {
        return;
    }

    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_HIDEWINDOW,
        )
    };
}

#[cfg(not(target_os = "windows"))]
fn hide_capsule_window_if_present() {}

fn emit_capsule(
    inner: &Arc<Inner>,
    state: CapsuleState,
    level: f32,
    elapsed_ms: u64,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    emit_capsule_with_session(
        inner,
        None,
        state,
        level,
        elapsed_ms,
        message,
        inserted_chars,
    );
}

fn emit_capsule_for_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
    state: CapsuleState,
    level: f32,
    elapsed_ms: u64,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    emit_capsule_with_session(
        inner,
        Some(session_id),
        state,
        level,
        elapsed_ms,
        message,
        inserted_chars,
    );
}

fn capsule_state_from_dictation(ui_state: DictationUiState) -> CapsuleState {
    match ui_state {
        DictationUiState::Recording => CapsuleState::Recording,
        DictationUiState::Transcribing => CapsuleState::Transcribing,
        DictationUiState::Polishing => CapsuleState::Polishing,
        DictationUiState::Done => CapsuleState::Done,
        DictationUiState::Cancelled => CapsuleState::Cancelled,
        DictationUiState::Error => CapsuleState::Error,
        DictationUiState::Idle => CapsuleState::Idle,
    }
}

fn emit_dictation_snapshot(
    inner: &Arc<Inner>,
    snapshot: DictationSnapshot,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    emit_capsule_for_session(
        inner,
        snapshot.session_id,
        capsule_state_from_dictation(snapshot.state),
        level,
        snapshot.elapsed_ms,
        message,
        inserted_chars,
    );
}

fn publish_dictation_transition(
    inner: &Arc<Inner>,
    transition: DictationTransition,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    if let Some(snapshot) = transition.snapshot() {
        emit_dictation_snapshot(inner, snapshot, level, message, inserted_chars);
        true
    } else {
        false
    }
}

fn publish_dictation_capsule(
    inner: &Arc<Inner>,
    session_id: SessionId,
    ui_state: DictationUiState,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    let snapshot = {
        let state = inner.state.lock();
        publishable_dictation_snapshot(&state, session_id, ui_state).ok()
    };
    let Some(snapshot) = snapshot else {
        return false;
    };
    emit_dictation_snapshot(inner, snapshot, level, message, inserted_chars);
    true
}

fn emit_capsule_with_session(
    inner: &Arc<Inner>,
    event_session_id: Option<SessionId>,
    state: CapsuleState,
    level: f32,
    elapsed_ms: u64,
    message: Option<String>,
    inserted_chars: Option<u32>,
) {
    let app_opt = inner.app.lock().clone();
    let Some(app) = app_opt else { return };
    let seq = inner.capsule_sequence.fetch_add(1, Ordering::SeqCst) + 1;
    let session_id = event_session_id.map(|id| id.to_string());
    let translation = inner.translation_modifier_seen.load(Ordering::SeqCst);
    let payload = CapsulePayload {
        seq,
        session_id,
        state,
        level,
        elapsed_ms,
        message,
        inserted_chars,
        translation,
    };

    let visible = !matches!(state, CapsuleState::Idle);
    let show_capsule = inner.prefs.get().show_capsule;
    crate::capsule_log::record_backend_emit(&payload, visible, show_capsule);
    let should_trace_emit = !matches!(state, CapsuleState::Recording)
        || elapsed_ms == 0
        || payload.message.is_some()
        || elapsed_ms % 500 == 0;
    let session_id_for_log = payload
        .session_id
        .clone()
        .unwrap_or_else(|| "-".to_string());
    if should_trace_emit {
        crate::timeline::mark(
            "backend.capsule",
            "emit_request",
            format!(
                "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms} level={level:.3} visible={visible} message={}",
                payload.message.as_deref().unwrap_or("-")
            ),
        );
    }

    let run_window_ops = {
        let mut throttle = inner.capsule_ui_throttle.lock();
        throttle.should_run_window_ops(
            CapsuleWindowRequest {
                session_id: payload.session_id.clone(),
                state,
                visible,
                translation,
                show_capsule,
            },
            Instant::now(),
        )
    };

    if run_window_ops {
        let inner_for_main = Arc::clone(inner);
        let app_for_main = app.clone();
        let session_id_for_main = session_id_for_log.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(window) = app_for_main.get_webview_window("capsule") else {
                log::warn!("[capsule] emit requested but capsule window is missing");
                crate::timeline::mark(
                    "backend.capsule",
                    "missing_window",
                    format!("seq={seq} session_id={session_id_for_main} state={state:?} elapsed_ms={elapsed_ms}"),
                );
                return;
            };
            crate::prepare_capsule_window_for_overlay(&window);
            maybe_position_capsule_bottom_center(&inner_for_main, &window, translation);
            if show_capsule && visible {
                let shown_no_activate = show_capsule_window_no_activate(&app_for_main, &window);
                crate::timeline::mark(
                    "backend.capsule",
                    "show_request",
                    format!(
                        "seq={seq} session_id={session_id_for_main} state={state:?} elapsed_ms={elapsed_ms} shown_no_activate={shown_no_activate}"
                    ),
                );
                log::info!(
                    "[capsule] show request state={state:?} shown_no_activate={shown_no_activate}"
                );
                if !shown_no_activate {
                    #[cfg(target_os = "windows")]
                    log::warn!("[capsule] no-activate show failed; skipped activating fallback");
                    #[cfg(not(target_os = "windows"))]
                    let _ = window.show();
                }
                #[cfg(target_os = "macos")]
                crate::restore_main_window_key_if_active(&app_for_main);
            } else {
                crate::timeline::mark(
                    "backend.capsule",
                    "hide_request",
                    format!(
                        "seq={seq} session_id={session_id_for_main} state={state:?} elapsed_ms={elapsed_ms} show_capsule={show_capsule} visible={visible}"
                    ),
                );
                log::info!(
                    "[capsule] hide request state={state:?} show_capsule={show_capsule} visible={visible}"
                );
                hide_capsule_window_if_present();
                let _ = window.hide();
            }
        });
    }

    if should_trace_emit {
        crate::timeline::mark(
            "backend.capsule",
            "emit_to_frontend",
            format!(
                "seq={seq} session_id={session_id_for_log} state={state:?} elapsed_ms={elapsed_ms}"
            ),
        );
    }
    let _ = app.emit_to("capsule", "capsule:state", payload);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapsuleWindowRequest {
    session_id: Option<String>,
    state: CapsuleState,
    visible: bool,
    translation: bool,
    show_capsule: bool,
}

impl CapsuleWindowRequest {
    fn visible_recording(&self) -> bool {
        self.visible && self.show_capsule && matches!(self.state, CapsuleState::Recording)
    }
}

#[derive(Debug, Default)]
struct CapsuleUiThrottleState {
    last_request: Option<CapsuleWindowRequest>,
    last_recording_keepalive_at: Option<Instant>,
}

impl CapsuleUiThrottleState {
    fn should_run_window_ops(&mut self, request: CapsuleWindowRequest, now: Instant) -> bool {
        let visible_recording = request.visible_recording();
        if self.last_request.as_ref() != Some(&request) {
            self.last_request = Some(request);
            self.last_recording_keepalive_at = visible_recording.then_some(now);
            return true;
        }

        if visible_recording {
            let due = self
                .last_recording_keepalive_at
                .map(|last| {
                    now.duration_since(last)
                        >= Duration::from_millis(CAPSULE_RECORDING_WINDOW_KEEPALIVE_MS)
                })
                .unwrap_or(true);
            if due {
                self.last_recording_keepalive_at = Some(now);
                return true;
            }
        }

        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CapsuleLayoutState {
    translation_active: bool,
    monitor_x: i32,
    monitor_y: i32,
    monitor_width: u32,
    monitor_height: u32,
    scale_bits: u64,
}

fn maybe_position_capsule_bottom_center<R: tauri::Runtime>(
    inner: &Arc<Inner>,
    window: &tauri::WebviewWindow<R>,
    translation_active: bool,
) {
    let Some(monitor) = window.current_monitor().ok().flatten() else {
        return;
    };
    let next = CapsuleLayoutState {
        translation_active,
        monitor_x: monitor.position().x,
        monitor_y: monitor.position().y,
        monitor_width: monitor.size().width,
        monitor_height: monitor.size().height,
        scale_bits: monitor.scale_factor().to_bits(),
    };
    {
        let last = inner.capsule_layout.lock();
        if last.as_ref() == Some(&next) {
            return;
        }
    }
    if crate::position_capsule_bottom_center(window, translation_active).is_ok() {
        let mut last = inner.capsule_layout.lock();
        *last = Some(next);
    }
}

// ─────────────────────────── audio bridge ───────────────────────────

struct DeferredAsrBridge {
    state: Mutex<DeferredAsrState>,
}

struct DeferredAsrState {
    target: Option<Arc<dyn crate::asr::AudioConsumer>>,
    pending_audio: Vec<u8>,
    attaching: bool,
}

impl DeferredAsrBridge {
    fn new() -> Self {
        Self {
            state: Mutex::new(DeferredAsrState {
                target: None,
                pending_audio: Vec::new(),
                attaching: false,
            }),
        }
    }

    fn attach(&self, target: Arc<dyn crate::asr::AudioConsumer>) -> usize {
        let mut flushed_bytes = 0;
        {
            let mut state = self.state.lock();
            state.attaching = true;
        }

        loop {
            let pending = {
                let mut state = self.state.lock();
                if state.pending_audio.is_empty() {
                    state.target = Some(Arc::clone(&target));
                    state.attaching = false;
                    return flushed_bytes;
                }
                std::mem::take(&mut state.pending_audio)
            };
            flushed_bytes += pending.len();
            target.consume_pcm_chunk(&pending);
        }
    }
}

impl crate::recorder::AudioConsumer for DeferredAsrBridge {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        let target = {
            let mut state = self.state.lock();
            if state.attaching {
                state.pending_audio.extend_from_slice(pcm);
                return;
            }
            if let Some(target) = state.target.as_ref() {
                Some(Arc::clone(target))
            } else {
                state.pending_audio.extend_from_slice(pcm);
                None
            }
        };

        if let Some(target) = target {
            target.consume_pcm_chunk(pcm);
        }
    }
}
