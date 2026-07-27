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
    OpenAICompatibleConfig, OpenAICompatibleLLMProvider, ProviderProxyConfig, CODEX_DEFAULT_MODEL,
    CODEX_OAUTH_PROVIDER_ID,
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
mod resources;
mod support;

const EMBEDDED_BLE_RETRY_FAST_DELAY: Duration = Duration::from_millis(500);
const EMBEDDED_BLE_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
const EMBEDDED_BLE_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);
const EMBEDDED_BLE_RETRY_LONG_DELAY: Duration = Duration::from_secs(3);
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
    begin_session, cancel_session, current_embedded_audio_partial_preview, end_session,
    handle_pressed, handle_pressed_edge, handle_released_edge, hidden_automatic_candidate_active,
    request_embedded_audio_stop_feedback, request_embedded_ble_recording_stop_from_host,
    request_hidden_automatic_candidate_promotion, request_stop_during_starting,
    submit_embedded_audio_ble_once, submit_embedded_audio_ble_stream,
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
    capture_focus_target, capture_frontmost_app, emit_capsule, emit_capsule_for_session,
    emit_capsule_with_session, enabled_phrases, listening_session_has_no_current_asr,
    local_qwen_transcribe_timeout, publish_dictation_capsule, publish_dictation_transition,
    restore_focus_target_if_possible, schedule_capsule_idle, set_phase_idle_if_session_matches,
    startup_race_status_for_starting, CAPSULE_ACTIONABLE_ERROR_HIDE_DELAY_MS,
    CAPSULE_AUTO_HIDE_DELAY_MS, CAPSULE_EMPTY_TRANSCRIPT_HIDE_DELAY_MS,
    CAPSULE_STREAM_ERROR_HIDE_DELAY_MS, CAPSULE_SUCCESS_HIDE_DELAY_MS,
    COORDINATOR_GLOBAL_TIMEOUT_SECS,
};
#[cfg(target_os = "windows")]
#[allow(unused_imports)]
use support::{
    capture_ime_submit_target, foundry_audio_transcribe_timeout_duration, windows_hwnd_is_present,
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
    /// 当前嵌入式 BLE 音频会话的 ASR 最终文本，用于 headless A2 验收。
    embedded_audio_final_result:
        Mutex<Option<crate::embedded_audio::EmbeddedAudioTranscriptResult>>,
    /// 嵌入式 BLE 流式 ASR 的最近一次 partial preview。只用于胶囊视觉反馈；
    /// 光标仍只在 final text 完成后写入。
    embedded_audio_partial_preview: Mutex<Option<String>>,
    /// 自动唤醒会话的激活词。只用于从该会话的预览和最终文本中移除激活词；
    /// 手动录音没有这个标记，因此保留相同文字。
    embedded_audio_wake_phrase_filter: Mutex<Option<(SessionId, String)>>,
    /// 最近一次用于录音胶囊的嵌入式 BLE PCM 电平。ASR partial preview 到达时沿用它，
    /// 避免文字刷新把音量动画刷成 0。
    embedded_audio_last_capsule_level: Mutex<f32>,
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

#[cfg(target_os = "windows")]
pub(super) struct WindowsInsertionResult {
    pub(super) status: InsertStatus,
    /// Only a successful TSF submit confirms that the original target accepted the text.
    pub(super) target_confirmed: bool,
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

#[derive(Debug, Default)]
struct EmbeddedBleSessionActorState {
    next_seq: u64,
    history: VecDeque<EmbeddedBleSessionActorRecord>,
    pcm_capsule_trace: EmbeddedBlePcmCapsuleTraceState,
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
                    asr: Mutex::new(None),
                    recorder: Mutex::new(None),
                    audio_archive_active: AtomicBool::new(false),
                    embedded_audio_stats: Mutex::new(None),
                    embedded_audio_final_result: Mutex::new(None),
                    embedded_audio_partial_preview: Mutex::new(None),
                    embedded_audio_wake_phrase_filter: Mutex::new(None),
                    embedded_audio_last_capsule_level: Mutex::new(0.0),
                    embedded_audio_stop_feedback_latched: AtomicBool::new(false),
                    embedded_ble_listener_generation: AtomicU64::new(0),
                    embedded_ble_ota_active: AtomicBool::new(false),
                    embedded_ble_listener_cancel: Mutex::new(None),
                    embedded_ble_listener_leave_cccd_enabled_on_cancel: Mutex::new(None),
                    embedded_ble_listener_ready: AtomicBool::new(false),
                    embedded_ble_listener_ready_notification: Notify::new(),
                    embedded_ble_device_key_wake_generation: AtomicU64::new(0),
                    embedded_ble_ota_recovery_generation: AtomicU64::new(0),
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
                asr: Mutex::new(None),
                recorder: Mutex::new(None),
                audio_archive_active: AtomicBool::new(false),
                embedded_audio_stats: Mutex::new(None),
                embedded_audio_final_result: Mutex::new(None),
                embedded_audio_partial_preview: Mutex::new(None),
                embedded_audio_wake_phrase_filter: Mutex::new(None),
                embedded_audio_last_capsule_level: Mutex::new(0.0),
                embedded_audio_stop_feedback_latched: AtomicBool::new(false),
                embedded_ble_listener_generation: AtomicU64::new(0),
                embedded_ble_ota_active: AtomicBool::new(false),
                embedded_ble_listener_cancel: Mutex::new(None),
                embedded_ble_listener_leave_cccd_enabled_on_cancel: Mutex::new(None),
                embedded_ble_listener_ready: AtomicBool::new(false),
                embedded_ble_listener_ready_notification: Notify::new(),
                embedded_ble_device_key_wake_generation: AtomicU64::new(0),
                embedded_ble_ota_recovery_generation: AtomicU64::new(0),
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
                let wake_confirmation = crate::speaker_verification::is_enrolled();
                if !active_foundry && !wake_confirmation {
                    return;
                }
                if wake_confirmation {
                    let helper_result = tauri::async_runtime::spawn_blocking(
                        crate::asr::local::wake_helper::preload,
                    )
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
                "[embedded-ble] startup BLE name/power sync running for existing embedded BLE source user_overridden={}",
                prefs.dictation_input_source_user_overridden
            );
            async_runtime::spawn_blocking(move || {
                sync_device_ble_name_from_firmware_settings(
                    &inner,
                    "startup_embedded_ble_power_probe",
                );
                refresh_embedded_ble_listener(&inner);
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
            EmbeddedBleForegroundProbeMode::RefreshBackgroundListener => {
                log::info!("[embedded-ble] foreground BLE path probe refreshing active background listener");
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

    pub fn pause_embedded_ble_listener_for_ota(&self) {
        pause_embedded_ble_listener_capture(&self.inner, "firmware OTA transfer");
    }

    pub fn try_begin_firmware_ota_transfer(&self) -> bool {
        self.inner
            .embedded_ble_ota_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn begin_firmware_ota_transfer(&self) {
        let _ = self.try_begin_firmware_ota_transfer();
    }

    pub fn firmware_ota_transfer_active(&self) -> bool {
        self.inner.embedded_ble_ota_active.load(Ordering::SeqCst)
    }

    pub fn end_firmware_ota_transfer(&self) {
        self.inner
            .embedded_ble_ota_active
            .store(false, Ordering::SeqCst);
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
        wait_for_embedded_ble_listener_ready(&self.inner, timeout).await?;
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
                    Some("打开应用失败".to_string()),
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
                Some(format!("{label} 发送失败")),
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
    let wake_started_at = (!embedded_ble_listener_capture_ready(&inner)).then(Instant::now);
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
        let mut queue_pending_start_on_wait_failure = false;
        if !embedded_ble_listener_capture_ready(&inner) {
            if matches!(
                device_key_ble_recording_control_decision(&inner),
                DeviceKeyBleRecordingControlDecision::Start
            ) {
                queue_pending_start_on_wait_failure = true;
            }
            record_embedded_ble_reconnect_attempt(&inner, "device_key_recording_control");
            refresh_embedded_ble_listener_for_device_key_wake(&inner);
            emit_capsule(
                &inner,
                CapsuleState::Reconnecting,
                0.0,
                0,
                Some("正在恢复 Listener 音频".to_string()),
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
                    "key={} gesture={} queue_pending_start={} error={error}",
                    key.label(),
                    gesture.label(),
                    queue_pending_start_on_wait_failure
                ),
            );
            if queue_pending_start_on_wait_failure {
                queue_pending_device_key_ble_start(&inner, key, gesture, "listener_not_ready");
                emit_capsule(
                    &inner,
                    CapsuleState::Reconnecting,
                    0.0,
                    0,
                    Some("Listener 恢复后自动录音".to_string()),
                    None,
                );
                return;
            }
            emit_capsule(
                &inner,
                CapsuleState::Error,
                0.0,
                0,
                Some("Listener 音频通道未恢复".to_string()),
                None,
            );
            schedule_capsule_idle(&inner, 6000, None);
            return;
        }
        if let Some(started_at) = wake_started_at {
            let elapsed_ms = started_at.elapsed().as_millis();
            let target_ms = EMBEDDED_BLE_IDLE_AUDIO_WAKE_TARGET.as_millis();
            crate::timeline::mark(
                "backend.device_key",
                "idle_audio_notify_ready",
                format!(
                    "key={} gesture={} elapsed_ms={elapsed_ms} target_ms={target_ms} met={}",
                    key.label(),
                    gesture.label(),
                    elapsed_ms <= target_ms,
                ),
            );
            log::info!(
                "[embedded-ble] device-key Idle audio notify ready elapsed_ms={elapsed_ms} target_ms={target_ms} met={}",
                elapsed_ms <= target_ms,
            );
        }

        let control_decision = device_key_ble_recording_control_decision(&inner);
        let promote_hidden_candidate = should_promote_hidden_automatic_candidate(
            control_decision,
            hidden_automatic_candidate_active(),
        );
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
        let waiting_message = if promote_hidden_candidate {
            "正在接管当前录音..."
        } else if control_session.is_some() {
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
        let waiting_message = if stop_feedback_requested {
            current_embedded_audio_partial_preview(&inner)
                .unwrap_or_else(|| waiting_message.to_string())
        } else {
            waiting_message.to_string()
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
            waiting_message,
        );
        let send_stop_control = matches!(
            control_decision,
            DeviceKeyBleRecordingControlDecision::Stop {
                phase: SessionPhase::Listening,
                ..
            }
        );
        let result = async_runtime::spawn_blocking(move || {
            if promote_hidden_candidate {
                crate::embedded_ble::send_recording_control_activate(
                    EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                )
            } else if send_stop_control {
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
                let promotion_requested = if promote_hidden_candidate {
                    request_hidden_automatic_candidate_promotion()
                } else {
                    false
                };
                clear_embedded_ble_listener_last_error(&inner);
                clear_pending_device_key_ble_start(&inner, key, gesture, "control_sent");
                crate::timeline::mark(
                    "backend.device_key",
                    "ble_recording_control_sent",
                    format!(
                        "key={} gesture={} hidden_candidate_promotion={promotion_requested}",
                        key.label(),
                        gesture.label()
                    ),
                );
                if promote_hidden_candidate {
                    if promotion_requested {
                        log::info!(
                            "[device-key] {} {} promoted the active hidden automatic candidate",
                            key.label(),
                            gesture.label()
                        );
                    } else {
                        log::warn!(
                            "[device-key] {} {} activation arrived after the hidden automatic candidate had already resolved",
                            key.label(),
                            gesture.label()
                        );
                    }
                }
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
                                Some("Listener 恢复后自动录音".to_string()),
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
                                "Listener 恢复后自动停止录音".to_string(),
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
                    "Listener 录音控制失败".to_string(),
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

fn should_promote_hidden_automatic_candidate(
    decision: DeviceKeyBleRecordingControlDecision,
    hidden_candidate_active: bool,
) -> bool {
    hidden_candidate_active && matches!(decision, DeviceKeyBleRecordingControlDecision::Start)
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
    schedule_pending_device_key_ble_action_expiry(inner, action);
}

fn take_expired_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    expected: PendingDeviceKeyBleAction,
    reason: &'static str,
) -> Option<PendingDeviceKeyBleAction> {
    let expired = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        if slot.as_ref().is_some_and(|action| {
            *action == expected && !pending_device_key_ble_action_is_fresh(*action, Instant::now())
        }) {
            slot.take()
        } else {
            None
        }
    };
    let Some(action) = expired else {
        return None;
    };
    let age_ms = pending_device_key_ble_action_age(action, Instant::now()).as_millis();
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
    log::warn!(
        "[device-key] pending BLE recording {} reached its terminal TTL key={} gesture={} reason={reason} age_ms={age_ms}",
        action.kind.label(),
        action.key.label(),
        action.gesture.label()
    );
    Some(action)
}

fn schedule_pending_device_key_ble_action_expiry(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
) {
    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        tokio::time::sleep(DEVICE_KEY_BLE_PENDING_ACTION_TTL + Duration::from_millis(1)).await;
        let Some(expired) =
            take_expired_pending_device_key_ble_action(&inner, action, "terminal_ttl")
        else {
            return;
        };
        if expired.kind == PendingDeviceKeyBleActionKind::Start
            && inner.state.lock().phase == SessionPhase::Idle
        {
            let idle_session = emit_device_key_recording_control_capsule(
                &inner,
                None,
                DictationUiState::Error,
                CapsuleState::Error,
                "Listener 音频通道未恢复".to_string(),
            );
            schedule_capsule_idle(&inner, 6000, idle_session);
        }
    });
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
                    "Listener 恢复后自动停止录音".to_string(),
                );
                return;
            }
            let idle_session = emit_device_key_recording_control_capsule(
                &inner,
                control_session,
                DictationUiState::Error,
                CapsuleState::Error,
                "Listener 录音控制失败".to_string(),
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

async fn request_embedded_ble_recording_start_from_host(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Result<SessionId, String> {
    if !embedded_ble_listener_capture_ready(inner) {
        return Err("Listener BLE audio channel is not ready".to_string());
    }

    let session_id = {
        let mut state = inner.state.lock();
        match state.phase {
            SessionPhase::Idle => begin_session_state(&mut state, None, capture_frontmost_app())
                .ok_or_else(|| "Listener BLE recording start ignored while idle".to_string())?,
            SessionPhase::Starting | SessionPhase::Listening => state.session_id,
            phase => {
                return Err(format!(
                    "Listener BLE recording start ignored while dictation phase is {phase:?}"
                ));
            }
        }
    };

    record_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::StartCommand,
        Some(session_id),
        format!("host start requested reason={reason}"),
    );
    emit_capsule_for_session(
        inner,
        session_id,
        CapsuleState::Recording,
        0.0,
        0,
        Some("Listener 录音已启动，正在接收音频...".to_string()),
        None,
    );

    #[cfg(test)]
    {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            "firmware_start_skipped_test",
            format!("session_id={session_id} reason={reason}"),
        );
        return Ok(session_id);
    }

    #[cfg(not(test))]
    {
        let result = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::send_recording_control_toggle(
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
                    "firmware_start_sent",
                    format!("session_id={session_id} reason={reason}"),
                );
                log::info!(
                    "[coord] embedded BLE firmware start sent session_id={session_id} reason={reason}"
                );
                Ok(session_id)
            }
            Err(err) => {
                set_phase_idle_if_session_matches(inner, session_id);
                record_embedded_ble_listener_last_error(inner, &err);
                record_embedded_ble_recovery_failure(inner, &err);
                refresh_embedded_ble_listener(inner);
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some("Listener 录音启动失败".to_string()),
                    None,
                );
                schedule_capsule_idle(inner, 6000, Some(session_id));
                Err(err)
            }
        }
    }
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

fn embedded_ble_recovery_error_still_current(
    inner: &Arc<Inner>,
    err: &str,
    stage: &'static str,
) -> bool {
    if inner.embedded_ble_listener_ready.load(Ordering::SeqCst) {
        log::info!(
            "[embedded-ble] background stale pairing cleanup aborted because notify is already ready stage={stage}"
        );
        return false;
    }
    let last_error = embedded_ble_listener_last_error(inner);
    if last_error.as_deref() != Some(err) {
        log::info!(
            "[embedded-ble] background stale pairing cleanup aborted because recovery error is stale stage={stage} current_error={}",
            last_error
                .as_deref()
                .map(embedded_ble_log_preview)
                .unwrap_or_else(|| "none".to_string())
        );
        return false;
    }
    true
}

fn record_embedded_ble_session_actor_command(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
) -> u64 {
    dispatch_embedded_ble_session_actor_command_with_trace(
        inner,
        command,
        session_id,
        detail,
        true,
        |seq| seq,
    )
}

fn dispatch_embedded_ble_session_actor_command<T>(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
    handle: impl FnOnce(u64) -> T,
) -> T {
    dispatch_embedded_ble_session_actor_command_with_trace(
        inner, command, session_id, detail, true, handle,
    )
}

fn dispatch_embedded_ble_session_actor_command_with_trace<T>(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: Option<SessionId>,
    detail: impl Into<String>,
    trace_timeline: bool,
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
    if trace_timeline {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            command.as_str(),
            format!("seq={seq} session_id={session_id:?} {detail}"),
        );
    }
    result
}

fn should_trace_embedded_ble_pcm_capsule(
    inner: &Arc<Inner>,
    session_id: SessionId,
    after_stop: bool,
) -> bool {
    let mut actor = inner.embedded_ble_session_actor.lock();
    actor
        .pcm_capsule_trace
        .should_trace(session_id, after_stop, Instant::now())
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

fn hold_embedded_ble_listener_for_pairing_confirmation(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> u64 {
    hold_embedded_ble_listener_for_pairing_confirmation_for(
        inner,
        reason,
        EMBEDDED_BLE_PAIRING_CONFIRMATION_HOLD,
    )
}

fn hold_embedded_ble_listener_for_pairing_confirmation_for(
    inner: &Arc<Inner>,
    reason: &'static str,
    duration: Duration,
) -> u64 {
    clear_embedded_ble_passive_local_reattach(inner, reason);
    let generation = inner
        .embedded_ble_pairing_hold_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let until = Instant::now() + duration;
    {
        let mut hold = inner.embedded_ble_pairing_hold_until.lock();
        *hold = Some(until);
    }
    pause_embedded_ble_listener_capture(inner, reason);
    log::info!(
        "[embedded-ble] background listener held for Windows pairing confirmation reason={reason} hold_generation={generation} hold_ms={}",
        duration.as_millis()
    );
    generation
}

fn clear_embedded_ble_pairing_confirmation_hold(inner: &Arc<Inner>, reason: &'static str) {
    let had_hold = inner
        .embedded_ble_pairing_hold_until
        .lock()
        .take()
        .is_some();
    if had_hold {
        inner
            .embedded_ble_pairing_hold_generation
            .fetch_add(1, Ordering::SeqCst);
        log::info!("[embedded-ble] Windows pairing confirmation hold cleared reason={reason}");
    }
}

fn clear_embedded_ble_passive_local_reattach(inner: &Arc<Inner>, reason: &'static str) {
    if inner
        .embedded_ble_passive_local_reattach_active
        .swap(false, Ordering::SeqCst)
    {
        log::info!("[embedded-ble] passive local Windows reattach monitor cleared reason={reason}");
    }
}

fn embedded_ble_pairing_confirmation_hold_remaining(
    inner: &Arc<Inner>,
    now: Instant,
) -> Option<Duration> {
    let mut hold = inner.embedded_ble_pairing_hold_until.lock();
    match *hold {
        Some(until) if until > now => Some(until.saturating_duration_since(now)),
        Some(_) => {
            *hold = None;
            inner
                .embedded_ble_pairing_hold_generation
                .fetch_add(1, Ordering::SeqCst);
            log::info!("[embedded-ble] Windows pairing confirmation hold expired");
            None
        }
        None => None,
    }
}

fn embedded_ble_pairing_prompt_ready(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::Paired
            | crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
    ) && !pairing.open_bluetooth_settings
        && pairing.failed_devices == 0
}

fn embedded_ble_pairing_confirmation_ready(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
    native_hid_addresses: &[u64],
) -> bool {
    embedded_ble_pairing_prompt_ready(pairing) || !native_hid_addresses.is_empty()
}

fn embedded_ble_pairing_confirmation_expiry_should_refresh_background(
    reason: &'static str,
) -> bool {
    reason != EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON
        && reason != EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON
        && reason != EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON
        && reason != EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON
        && reason != EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON
}

struct EmbeddedBlePairingRecoveryGuard {
    inner: Arc<Inner>,
    reason: &'static str,
}

impl Drop for EmbeddedBlePairingRecoveryGuard {
    fn drop(&mut self) {
        self.inner
            .embedded_ble_pairing_recovery_active
            .store(false, Ordering::SeqCst);
        log::info!(
            "[embedded-ble] pairing recovery guard released reason={}",
            self.reason
        );
    }
}

fn try_begin_embedded_ble_pairing_recovery(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Option<EmbeddedBlePairingRecoveryGuard> {
    match inner.embedded_ble_pairing_recovery_active.compare_exchange(
        false,
        true,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        Ok(_) => {
            log::info!("[embedded-ble] pairing recovery guard acquired reason={reason}");
            Some(EmbeddedBlePairingRecoveryGuard {
                inner: Arc::clone(inner),
                reason,
            })
        }
        Err(_) => {
            log::warn!(
                "[embedded-ble] pairing recovery guard busy; deferring overlapping recovery reason={reason}"
            );
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EmbeddedBleStalePairingCleanupOutcome {
    Skipped,
    RetryImmediate,
    RetrySoon,
    HoldForConfirmation,
}

fn embedded_ble_pairing_prompt_waiting_for_windows(
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
) -> bool {
    let Some(pairing) = pairing else {
        return true;
    };
    if embedded_ble_pairing_prompt_ready(pairing) {
        return false;
    }
    match pairing.status {
        crate::embedded_ble::BleDevicePairingPromptStatus::NotFound => pairing.failed_devices == 0,
        crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction => {
            pairing.open_bluetooth_settings
                || pairing.matched_devices > 0
                || pairing.failed_devices == 0
        }
        _ => false,
    }
}

fn embedded_ble_failed_recovery_pairing_should_retry_soon(
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
    recovery_pairing_probe: crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> bool {
    let Some(pairing) = pairing else {
        return false;
    };
    if !recovery_pairing_probe.visible
        || !recovery_pairing_probe.has_random_identity
        || embedded_ble_pairing_prompt_ready(pairing)
    {
        return false;
    }

    matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::NotFound
    ) || (pairing.failed_devices > 0
        && pairing.prompted_devices == 0
        && pairing.already_paired_devices == 0)
}

fn embedded_ble_pairing_recovery_accepts_link_reachable(
    reason: &'static str,
    pairing_ready: bool,
) -> bool {
    if pairing_ready {
        return true;
    }

    !matches!(
        reason,
        EMBEDDED_BLE_TYPE_NATIVE_PAIRING_HANDOFF_REASON
            | EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON
            | EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON
            | EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON
            | EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON
    )
}

fn open_windows_bluetooth_settings_for_embedded_ble_pairing(reason: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        fn wide_null(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(std::iter::once(0)).collect()
        }

        let operation = wide_null("open");
        let target = wide_null("ms-settings:bluetooth");
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(operation.as_ptr()),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            log::warn!(
                "[embedded-ble] open Windows Bluetooth settings failed reason={} shell_result={}",
                reason,
                result.0 as isize
            );
        } else {
            log::info!("[embedded-ble] opened Windows Bluetooth settings reason={reason}");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = reason;
    }
}

fn mark_embedded_ble_pairing_link_reachable(inner: &Arc<Inner>, reason: &'static str) {
    let mut wake = inner.embedded_ble_wake_recovery.lock();
    wake.status = EmbeddedBleWakeRecoveryStatus::Reconnecting;
    wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Opening;
    wake.recent_disconnect_reason = Some(format!(
        "{reason}: local recovery evidence accepted; restoring background notify"
    ));
    wake.user_guidance = "Listener 蓝牙已经重新连上，Type 正在恢复音频 notify。".to_string();
}

fn resume_embedded_ble_listener_after_pairing_recovery(
    inner: &Arc<Inner>,
    reason: &'static str,
    message: EmbeddedBleRecoveryCapsuleMessage,
    emit_reconnecting_capsule: bool,
) {
    clear_embedded_ble_passive_local_reattach(inner, reason);
    mark_embedded_ble_pairing_link_reachable(inner, reason);
    if emit_reconnecting_capsule {
        emit_embedded_ble_recovery_capsule(inner, "reconnecting", message, Some(1800));
    } else {
        log::info!(
            "[embedded-ble] EC11 Type-controlled recovery suppresses intermediate capsule until firmware Type-ready terminal confirmation"
        );
    }
    clear_embedded_ble_pairing_confirmation_hold(inner, reason);
    refresh_embedded_ble_listener(inner);
}

fn arm_embedded_ble_type_pairasync_startup_guard(inner: &Arc<Inner>) {
    let until = Instant::now() + EMBEDDED_BLE_TYPE_PAIRASYNC_STARTUP_GUARD;
    *inner.embedded_ble_type_pairasync_startup_guard_until.lock() = Some(until);
    log::info!(
        "[embedded-ble] Type PairAsync startup manual-delete guard armed duration_ms={}",
        EMBEDDED_BLE_TYPE_PAIRASYNC_STARTUP_GUARD.as_millis()
    );
}

fn embedded_ble_type_pairasync_startup_guard_active(inner: &Arc<Inner>) -> bool {
    let now = Instant::now();
    let mut guard_until = inner.embedded_ble_type_pairasync_startup_guard_until.lock();
    if guard_until.is_some_and(|until| now < until) {
        return true;
    }
    *guard_until = None;
    false
}

async fn embedded_ble_pairing_recovery_link_reachable(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> bool {
    embedded_ble_pairing_recovery_link_reachable_with_timeout(
        inner,
        reason,
        EMBEDDED_BLE_PAIRING_GATT_REBUILD_TIMEOUT,
        None,
    )
    .await
}

async fn embedded_ble_pairing_recovery_link_reachable_with_timeout(
    inner: &Arc<Inner>,
    reason: &'static str,
    timeout: Duration,
    preferred_address: Option<u64>,
) -> bool {
    if inner.shutdown.load(Ordering::SeqCst)
        || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
    {
        return false;
    }
    let result = async_runtime::spawn_blocking(move || match preferred_address {
        Some(address) => {
            crate::embedded_ble::read_embedded_audio_status_for_device(address, timeout)
        }
        None => crate::embedded_ble::read_embedded_audio_status(timeout),
    })
    .await;
    match result {
        Ok(Ok(status)) if status.connected => {
            log::info!(
                "[embedded-ble] {reason}: Listener GATT reachable during pairing recovery detail={:?}",
                status.detail
            );
            true
        }
        Ok(Ok(status)) => {
            log::info!(
                "[embedded-ble] {reason}: Listener GATT status was not connected during pairing recovery detail={:?}",
                status.detail
            );
            false
        }
        Ok(Err(err)) => {
            log::info!(
                "[embedded-ble] {reason}: Listener GATT not reachable during pairing recovery: {}",
                embedded_ble_log_preview(&err)
            );
            false
        }
        Err(err) => {
            log::warn!("[embedded-ble] {reason}: pairing recovery reachability task failed: {err}");
            false
        }
    }
}

fn start_embedded_ble_passive_local_reattach_watch(
    inner: &Arc<Inner>,
    expected_ble_name: String,
    reason: &'static str,
) {
    if inner.shutdown.load(Ordering::SeqCst)
        || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
    {
        return;
    }
    if inner
        .embedded_ble_passive_local_reattach_active
        .swap(true, Ordering::SeqCst)
    {
        log::info!(
            "[embedded-ble] passive local Windows reattach monitor already active reason={reason} target={expected_ble_name:?}"
        );
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        let baseline_native_hid_addresses = match async_runtime::spawn_blocking(|| {
            // The present-only CIM query is ~3x cheaper than the full Get-PnpDevice
            // enumeration and is the correct evidence class here: a fresh local
            // re-pair always produces present PnP nodes.
            crate::embedded_ble::native_windows_hid_present_pairing_addresses()
        })
        .await
        {
            Ok(Ok(addresses)) => Some(addresses),
            Ok(Err(err)) => {
                log::debug!(
                    "[embedded-ble] passive local Windows reattach baseline HID check unavailable: {err}"
                );
                None
            }
            Err(err) => {
                log::debug!(
                    "[embedded-ble] passive local Windows reattach baseline HID task failed: {err}"
                );
                None
            }
        };
        let baseline_labels = baseline_native_hid_addresses
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|address| format!("{address:012X}"))
            .collect::<Vec<_>>();
        log::info!(
            "[embedded-ble] passive local Windows reattach monitor started reason={reason} target={expected_ble_name:?}; baseline_native_hid_addresses={baseline_labels:?}; waiting only for explicit local pairing/HID evidence"
        );
        loop {
            if inner.shutdown.load(Ordering::SeqCst)
                || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            {
                clear_embedded_ble_passive_local_reattach(&inner, reason);
                log::info!(
                    "[embedded-ble] passive local Windows reattach monitor stopped reason={reason}"
                );
                break;
            }
            if !inner
                .embedded_ble_passive_local_reattach_active
                .load(Ordering::SeqCst)
            {
                log::info!(
                    "[embedded-ble] passive local Windows reattach monitor superseded reason={reason}"
                );
                break;
            }

            let native_hid_pairing = async_runtime::spawn_blocking(|| {
                crate::embedded_ble::native_windows_hid_present_pairing_addresses()
            })
            .await;
            let native_hid_addresses = match native_hid_pairing {
                Ok(Ok(addresses)) => addresses,
                Ok(Err(err)) => {
                    log::debug!(
                        "[embedded-ble] passive local Windows reattach native HID check unavailable: {err}"
                    );
                    Vec::new()
                }
                Err(err) => {
                    log::debug!(
                        "[embedded-ble] passive local Windows reattach native HID task failed: {err}"
                    );
                    Vec::new()
                }
            };
            let fresh_native_hid_address =
                baseline_native_hid_addresses
                    .as_deref()
                    .and_then(|baseline| {
                        native_hid_addresses
                            .iter()
                            .copied()
                            .find(|address| !baseline.contains(address))
                    });
            let fresh_native_hid_evidence = fresh_native_hid_address.is_some();
            let pairing = if fresh_native_hid_evidence {
                None
            } else {
                let expected_for_query = expected_ble_name.clone();
                match async_runtime::spawn_blocking(move || {
                    crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
                })
                .await
                {
                    Ok(pairing) => Some(pairing),
                    Err(err) => {
                        log::debug!(
                            "[embedded-ble] passive local Windows reattach pairing poll unavailable: {err}"
                        );
                        None
                    }
                }
            };
            if embedded_ble_passive_local_reattach_evidence_ready(
                pairing.as_ref(),
                &native_hid_addresses,
                baseline_native_hid_addresses.as_deref(),
            ) {
                let labels = native_hid_addresses
                    .iter()
                    .map(|address| format!("{address:012X}"))
                    .collect::<Vec<_>>();
                let evidence = if fresh_native_hid_evidence {
                    "new native HID address after passive monitor baseline"
                } else {
                    "current Windows pairing plus Listener HID"
                };
                log::info!(
                    "[embedded-ble] passive local Windows reattach accepted explicit local pairing evidence={evidence} status={:?} matched={} already_paired={} native_hid_addresses={labels:?}; rebuilding GATT",
                    pairing.as_ref().map(|value| value.status),
                    pairing.as_ref().map_or(0, |value| value.matched_devices),
                    pairing.as_ref().map_or(0, |value| value.already_paired_devices)
                );
                if let Some(address) = fresh_native_hid_address {
                    *inner.embedded_ble_power_cycle_hid_observed_at.lock() = Some(Instant::now());
                    log::info!(
                        "[embedded-ble] passive local Windows reattach observed new local HID address={address:012X}; reopening background notify directly without the status-characteristic probe"
                    );
                    arm_embedded_ble_type_pairasync_startup_guard(&inner);
                    resume_embedded_ble_listener_after_pairing_recovery(
                        &inner,
                        "passive local Windows reattach new HID evidence",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        true,
                    );
                    break;
                }
                let link_reachable = embedded_ble_pairing_recovery_link_reachable(
                    &inner,
                    "passive local Windows reattach current-pairing GATT link check",
                )
                .await;
                if link_reachable {
                    // Windows may still rebuild the HID/GATT service graph after the
                    // fresh local pairing is reachable. Skip one startup stale-HID
                    // preflight during that bounded rebuild window; otherwise the
                    // old deleted address can pause notify immediately again.
                    arm_embedded_ble_type_pairasync_startup_guard(&inner);
                    resume_embedded_ble_listener_after_pairing_recovery(
                        &inner,
                        "passive local Windows reattach paired and link reachable",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        true,
                    );
                    break;
                }
            } else {
                log::debug!(
                    "[embedded-ble] passive local Windows reattach still waiting for current local pairing plus HID evidence; no GATT probe status={:?} matched={} already_paired={} native_hid_count={} baseline_hid_count={}",
                    pairing.as_ref().map(|value| value.status),
                    pairing.as_ref().map_or(0, |value| value.matched_devices),
                    pairing.as_ref().map_or(0, |value| value.already_paired_devices),
                    native_hid_addresses.len(),
                    baseline_native_hid_addresses.as_ref().map_or(0, Vec::len)
                );
            }

            tokio::time::sleep(EMBEDDED_BLE_PASSIVE_LOCAL_REATTACH_POLL).await;
        }
    });
}

fn embedded_ble_passive_local_reattach_evidence_ready(
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
    native_hid_addresses: &[u64],
    baseline_native_hid_addresses: Option<&[u64]>,
) -> bool {
    let paired_devices_visible = pairing.is_some_and(|value| value.already_paired_devices > 0);
    let fresh_native_hid_after_baseline = baseline_native_hid_addresses.is_some_and(|baseline| {
        native_hid_addresses
            .iter()
            .any(|address| !baseline.contains(address))
    });
    denzic_ble_pairing::reattach_evidence_ready(
        paired_devices_visible,
        !native_hid_addresses.is_empty(),
        fresh_native_hid_after_baseline,
    )
}

fn start_embedded_ble_pairing_confirmation_watch(
    inner: &Arc<Inner>,
    expected_ble_name: String,
    hold_generation: u64,
    reason: &'static str,
) {
    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        log::info!(
            "[embedded-ble] Windows pairing confirmation watch started reason={reason} hold_generation={hold_generation} target={expected_ble_name:?}"
        );
        loop {
            if inner.shutdown.load(Ordering::SeqCst)
                || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            {
                log::info!(
                    "[embedded-ble] Windows pairing confirmation watch stopped reason={reason}"
                );
                break;
            }
            if inner
                .embedded_ble_pairing_hold_generation
                .load(Ordering::SeqCst)
                != hold_generation
            {
                log::info!(
                    "[embedded-ble] Windows pairing confirmation watch superseded reason={reason} hold_generation={hold_generation}"
                );
                break;
            }
            let Some(remaining) =
                embedded_ble_pairing_confirmation_hold_remaining(&inner, Instant::now())
            else {
                if !embedded_ble_pairing_confirmation_expiry_should_refresh_background(reason) {
                    log::warn!(
                        "[embedded-ble] Windows pairing confirmation watch expired after user-controlled recovery; leaving background listener paused until explicit Windows pairing or next scheduled retry reason={reason}"
                    );
                    {
                        let mut wake = inner.embedded_ble_wake_recovery.lock();
                        wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
                        wake.notify_subscription_state =
                            EmbeddedBleNotifySubscriptionState::Cancelled;
                        wake.recent_disconnect_reason = Some(format!(
                            "{reason} hold expired without confirmed Listener GATT recovery"
                        ));
                        wake.user_guidance = format!(
                            "Listener 正在等待 Windows 重新配对 {expected_ble_name}。Type 不会自动抢回连接；如需继续使用这台电脑，请在 Windows 蓝牙里手动添加设备。"
                        );
                    }
                    start_embedded_ble_passive_local_reattach_watch(
                        &inner,
                        expected_ble_name.clone(),
                        reason,
                    );
                    break;
                }
                log::warn!(
                    "[embedded-ble] Windows pairing confirmation watch expired; refreshing background listener reason={reason}"
                );
                refresh_embedded_ble_listener(&inner);
                break;
            };

            let expected_for_query = expected_ble_name.clone();
            let query = async_runtime::spawn_blocking(move || {
                crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
            })
            .await;
            match query {
                Ok(pairing) => {
                    log::info!(
                        "[embedded-ble] Windows pairing confirmation poll status={:?} matched={} already_paired={} failed={} open_settings={} remaining_ms={}",
                        pairing.status,
                        pairing.matched_devices,
                        pairing.already_paired_devices,
                        pairing.failed_devices,
                        pairing.open_bluetooth_settings,
                        remaining.as_millis()
                    );
                    let native_hid_pairing = async_runtime::spawn_blocking(|| {
                        crate::embedded_ble::native_windows_hid_pairing_addresses()
                    })
                    .await;
                    let native_hid_addresses = match native_hid_pairing {
                        Ok(Ok(addresses)) => addresses,
                        Ok(Err(err)) => {
                            log::warn!(
                                "[embedded-ble] Windows pairing confirmation native HID check unavailable: {err}"
                            );
                            Vec::new()
                        }
                        Err(err) => {
                            log::warn!(
                                "[embedded-ble] Windows pairing confirmation native HID task failed: {err}"
                            );
                            Vec::new()
                        }
                    };
                    if !native_hid_addresses.is_empty() {
                        let labels = native_hid_addresses
                            .iter()
                            .map(|address| format!("{address:012X}"))
                            .collect::<Vec<_>>();
                        log::info!(
                            "[embedded-ble] Windows pairing confirmation accepted complete native Listener HID evidence addresses={labels:?} while paired-AEP state settles"
                        );
                    }
                    let pairing_ready =
                        embedded_ble_pairing_confirmation_ready(&pairing, &native_hid_addresses);
                    let link_reachable = if embedded_ble_pairing_recovery_accepts_link_reachable(
                        reason,
                        pairing_ready,
                    ) {
                        embedded_ble_pairing_recovery_link_reachable(
                            &inner,
                            if pairing_ready {
                                "Windows pairing confirmation watch paired link check"
                            } else {
                                "Windows pairing confirmation watch link reachable"
                            },
                        )
                        .await
                    } else {
                        log::info!(
                                "[embedded-ble] Windows pairing confirmation watch ignoring GATT reachability until Windows pairing is confirmed reason={reason} remaining_ms={}",
                                remaining.as_millis()
                            );
                        false
                    };
                    if link_reachable {
                        resume_embedded_ble_listener_after_pairing_recovery(
                            &inner,
                            if pairing_ready {
                                "Windows pairing confirmation watch paired and link reachable"
                            } else {
                                "Windows pairing confirmation watch link reachable"
                            },
                            if pairing_ready {
                                EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio
                            } else {
                                EmbeddedBleRecoveryCapsuleMessage::RestoringAudio
                            },
                            true,
                        );
                        break;
                    }
                    if pairing_ready {
                        log::info!(
                            "[embedded-ble] Windows pairing confirmation watch paired; waiting for Listener GATT to become ready remaining_ms={}",
                            remaining.as_millis()
                        );
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] Windows pairing confirmation poll task failed: {err}"
                    );
                }
            }

            tokio::time::sleep(remaining.min(EMBEDDED_BLE_PAIRING_CONFIRMATION_POLL)).await;
        }
    });
}

fn startup_ble_name_sync_reason(reason: &'static str) -> bool {
    matches!(
        reason,
        "startup_embedded_ble_power_probe" | "auto_input_source_probe"
    )
}

fn mark_startup_ble_name_sync_done(inner: &Arc<Inner>, reason: &'static str) {
    if !startup_ble_name_sync_reason(reason) {
        return;
    }
    let was_done = inner
        .embedded_ble_startup_name_sync_done
        .swap(true, Ordering::SeqCst);
    if !was_done {
        log::info!("[embedded-ble] startup BLE name sync gate opened reason={reason}");
    }
}

fn sync_device_ble_name_from_firmware_settings(inner: &Arc<Inner>, reason: &'static str) -> bool {
    let synced = match crate::embedded_ble::read_device_settings_status(Duration::from_secs(2)) {
        Ok(status) => {
            record_embedded_ble_device_settings_power_status(inner, &status, reason);
            let firmware_name = status.ble_name.trim();
            let valid = crate::types::device_ble_name_is_valid(firmware_name);
            if status.ble_name_pending_restart || !valid {
                log::info!(
                    "[embedded-ble] startup BLE name sync skipped reason={reason} firmware_name={firmware_name:?} pending={} valid={valid}",
                    status.ble_name_pending_restart
                );
                false
            } else {
                let mut prefs = inner.prefs.get();
                if prefs.device_ble_name == firmware_name {
                    crate::embedded_ble::set_configured_bluetooth_target_name(firmware_name);
                    false
                } else {
                    let previous = prefs.device_ble_name.clone();
                    prefs.device_ble_name = firmware_name.to_string();
                    match inner.prefs.set(prefs.clone()) {
                        Ok(()) => {
                            crate::embedded_ble::set_configured_bluetooth_target_name(
                                firmware_name,
                            );
                            log::warn!(
                                "[embedded-ble] startup BLE name sync updated Type target from {previous:?} to firmware name {firmware_name:?} reason={reason}"
                            );
                            if let Some(app) = inner.app.lock().clone() {
                                let _ = app.emit("prefs:changed", &prefs);
                                let _ = app.emit_to("main", "prefs:changed", &prefs);
                            }
                            true
                        }
                        Err(err) => {
                            log::warn!(
                                "[embedded-ble] startup BLE name sync persist failed previous={previous:?} firmware_name={firmware_name:?} reason={reason}: {err}"
                            );
                            false
                        }
                    }
                }
            }
        }
        Err(err) => {
            log::info!(
                "[embedded-ble] startup BLE name sync skipped reason={reason} device settings unavailable: {}",
                embedded_ble_log_preview(&err)
            );
            false
        }
    };
    mark_startup_ble_name_sync_done(inner, reason);
    synced
}

fn record_embedded_ble_device_settings_power_status(
    inner: &Arc<Inner>,
    status: &crate::embedded_ble::DeviceSettingsStatus,
    reason: &'static str,
) {
    let usb_powered = status.external_power_present
        || status.usb_power_present
        || status.charging
        || status.charge_full;
    let mut snapshot = inner.embedded_ble_wake_recovery.lock();
    snapshot.usb_powered = Some(usb_powered);
    log::info!(
        "[embedded-ble] cached device settings power state reason={reason} usb_powered={usb_powered} external_power={} usb_power={} charging={} charge_full={}",
        status.external_power_present,
        status.usb_power_present,
        status.charging,
        status.charge_full
    );
}

fn refresh_embedded_ble_listener(inner: &Arc<Inner>) {
    refresh_embedded_ble_listener_with_options(inner, false, false);
}

fn refresh_embedded_ble_listener_after_firmware_ota(inner: &Arc<Inner>) {
    refresh_embedded_ble_listener_with_options(inner, false, true);
}

fn refresh_embedded_ble_listener_for_device_key_wake(inner: &Arc<Inner>) {
    if embedded_ble_listener_capture_active(inner) {
        log::info!(
            "[embedded-ble] device-key Idle wake joined active notify recovery without replacing the capture"
        );
        return;
    }
    refresh_embedded_ble_listener_with_options(inner, true, false);
}

fn refresh_embedded_ble_listener_with_options(
    inner: &Arc<Inner>,
    device_key_idle_wake: bool,
    firmware_ota_recovery: bool,
) {
    if !inner
        .embedded_ble_startup_name_sync_done
        .load(Ordering::SeqCst)
    {
        log::info!(
            "[embedded-ble] background listener refresh deferred until startup BLE name sync completes"
        );
        return;
    }
    if inner.embedded_ble_ota_active.load(Ordering::SeqCst) {
        log::info!("[embedded-ble] background listener refresh skipped during firmware OTA");
        return;
    }
    if inner
        .embedded_ble_passive_local_reattach_active
        .load(Ordering::SeqCst)
    {
        cancel_embedded_ble_listener_capture(
            inner,
            "passively awaiting explicit local Windows re-pair",
            false,
        );
        log::info!(
            "[embedded-ble] background listener refresh skipped while passively awaiting explicit local Windows re-pair"
        );
        return;
    }
    if let Some(remaining) = embedded_ble_pairing_confirmation_hold_remaining(inner, Instant::now())
    {
        cancel_embedded_ble_listener_capture(inner, "Windows pairing confirmation hold", false);
        log::info!(
            "[embedded-ble] background listener refresh skipped while waiting for Windows pairing confirmation remaining_ms={}",
            remaining.as_millis()
        );
        return;
    }
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    if device_key_idle_wake {
        inner
            .embedded_ble_device_key_wake_generation
            .store(generation, Ordering::SeqCst);
    }
    if firmware_ota_recovery {
        inner
            .embedded_ble_ota_recovery_generation
            .store(generation, Ordering::SeqCst);
    }
    cancel_embedded_ble_listener_capture(inner, "refresh", false);
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
    snapshot.consecutive_reconnect_failures =
        snapshot.consecutive_reconnect_failures.saturating_add(1);
    snapshot.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Opening;
    snapshot.last_attempt_at = Some(now_rfc3339());
    log::info!(
        "[embedded-ble] wake recovery attempt #{} consecutive_failures={} reason={reason}",
        snapshot.reconnect_attempts,
        snapshot.consecutive_reconnect_failures
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
    let consecutive_reconnect_failures = snapshot.consecutive_reconnect_failures;
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
        "[embedded-ble] notify ready recovery decision recovered={} emit_recovered_capsule={} previous_status={:?} previous_notify_state={:?} notify_was_recovering={} reconnect_attempts={} consecutive_failures={} usb_powered={:?} battery_percent={:?} recent_disconnect_kind={:?} recent_disconnect_automatic_recovery={} recent_disconnect_low_power_idle={} recent_disconnect_reason={}",
        recovered,
        emit_recovered_capsule,
        previous_status,
        previous_notify_state,
        notify_was_recovering,
        reconnect_attempts,
        consecutive_reconnect_failures,
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
    snapshot.consecutive_reconnect_failures = 0;
    if recovered {
        snapshot.recent_disconnect_reason = None;
    }
    crate::startup_evidence::record_background_notify_ready();
    emit_recovered_capsule
}

fn firmware_mode_for_device_knob_rotation_action(action: DeviceKnobRotationAction) -> &'static str {
    match action {
        DeviceKnobRotationAction::SystemVolume => "system_volume",
        DeviceKnobRotationAction::ScreenBrightness => "screen_brightness",
        DeviceKnobRotationAction::Disabled => "disabled",
    }
}

fn sync_device_knob_rotation_action_to_firmware(inner: &Arc<Inner>, reason: &'static str) {
    let prefs = inner.prefs.get();
    let action = prefs.device_knob_rotation_action;
    let mode = firmware_mode_for_device_knob_rotation_action(action);
    let ec11_fast_recording = prefs.dictation_input_source == DictationInputSource::EmbeddedBle
        && prefs.device_custom_keys.knob.action == DeviceCustomKeyAction::Dictation;
    async_runtime::spawn_blocking(move || {
        let command = format!(
            "DEVICE:SET knob_rotation={mode} e11r={}",
            if ec11_fast_recording { 1 } else { 0 }
        );
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut attempt = 0u32;
        let last_err = loop {
            attempt = attempt.saturating_add(1);
            match crate::embedded_ble::send_device_settings_command_via_active_capture_only(
                &command,
                Duration::from_secs(2),
                "device knob rotation sync",
            ) {
                Ok(()) => {
                    log::info!(
                        "[device-knob] synced knob_rotation and EC11 fast-recording settings via active capture mode={mode} fast_recording={} reason={reason} attempts={attempt}",
                        ec11_fast_recording as u8,
                    );
                    return;
                }
                Err(err) => {
                    if Instant::now() >= deadline {
                        break err;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        };
        log::warn!(
            "[device-knob] knob_rotation/EC11 fast-recording active-capture sync deferred reason={reason} mode={mode} fast_recording={} attempts={attempt} last_error={}",
            ec11_fast_recording as u8,
            last_err
        );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddedBleRecoveryCapsuleMessage {
    RestoringAudio,
    WaitingWindowsPairing,
    WaitingManualPairing,
    RebuildingPairing,
    CleaningPairing,
    LocalPairingRestoringAudio,
    WaitingTypePairing,
    AudioRecovered,
}

impl EmbeddedBleRecoveryCapsuleMessage {
    fn text(self) -> &'static str {
        match self {
            Self::RestoringAudio => "正在恢复 Listener 音频",
            Self::WaitingWindowsPairing => "等待 Windows 配对",
            Self::WaitingManualPairing => "等待手动配对",
            Self::RebuildingPairing => "正在重建 Listener 配对",
            Self::CleaningPairing => "正在清理旧配对",
            Self::LocalPairingRestoringAudio => "本机配对完成，恢复音频",
            Self::WaitingTypePairing => "等待本机配对",
            Self::AudioRecovered => "Listener 音频已恢复",
        }
    }
}

fn emit_embedded_ble_recovery_capsule(
    inner: &Arc<Inner>,
    state_label: &str,
    message: EmbeddedBleRecoveryCapsuleMessage,
    idle_after_ms: Option<u64>,
) {
    let state = if state_label == "reconnected" {
        CapsuleState::Done
    } else {
        CapsuleState::Reconnecting
    };
    emit_capsule(inner, state, 0.0, 0, Some(message.text().to_string()), None);
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

fn embedded_ble_listener_generation_is_current(inner: &Arc<Inner>, generation: u64) -> bool {
    !inner.shutdown.load(Ordering::SeqCst)
        && inner
            .embedded_ble_listener_generation
            .load(Ordering::SeqCst)
            == generation
        && !inner.embedded_ble_ota_active.load(Ordering::SeqCst)
        && inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
}

fn take_embedded_ble_device_key_wake_preflight_bypass(inner: &Arc<Inner>, generation: u64) -> bool {
    inner
        .embedded_ble_device_key_wake_generation
        .compare_exchange(generation, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

fn take_embedded_ble_ota_recovery_preflight_bypass(inner: &Arc<Inner>, generation: u64) -> bool {
    inner
        .embedded_ble_ota_recovery_generation
        .compare_exchange(generation, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

async fn embedded_ble_background_listener_loop(inner: Arc<Inner>, generation: u64) {
    log::info!("[embedded-ble] background listener started generation={generation}");
    crate::startup_evidence::record_startup_stage("background_listener_started");
    let mut retry_delay = EMBEDDED_BLE_RETRY_BASE_DELAY;
    let mut last_stale_cleanup_at: Option<Instant> = None;
    let mut startup_pairing_preflight_checked = false;
    loop {
        if !embedded_ble_listener_generation_is_current(&inner, generation) {
            break;
        }
        if let Some(remaining) =
            embedded_ble_pairing_confirmation_hold_remaining(&inner, Instant::now())
        {
            log::info!(
                "[embedded-ble] background listener waiting for Windows pairing confirmation generation={generation} remaining_ms={}",
                remaining.as_millis()
            );
            tokio::time::sleep(remaining.min(EMBEDDED_BLE_RETRY_LONG_DELAY)).await;
            continue;
        }
        if !startup_pairing_preflight_checked {
            startup_pairing_preflight_checked = true;
            if take_embedded_ble_device_key_wake_preflight_bypass(&inner, generation) {
                log::info!(
                    "[embedded-ble] device-key Idle wake bypassed slow manual-delete preflight generation={generation}; physical HID input proves the local pairing remains installed"
                );
            } else if take_embedded_ble_ota_recovery_preflight_bypass(&inner, generation) {
                log::info!(
                    "[embedded-ble] confirmed firmware OTA recovery bypassed repeated Windows HID pairing preflight generation={generation}; the pre-OTA active link and post-OTA service probe prove this trusted local path"
                );
            } else if maybe_hold_embedded_ble_startup_without_current_native_pairing(&inner).await {
                continue;
            }
            // The PnP preflight runs in a blocking task. A device-key wake may have
            // superseded this loop while that task was still enumerating Windows.
            if !embedded_ble_listener_generation_is_current(&inner, generation) {
                break;
            }
        }

        let cancel_capture = install_embedded_ble_listener_cancel(&inner, generation);
        let leave_notify_cccd_enabled_on_cancel =
            embedded_ble_listener_cccd_handoff_flag(&inner, &cancel_capture);
        record_embedded_ble_reconnect_attempt(&inner, "background_listener_loop");
        match submit_embedded_audio_ble_stream_background(
            &inner,
            Arc::clone(&cancel_capture),
            leave_notify_cccd_enabled_on_cancel,
        )
        .await
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
                if crate::embedded_ble::is_background_listener_deferred_for_ota_error(&err) {
                    clear_embedded_ble_listener_last_error(&inner);
                    retry_delay = EMBEDDED_BLE_RETRY_OTA_DEFER_DELAY;
                    log::info!(
                        "[embedded-ble] background listener deferred while firmware OTA is active; retrying in {} ms",
                        retry_delay.as_millis()
                    );
                    tokio::time::sleep(retry_delay).await;
                    continue;
                }
                if is_embedded_ble_idle_timeout_error(&err) {
                    clear_embedded_ble_listener_last_error(&inner);
                    retry_delay = next_embedded_ble_background_retry_delay(&err, retry_delay);
                } else {
                    record_embedded_ble_listener_last_error(&inner, &err);
                    record_embedded_ble_recovery_failure(&inner, &err);
                    if maybe_hold_embedded_ble_after_lost_native_pairing(&inner, &err).await {
                        log::info!(
                            "[embedded-ble] background listener stopped after current native Windows pairing disappeared during link recovery"
                        );
                        break;
                    }
                    let stale_cleanup_outcome =
                        maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
                            &inner,
                            &err,
                            &mut last_stale_cleanup_at,
                        )
                        .await;
                    if inner
                        .embedded_ble_passive_local_reattach_active
                        .load(Ordering::SeqCst)
                    {
                        log::info!(
                            "[embedded-ble] background listener stopped while passive local Windows reattach monitor owns recovery"
                        );
                        break;
                    }
                    if stale_cleanup_outcome == EmbeddedBleStalePairingCleanupOutcome::Skipped
                        && should_emit_embedded_ble_background_recovery_capsule(&inner, &err)
                    {
                        emit_embedded_ble_recovery_capsule(
                            &inner,
                            "reconnecting",
                            EmbeddedBleRecoveryCapsuleMessage::RestoringAudio,
                            Some(1800),
                        );
                    } else if is_embedded_ble_automatic_recovery_error(&err) {
                        log::info!(
                            "[embedded-ble] background recovery capsule suppressed by pairing/recovery decision outcome={:?}",
                            stale_cleanup_outcome
                        );
                    }
                    let stale_cleanup_retry_soon =
                        stale_cleanup_outcome == EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
                    let stale_cleanup_retry_immediate = stale_cleanup_outcome
                        == EmbeddedBleStalePairingCleanupOutcome::RetryImmediate;
                    let stale_cleanup_holding = stale_cleanup_outcome
                        == EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
                    let stale_cleanup_skipped =
                        stale_cleanup_outcome == EmbeddedBleStalePairingCleanupOutcome::Skipped;
                    let stale_cleanup_backoff = stale_cleanup_holding
                        || (stale_cleanup_skipped
                            && should_throttle_embedded_ble_background_stale_pairing_cleanup(
                                &err,
                                &embedded_ble_wake_recovery_snapshot(&inner),
                                last_stale_cleanup_at,
                                Instant::now(),
                            ));
                    if stale_cleanup_backoff && stale_cleanup_skipped {
                        log::warn!(
                            "[embedded-ble] background stale pairing cleanup cooldown active; using long retry backoff err={}",
                            embedded_ble_log_preview(&err),
                        );
                    }
                    retry_delay = if stale_cleanup_retry_immediate {
                        Duration::ZERO
                    } else if stale_cleanup_retry_soon {
                        EMBEDDED_BLE_RETRY_LONG_DELAY
                    } else if stale_cleanup_backoff {
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
) -> EmbeddedBleStalePairingCleanupOutcome {
    let snapshot = embedded_ble_wake_recovery_snapshot(inner);
    let now = Instant::now();
    let recovery_pairing_probe = maybe_probe_embedded_ble_recovery_pairing_advertisement(
        inner,
        err,
        &snapshot,
        *last_cleanup_at,
        now,
    )
    .await;
    if !embedded_ble_recovery_error_still_current(inner, err, "after_recovery_advertisement_probe")
    {
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    let recovery_pairing_window_visible = recovery_pairing_probe.visible;
    let pairing_confirmation_hold_active =
        embedded_ble_pairing_confirmation_hold_remaining(inner, now).is_some();
    let hardware_ec11_recovery_notice = embedded_ble_hardware_ec11_recovery_notice_observed(err);
    let active_capture_type_recovery =
        recovery_pairing_advertisement_already_observed_during_active_capture(err);
    let type_observed_recovery_advertisement = !pairing_confirmation_hold_active
        && (recovery_pairing_advertisement_already_observed_during_notify_open(err)
            || (hardware_ec11_recovery_notice && recovery_pairing_probe.visible));
    let stale_cleanup_candidate = should_attempt_embedded_ble_background_stale_pairing_cleanup(
        err,
        &snapshot,
        *last_cleanup_at,
        now,
    );
    let direct_gatt_instability_recovery = recovery_pairing_window_visible
        && should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
            err,
            &snapshot,
            *last_cleanup_at,
            now,
        );
    let visible_recovery_allows_cleanup = recovery_pairing_probe_allows_immediate_stale_cleanup(
        err,
        &recovery_pairing_probe,
        direct_gatt_instability_recovery,
    );
    let recovery_advertisement_allows_cleanup =
        recovery_pairing_advertisement_allows_immediate_stale_cleanup(err);
    let should_query_pairing_preflight = !type_observed_recovery_advertisement
        && (recovery_pairing_window_visible
            || stale_cleanup_candidate
            || visible_recovery_allows_cleanup
            || recovery_advertisement_allows_cleanup);
    let usb_ble_name_synced = should_query_pairing_preflight
        && sync_device_ble_name_from_firmware_settings(
            inner,
            "background_stale_pairing_usb_name_probe",
        );
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let pairing_before_cleanup = if should_query_pairing_preflight {
        let expected_ble_name = expected_ble_name.clone();
        let pairing = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::query_listener_pairing(Some(expected_ble_name.as_str()))
        })
        .await;
        match pairing {
            Ok(pairing) => {
                log::info!(
                    "[embedded-ble] background stale pairing cleanup preflight Windows pairing status={:?} matched={} already_paired={} failed={} open_settings={} direct_gatt_instability_recovery={direct_gatt_instability_recovery}",
                    pairing.status,
                    pairing.matched_devices,
                    pairing.already_paired_devices,
                    pairing.failed_devices,
                    pairing.open_bluetooth_settings,
                );
                Some(pairing)
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] background stale pairing cleanup preflight pairing query failed; continuing conservative recovery: {err}"
                );
                None
            }
        }
    } else {
        None
    };
    let native_windows_hid_pairing_visible = if should_query_pairing_preflight {
        match async_runtime::spawn_blocking(|| {
            crate::embedded_ble::native_windows_hid_pairing_addresses()
        })
        .await
        {
            Ok(Ok(addresses)) if !addresses.is_empty() => {
                log::info!(
                    "[embedded-ble] background stale pairing cleanup found native Windows HID pairing evidence addresses={addresses:?}; evaluating whether it is current or stale"
                );
                true
            }
            Ok(Ok(_)) => false,
            Ok(Err(err)) => {
                log::warn!(
                    "[embedded-ble] background native Windows HID pairing evidence unavailable; preserving manual-delete safety: {err}"
                );
                false
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] background native Windows HID pairing evidence task failed; preserving manual-delete safety: {err}"
                );
                false
            }
        }
    } else {
        false
    };
    let native_windows_hid_pairing_blocks_pairasync =
        native_windows_hid_pairing_blocks_type_pairasync(
            native_windows_hid_pairing_visible,
            &recovery_pairing_probe,
            pairing_before_cleanup.as_ref(),
        );
    if native_windows_hid_pairing_visible && !native_windows_hid_pairing_blocks_pairasync {
        log::warn!(
            "[embedded-ble] recovery advertisement proves native Windows HID evidence is stale; allowing bounded Type PairAsync cleanup"
        );
    }
    if !embedded_ble_recovery_error_still_current(inner, err, "after_pairing_preflight") {
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    // Seeing a recovery advertisement after a link loss does not prove that
    // Type initiated recovery. Windows manual delete creates the same signal.
    let stale_native_hid_recovery =
        native_windows_hid_pairing_visible && !native_windows_hid_pairing_blocks_pairasync;
    let type_controlled_recovery =
        type_observed_recovery_advertisement || stale_native_hid_recovery;
    let ec11_type_controlled_recovery = type_controlled_recovery && hardware_ec11_recovery_notice;
    let recovery_advertisement_type_owned_cleanup = type_controlled_recovery;
    let noisy_cccd_stale_cache_type_owned_cleanup =
        pairing_before_cleanup.as_ref().is_some_and(|pairing| {
            noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
                err,
                stale_cleanup_candidate,
                pairing,
            )
        });
    let type_owned_stale_cache_cleanup =
        recovery_advertisement_type_owned_cleanup || noisy_cccd_stale_cache_type_owned_cleanup;
    let manual_unpair_hold = !native_windows_hid_pairing_blocks_pairasync
        && !usb_ble_name_synced
        && pairing_before_cleanup.as_ref().is_some_and(|pairing| {
            should_hold_embedded_ble_background_recovery_after_manual_unpair(
                pairing,
                direct_gatt_instability_recovery,
                type_owned_stale_cache_cleanup,
            )
        });
    let local_stale_cache_recovery_allows_cleanup = recovery_pairing_window_visible
        && pairing_before_cleanup.as_ref().is_some_and(|pairing| {
            !manual_unpair_hold
                && (pairing.already_paired_devices > 0
                    || pairing.matched_devices > 0
                    || pairing.failed_devices > 0)
        });
    let automatic_cleanup_allowed = !native_windows_hid_pairing_blocks_pairasync
        && embedded_ble_background_pairasync_is_authorized(
            manual_unpair_hold,
            type_observed_recovery_advertisement,
            visible_recovery_allows_cleanup,
            stale_cleanup_candidate,
            noisy_cccd_stale_cache_type_owned_cleanup,
            local_stale_cache_recovery_allows_cleanup,
            usb_ble_name_synced,
        );
    let mut device_control_recovery =
        crate::device_control_platform::BackgroundPairingRecovery::begin(
            type_controlled_recovery,
            manual_unpair_hold,
            automatic_cleanup_allowed,
        );
    let device_control_decision = device_control_recovery.decision();
    log::info!(
        "[embedded-ble] device-control recovery transaction execute={} replayed={} result={:?} error={:?} type_controlled={} manual_unpair={} authorized={}",
        device_control_decision.execute,
        device_control_decision.replayed,
        device_control_decision.result,
        device_control_decision.error,
        type_controlled_recovery,
        manual_unpair_hold,
        automatic_cleanup_allowed,
    );
    if noisy_cccd_stale_cache_type_owned_cleanup {
        log::warn!(
            "[embedded-ble] noisy CCCD stale Windows cache evidence allows Type automatic PairAsync recovery even though recovery advertisement scan may have missed err={}",
            embedded_ble_log_preview(err),
        );
    }
    if recovery_pairing_probe.has_random_identity {
        if visible_recovery_allows_cleanup || local_stale_cache_recovery_allows_cleanup {
            log::warn!(
                "[embedded-ble] random-identity recovery advertisement visible with stale Windows cache evidence; entering Windows pairing cleanup err={}",
                embedded_ble_log_preview(err),
            );
        } else {
            log::warn!(
                "[embedded-ble] random-identity recovery advertisement visible; waiting for user-controlled Windows pairing instead of retrying stale GATT address err={}",
                embedded_ble_log_preview(err),
            );
        }
    }
    if recovery_pairing_window_visible
        && !direct_gatt_instability_recovery
        && !automatic_cleanup_allowed
        && !manual_unpair_hold
    {
        if native_windows_hid_pairing_blocks_pairasync {
            log::info!(
                "[embedded-ble] native Windows HID pairing remains installed; retrying direct GATT without pairing cleanup"
            );
            return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
        }
        *last_cleanup_at = Some(now);
        log::warn!(
            "[embedded-ble] recovery pairing advertisement visible from hardware/user action; holding background listener without automatic PairAsync err={}",
            embedded_ble_log_preview(err),
        );
        let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
            inner,
            EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON,
        );
        start_embedded_ble_pairing_confirmation_watch(
            inner,
            expected_ble_name.clone(),
            hold_generation,
            EMBEDDED_BLE_HARDWARE_RECOVERY_PAIRING_HOLD_REASON,
        );
        {
            let mut wake = inner.embedded_ble_wake_recovery.lock();
            wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
            wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
            wake.recent_disconnect_reason = Some(format!(
                "Listener recovery advertisement is visible; Type is waiting for explicit Windows pairing target={expected_ble_name}"
            ));
            wake.user_guidance = format!(
                "Listener 已进入重新配对状态。Type 不会自动抢回连接；如需继续使用这台电脑，请在 Windows 蓝牙里手动添加 {expected_ble_name}。"
            );
        }
        emit_embedded_ble_recovery_capsule(
            inner,
            "reconnecting",
            EmbeddedBleRecoveryCapsuleMessage::WaitingWindowsPairing,
            Some(4200),
        );
        return EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
    }
    if manual_unpair_hold {
        *last_cleanup_at = Some(now);
        hold_embedded_ble_for_manual_windows_unpair(inner, &expected_ble_name).await;
        return EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
    }
    if !automatic_cleanup_allowed {
        if recovery_pairing_window_visible {
            log::info!(
                "[embedded-ble] recovery pairing advertisement visible after transient link loss; retrying direct audio GATT before clearing Windows pairing cache err={}",
                embedded_ble_log_preview(err),
            );
            return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
        }
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    if !device_control_recovery.may_execute() {
        log::error!(
            "[embedded-ble] device-control rejected an otherwise authorized PairAsync recovery; refusing to bypass the transaction result={:?} error={:?}",
            device_control_decision.result,
            device_control_decision.error,
        );
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    if crate::embedded_ble::listener_pairing_maintenance_active() {
        log::warn!(
            "[embedded-ble] background stale pairing cleanup deferred because Listener pairing/cache maintenance is already active"
        );
        return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
    }
    *last_cleanup_at = Some(now);

    let recovery_reason: &'static str = if direct_gatt_instability_recovery {
        EMBEDDED_BLE_DIRECT_GATT_PAIRING_RECOVERY_REASON
    } else {
        EMBEDDED_BLE_STALE_PAIRING_CLEANUP_REASON
    };
    let Some(_pairing_recovery_guard) =
        try_begin_embedded_ble_pairing_recovery(inner, recovery_reason)
    else {
        return EmbeddedBleStalePairingCleanupOutcome::RetrySoon;
    };

    log::warn!(
        "[embedded-ble] background stale pairing cleanup triggered reconnect_attempts={} consecutive_failures={} notify_state={:?} usb_powered={:?} recovery_pairing_window_visible={} direct_gatt_instability_recovery={} err={}",
        snapshot.reconnect_attempts,
        snapshot.consecutive_reconnect_failures,
        snapshot.notify_subscription_state,
        snapshot.usb_powered,
        recovery_pairing_window_visible,
        direct_gatt_instability_recovery,
        embedded_ble_log_preview(err),
    );
    if ec11_type_controlled_recovery {
        log::info!(
            "[embedded-ble] EC11 Type-controlled recovery suppresses pairing-progress capsule until firmware Type-ready terminal confirmation"
        );
    } else {
        emit_embedded_ble_recovery_capsule(
            inner,
            "reconnecting",
            if direct_gatt_instability_recovery {
                EmbeddedBleRecoveryCapsuleMessage::RebuildingPairing
            } else {
                EmbeddedBleRecoveryCapsuleMessage::CleaningPairing
            },
            Some(2600),
        );
    }
    if direct_gatt_instability_recovery {
        let recovery = async_runtime::spawn_blocking(|| {
            crate::embedded_ble::send_recording_control_recovery(Duration::from_secs(5))
        })
        .await;
        match recovery {
            Ok(Ok(())) => log::warn!(
                "[embedded-ble] background direct GATT instability sent Listener recovery pairing command"
            ),
            Ok(Err(err)) => log::warn!(
                "[embedded-ble] background direct GATT instability recovery command failed before Windows pairing cleanup: {}",
                embedded_ble_log_preview(&err),
            ),
            Err(err) => log::warn!(
                "[embedded-ble] background direct GATT instability recovery command task failed: {err}"
            ),
        }
        tokio::time::sleep(EMBEDDED_BLE_TYPE_RECOVERY_PAIRING_SETTLE).await;
    }
    if !embedded_ble_recovery_error_still_current(inner, err, "before_pairing_cleanup") {
        return EmbeddedBleStalePairingCleanupOutcome::Skipped;
    }
    let hold_generation =
        hold_embedded_ble_listener_for_pairing_confirmation(inner, recovery_reason);

    let pairing_expected_name = expected_ble_name.clone();
    let observed_recovery_addresses = if type_controlled_recovery {
        recovery_pairing_addresses_for_cleanup(err, &recovery_pairing_probe)
    } else {
        Vec::new()
    };
    let pairing_recovery_addresses = if type_controlled_recovery {
        recovery_pairing_addresses_for_direct_pairing(err, &recovery_pairing_probe)
    } else {
        Vec::new()
    };
    let pairing = async_runtime::spawn_blocking(move || {
        if type_controlled_recovery {
            let cleanup_names = vec![pairing_expected_name.clone()];
            let direct_pairing_uses_fresh_recovery_address = !pairing_recovery_addresses.is_empty();
            let unpair = if direct_pairing_uses_fresh_recovery_address {
                crate::embedded_ble::unpair_listener_pairing_for_known_addresses(
                    &cleanup_names,
                    &observed_recovery_addresses,
                )
            } else {
                crate::embedded_ble::unpair_listener_devices_for_known_addresses(
                    &cleanup_names,
                    &observed_recovery_addresses,
                )
            };
            log::warn!(
                "[embedded-ble] background Type controlled-recovery known-address cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={} fresh_direct_pairing={} active_capture={active_capture_type_recovery} stale_native_hid_recovery={stale_native_hid_recovery} cleanup_addresses={observed_recovery_addresses:?} pairing_addresses={pairing_recovery_addresses:?}",
                unpair.status,
                unpair.matched_devices,
                unpair.unpaired_devices,
                unpair.already_unpaired_devices,
                unpair.failed_devices,
                unpair.needs_user_action,
                direct_pairing_uses_fresh_recovery_address,
            );
            let pairing = crate::embedded_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
                Some(pairing_expected_name.as_str()),
                &pairing_recovery_addresses,
            );
            if !direct_pairing_uses_fresh_recovery_address
                || embedded_ble_pairing_prompt_ready(&pairing)
            {
                return pairing;
            }

            log::warn!(
                "[embedded-ble] fresh-address direct PairAsync did not complete after pairing-only cleanup; clearing only the exact BTHPORT cache before one final direct PairAsync status={:?} matched={} prompted={} failed={}",
                pairing.status,
                pairing.matched_devices,
                pairing.prompted_devices,
                pairing.failed_devices,
            );
            let fallback_unpair = crate::embedded_ble::clear_listener_bthport_cache_for_known_addresses(
                &cleanup_names,
                &observed_recovery_addresses,
            );
            log::warn!(
                "[embedded-ble] fresh-address direct PairAsync exact BTHPORT cache cleanup status={:?} matched={} removed={} already_clean={} failed={} user_action={}",
                fallback_unpair.status,
                fallback_unpair.matched_devices,
                fallback_unpair.unpaired_devices,
                fallback_unpair.already_unpaired_devices,
                fallback_unpair.failed_devices,
                fallback_unpair.needs_user_action,
            );
            crate::embedded_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
                Some(pairing_expected_name.as_str()),
                &pairing_recovery_addresses,
            )
        } else {
            crate::embedded_ble::prompt_listener_pairing_after_type_recovery(Some(
                pairing_expected_name.as_str(),
            ))
        }
    })
    .await;
    match pairing {
        Ok(pairing) => {
            log::warn!(
                "[embedded-ble] background Type recovery PairAsync result status={:?} matched={} prompted={} already_paired={} failed={} open_settings={}",
                pairing.status,
                pairing.matched_devices,
                pairing.prompted_devices,
                pairing.already_paired_devices,
                pairing.failed_devices,
                pairing.open_bluetooth_settings,
            );

            if embedded_ble_pairing_prompt_ready(&pairing) {
                let terminal = device_control_recovery.complete_pairing();
                log::info!(
                    "[embedded-ble] device-control recovery PairAsync terminal result={:?} error={:?}",
                    terminal.result,
                    terminal.error,
                );
                arm_embedded_ble_type_pairasync_startup_guard(inner);
                if type_controlled_recovery {
                    log::info!(
                        "[embedded-ble] background Type controlled-recovery PairAsync paired; reopening notify immediately for GATT/notify validation active_capture={active_capture_type_recovery} stale_native_hid_recovery={stale_native_hid_recovery}"
                    );
                    resume_embedded_ble_listener_after_pairing_recovery(
                        inner,
                        "background Type recovery PairAsync paired; reopening notify for GATT validation",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        !ec11_type_controlled_recovery,
                    );
                    return EmbeddedBleStalePairingCleanupOutcome::RetryImmediate;
                }

                if embedded_ble_pairing_recovery_link_reachable(
                    inner,
                    "background Type recovery PairAsync paired link check",
                )
                .await
                {
                    resume_embedded_ble_listener_after_pairing_recovery(
                        inner,
                        "background Type recovery PairAsync paired and link reachable",
                        EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                        true,
                    );
                    return EmbeddedBleStalePairingCleanupOutcome::RetryImmediate;
                }

                start_embedded_ble_pairing_confirmation_watch(
                    inner,
                    expected_ble_name.clone(),
                    hold_generation,
                    recovery_reason,
                );
                {
                    let mut wake = inner.embedded_ble_wake_recovery.lock();
                    wake.status = EmbeddedBleWakeRecoveryStatus::Reconnecting;
                    wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Opening;
                    wake.recent_disconnect_reason = Some(format!(
                        "background Type recovery PairAsync completed; waiting for Listener GATT/notify rebuild; previous error: {}",
                        embedded_ble_log_preview(err),
                    ));
                    wake.user_guidance =
                        "Type 已完成本机自动配对，正在等待 Windows BLE GATT/notify 恢复。"
                            .to_string();
                }
                emit_embedded_ble_recovery_capsule(
                    inner,
                    "reconnecting",
                    EmbeddedBleRecoveryCapsuleMessage::LocalPairingRestoringAudio,
                    Some(2600),
                );
                return EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
            }

            let terminal = device_control_recovery
                .fail_without_reclaim(denzic_device_control_v1_core::ErrorCategory::Ownership);
            log::info!(
                "[embedded-ble] device-control recovery terminal result={:?} error={:?}; holding without reclaim",
                terminal.result,
                terminal.error,
            );

            start_embedded_ble_pairing_confirmation_watch(
                inner,
                expected_ble_name.clone(),
                hold_generation,
                recovery_reason,
            );
            {
                let mut wake = inner.embedded_ble_wake_recovery.lock();
                wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
                wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
                wake.recent_disconnect_reason = Some(format!(
                    "background Type recovery PairAsync did not complete status={:?} direct_gatt_instability_recovery={}; if another host paired first, this Type instance must stop instead of stealing it back; previous error: {}",
                    pairing.status,
                    direct_gatt_instability_recovery,
                    embedded_ble_log_preview(err),
                ));
                wake.user_guidance = format!(
                    "Type 没有完成本机自动配对 {expected_ble_name}。如果你已在另一台电脑用 Windows 弹窗连上，这是预期；否则请保持设备可配对后再重试。"
                );
            }
            emit_embedded_ble_recovery_capsule(
                inner,
                "reconnecting",
                EmbeddedBleRecoveryCapsuleMessage::WaitingTypePairing,
                Some(4200),
            );
            EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation
        }
        Err(err) => {
            let terminal = device_control_recovery
                .fail_without_reclaim(denzic_device_control_v1_core::ErrorCategory::Host);
            log::info!(
                "[embedded-ble] device-control recovery task terminal result={:?} error={:?}",
                terminal.result,
                terminal.error,
            );
            log::warn!("[embedded-ble] background Type recovery PairAsync task failed: {err}");
            clear_embedded_ble_pairing_confirmation_hold(
                inner,
                "background Type recovery PairAsync task failed",
            );
            EmbeddedBleStalePairingCleanupOutcome::RetrySoon
        }
    }
}

async fn maybe_probe_embedded_ble_recovery_pairing_advertisement(
    inner: &Arc<Inner>,
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
    let should_probe = should_probe_embedded_ble_recovery_pairing_advertisement(
        err,
        snapshot,
        last_cleanup_at,
        now,
    );
    if !should_probe {
        return crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default();
    }
    if recovery_pairing_advertisement_already_observed_during_notify_open(err) {
        log::info!(
            "[embedded-ble] notify open already observed Listener recovery advertising; skipping duplicate recovery advertisement scan"
        );
        return crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe {
            visible: true,
            // The notify-open path proves that matching recovery advertising is present but does
            // not carry the address type. Treat it as random to retain the conservative
            // multi-host failure behavior; pairing preflight still decides ownership.
            has_random_identity: true,
            addresses: listener_recovery_addresses_from_error(err),
        };
    }
    if embedded_ble_hardware_ec11_recovery_notice_observed(err) {
        log::info!(
            "[embedded-ble] EC11 hardware recovery notice arrived before pairing reset; scanning recovery advertising before any GATT failure timeout"
        );
    }

    let expected_ble_name = inner.prefs.get().device_ble_name;
    match async_runtime::spawn_blocking(move || {
        crate::embedded_ble::listener_recovery_pairing_advertisement_probe(
            Some(&expected_ble_name),
            EMBEDDED_BLE_RECOVERY_PAIRING_ADV_SCAN_TIMEOUT,
        )
    })
    .await
    {
        Ok(probe) => probe,
        Err(err) => {
            log::warn!("[embedded-ble] recovery pairing advertisement probe task failed: {err}");
            crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default()
        }
    }
}

fn should_probe_embedded_ble_recovery_pairing_advertisement(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    if embedded_ble_hardware_ec11_recovery_notice_observed(err) {
        return true;
    }
    if last_cleanup_at.is_some_and(|last| {
        now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
    }) {
        return false;
    }
    if snapshot.reconnect_attempts == 0 {
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
    if matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::DeviceMissing
    ) || (matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::CccdProtocolError
    ) && is_embedded_ble_noisy_cccd_failure(err))
    {
        return true;
    }

    should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
        err,
        snapshot,
        last_cleanup_at,
        now,
    )
}

fn recovery_pairing_advertisement_allows_immediate_stale_cleanup(err: &str) -> bool {
    let failure = crate::embedded_ble::classify_ble_failure(err);
    matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::DeviceMissing
            | crate::embedded_ble::BleFailureKind::StaleGattService
    ) || (matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::CccdProtocolError
    ) && is_embedded_ble_noisy_cccd_failure(err))
}

fn recovery_pairing_advertisement_already_observed_during_notify_open(err: &str) -> bool {
    err.contains("Listener recovery Swift Pair advertisement visible for notify CCCD address")
        || err.contains("Listener recovery Swift Pair advertisement visible for persisted address")
        || recovery_pairing_advertisement_already_observed_during_active_capture(err)
}

fn embedded_ble_hardware_ec11_recovery_notice_observed(err: &str) -> bool {
    err.contains(EMBEDDED_BLE_EC11_HARDWARE_RECOVERY_NOTICE)
}

fn recovery_pairing_advertisement_already_observed_during_active_capture(err: &str) -> bool {
    err.contains("Listener recovery Swift Pair advertisement visible for active capture address")
}

fn listener_recovery_addresses_from_error(err: &str) -> Vec<u64> {
    let mut addresses = Vec::new();
    for token in err.split(|ch: char| !ch.is_ascii_hexdigit()) {
        if token.len() != 12 {
            continue;
        }
        if let Ok(address) = u64::from_str_radix(token, 16) {
            if address != 0 && !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    addresses
}

fn recovery_pairing_addresses_for_cleanup(
    err: &str,
    probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> Vec<u64> {
    let mut addresses = listener_recovery_addresses_from_error(err);
    for address in probe.addresses.iter().copied() {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

fn recovery_pairing_addresses_for_direct_pairing(
    err: &str,
    probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> Vec<u64> {
    if !probe.addresses.is_empty() {
        return probe.addresses.clone();
    }
    listener_recovery_addresses_from_error(err)
}

fn recovery_pairing_probe_allows_immediate_stale_cleanup(
    err: &str,
    probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
    direct_gatt_instability_recovery: bool,
) -> bool {
    probe.visible
        && direct_gatt_instability_recovery
        && (probe.has_random_identity
            || recovery_pairing_advertisement_allows_immediate_stale_cleanup(err))
}

fn noisy_cccd_stale_windows_cache_evidence_allows_type_recovery(
    err: &str,
    stale_cleanup_candidate: bool,
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    stale_cleanup_candidate
        && is_embedded_ble_noisy_cccd_failure(err)
        && matches!(
            pairing.status,
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
        )
        && pairing.open_bluetooth_settings
        && pairing.matched_devices > 0
        && pairing.failed_devices > 0
}

fn native_windows_hid_pairing_blocks_type_pairasync(
    native_windows_hid_pairing_visible: bool,
    recovery_pairing_probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
    pairing: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
) -> bool {
    if !native_windows_hid_pairing_visible {
        return false;
    }

    // A random recovery advertisement plus a Windows record that cannot be
    // reopened is a stale local pairing key, not a live HID takeover. The
    // physical double-click is allowed to rebuild that key through PairAsync.
    let stale_native_hid_evidence = recovery_pairing_probe.visible
        && recovery_pairing_probe.has_random_identity
        && pairing.is_some_and(|pairing| {
            pairing.already_paired_devices == 0
                && pairing.matched_devices > 0
                && pairing.failed_devices > 0
                && matches!(
                    pairing.status,
                    crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
                )
                && pairing.open_bluetooth_settings
        });
    !stale_native_hid_evidence
}

fn should_hold_embedded_ble_background_recovery_after_manual_unpair(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
    direct_gatt_instability_recovery: bool,
    recovery_advertisement_type_owned_cleanup: bool,
) -> bool {
    if direct_gatt_instability_recovery || recovery_advertisement_type_owned_cleanup {
        return false;
    }
    if pairing.already_paired_devices > 0 {
        return false;
    }
    if is_explicit_manual_windows_delete_pairing_state(pairing) {
        return true;
    }
    if pairing.matched_devices > 0 || pairing.failed_devices > 0 {
        return false;
    }
    matches!(
        pairing.status,
        crate::embedded_ble::BleDevicePairingPromptStatus::NotFound
            | crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
    )
}

fn is_explicit_manual_windows_delete_pairing_state(
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    pairing.already_paired_devices == 0
        && matches!(
            pairing.status,
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction
        )
        && pairing.open_bluetooth_settings
        && pairing.failed_devices > 0
}

async fn hold_embedded_ble_for_manual_windows_unpair(inner: &Arc<Inner>, expected_ble_name: &str) {
    log::warn!(
        "[embedded-ble] background stale pairing cleanup suppressed automatic PairAsync because Windows no longer reports a paired Listener; treating this as user/manual pairing removal target={expected_ble_name:?}"
    );
    let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
        inner,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
    let firmware_recovery = async_runtime::spawn_blocking(|| {
        crate::embedded_ble::send_recording_control_manual_pairing(Duration::from_secs(3))
    })
    .await;
    match firmware_recovery {
        Ok(Ok(())) => {
            log::warn!(
                "[embedded-ble] manual Windows unpair sent Listener manual-pairing recovery cue without Windows PairAsync"
            );
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
        Ok(Err(err)) => log::warn!(
            "[embedded-ble] manual Windows unpair Listener recovery command failed; still suppressing Windows PairAsync: {}",
            embedded_ble_log_preview(&err),
        ),
        Err(err) => log::warn!(
            "[embedded-ble] manual Windows unpair Listener recovery command task failed; still suppressing Windows PairAsync: {err}"
        ),
    }
    start_embedded_ble_pairing_confirmation_watch(
        inner,
        expected_ble_name.to_string(),
        hold_generation,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
    {
        let mut wake = inner.embedded_ble_wake_recovery.lock();
        wake.status = EmbeddedBleWakeRecoveryStatus::Failed;
        wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
        wake.recent_disconnect_reason = Some(format!(
            "Windows pairing was removed manually; Type opened Listener pairing window and suppressed automatic PairAsync target={expected_ble_name}"
        ));
        wake.user_guidance = format!(
            "Windows 已删除 {expected_ble_name} 的配对。Type 已让 Listener 进入可重新配对状态，但不会自动抢回连接；如果还要在这台电脑使用，请在 Windows 蓝牙里手动添加设备。"
        );
    }
    emit_embedded_ble_recovery_capsule(
        inner,
        "reconnecting",
        EmbeddedBleRecoveryCapsuleMessage::WaitingManualPairing,
        Some(4200),
    );
}

fn embedded_ble_lost_current_native_pairing_should_pause(
    err: &str,
    native_hid_addresses: &[u64],
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    is_embedded_ble_link_loss_error(err)
        && embedded_ble_current_native_pairing_is_missing(native_hid_addresses, pairing)
}

fn embedded_ble_current_native_pairing_is_missing(
    native_hid_addresses: &[u64],
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
) -> bool {
    native_hid_addresses.is_empty() && pairing.already_paired_devices == 0
}

fn hold_embedded_ble_for_missing_native_pairing(inner: &Arc<Inner>, reason: &str) {
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
        inner,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
    log::warn!(
        "[embedded-ble] current native Windows Listener HID is absent; releasing stale GATT and waiting for explicit local re-pair hold_generation={hold_generation} target={expected_ble_name:?} reason={}",
        embedded_ble_log_preview(reason),
    );
    {
        let mut wake = inner.embedded_ble_wake_recovery.lock();
        wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
        wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
        wake.recent_disconnect_reason = Some(format!(
            "current native Windows Listener HID disappeared; waiting for explicit local re-pair target={expected_ble_name}"
        ));
        wake.user_guidance = format!(
            "Listener 正在等待 Windows 重新配对 {expected_ble_name}。Type 已释放旧蓝牙连接，不会自动抢回设备。"
        );
    }
    emit_embedded_ble_recovery_capsule(
        inner,
        "reconnecting",
        EmbeddedBleRecoveryCapsuleMessage::WaitingManualPairing,
        Some(4200),
    );
    start_embedded_ble_passive_local_reattach_watch(
        inner,
        expected_ble_name,
        EMBEDDED_BLE_MANUAL_UNPAIR_HOLD_REASON,
    );
}

fn hold_embedded_ble_for_stale_native_hid_recovery(
    inner: &Arc<Inner>,
    reason: &str,
    native_hid_addresses: Vec<u64>,
    pairing: crate::embedded_ble::BleDevicePairingPromptResult,
) {
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let hold_generation = hold_embedded_ble_listener_for_pairing_confirmation(
        inner,
        EMBEDDED_BLE_STALE_NATIVE_HID_RECOVERY_WAIT_REASON,
    );
    log::warn!(
        "[embedded-ble] stale native Windows HID detected; waiting for a real Listener recovery advertisement before bounded local PairAsync hold_generation={hold_generation} target={expected_ble_name:?} reason={}",
        embedded_ble_log_preview(reason),
    );
    {
        let mut wake = inner.embedded_ble_wake_recovery.lock();
        wake.status = EmbeddedBleWakeRecoveryStatus::NeedsWakeKey;
        wake.notify_subscription_state = EmbeddedBleNotifySubscriptionState::Cancelled;
        wake.recent_disconnect_reason = Some(format!(
            "stale native Windows HID is waiting for Listener recovery advertising target={expected_ble_name}"
        ));
        wake.user_guidance =
            "Listener 正在等待设备重新进入恢复广播；检测到后会自动恢复本机蓝牙连接。".to_string();
    }
    emit_embedded_ble_recovery_capsule(
        inner,
        "reconnecting",
        EmbeddedBleRecoveryCapsuleMessage::WaitingTypePairing,
        Some(4200),
    );
    start_embedded_ble_stale_native_hid_recovery_watch(
        inner,
        expected_ble_name,
        hold_generation,
        native_hid_addresses,
        pairing,
    );
}

fn start_embedded_ble_stale_native_hid_recovery_watch(
    inner: &Arc<Inner>,
    expected_ble_name: String,
    hold_generation: u64,
    native_hid_addresses: Vec<u64>,
    pairing: crate::embedded_ble::BleDevicePairingPromptResult,
) {
    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        log::info!(
            "[embedded-ble] stale native HID recovery advertisement watch started target={expected_ble_name:?} hold_generation={hold_generation}"
        );
        if inner.shutdown.load(Ordering::SeqCst)
            || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            || inner
                .embedded_ble_pairing_hold_generation
                .load(Ordering::SeqCst)
                != hold_generation
        {
            return;
        }

        // Keep one watcher alive for the bounded pairing window. Restarting four-second
        // scans introduced a blind gap after a physical EC11 double-click, even though the
        // startup preflight had already proved this PC owns only a stale local HID record.
        let expected_for_scan = expected_ble_name.clone();
        let recovery_pairing_probe = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::listener_recovery_pairing_advertisement_probe(
                Some(&expected_for_scan),
                EMBEDDED_BLE_PAIRING_CONFIRMATION_HOLD,
            )
        })
        .await;
        let Ok(recovery_pairing_probe) = recovery_pairing_probe else {
            log::warn!(
                "[embedded-ble] stale native HID recovery advertisement watch task failed; keeping passive ownership"
            );
            return;
        };
        if !recovery_pairing_probe.visible
            || inner.shutdown.load(Ordering::SeqCst)
            || inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle
            || inner
                .embedded_ble_pairing_hold_generation
                .load(Ordering::SeqCst)
                != hold_generation
        {
            return;
        }
        if !startup_stale_native_hid_recovery_is_authorized(
            &native_hid_addresses,
            &pairing,
            &recovery_pairing_probe,
        ) {
            log::info!(
                "[embedded-ble] recovery advertising is visible but cached stale-HID ownership proof is incomplete; preserving passive ownership native_hid_count={} status={:?} matched={} already_paired={} failed={}",
                native_hid_addresses.len(),
                pairing.status,
                pairing.matched_devices,
                pairing.already_paired_devices,
                pairing.failed_devices,
            );
            return;
        }

        let observed_addresses = recovery_pairing_probe
            .addresses
            .iter()
            .map(|address| format!("{address:012X}"))
            .collect::<Vec<_>>();
        let recovery_error = format!(
            "Listener recovery Swift Pair advertisement visible for persisted address {} after stale native HID recovery watch; missing pairing must use Type automatic PairAsync recovery before declaring notify ready",
            observed_addresses.join(",")
        );
        log::warn!(
            "[embedded-ble] stale native HID recovery watch used startup stale-pairing proof and active recovery advertising; starting bounded Type PairAsync recovery recovery_addresses={observed_addresses:?}"
        );
        clear_embedded_ble_pairing_confirmation_hold(
            &inner,
            "stale native HID recovery advertisement observed",
        );
        record_embedded_ble_listener_last_error(&inner, &recovery_error);
        let mut cleanup_at = None;
        let outcome = maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
            &inner,
            &recovery_error,
            &mut cleanup_at,
        )
        .await;
        log::info!(
            "[embedded-ble] stale native HID recovery watch Type recovery outcome={outcome:?}"
        );
    });
}

async fn maybe_hold_embedded_ble_after_lost_native_pairing(inner: &Arc<Inner>, err: &str) -> bool {
    if !is_embedded_ble_link_loss_error(err)
        || embedded_ble_type_pairasync_startup_guard_active(inner)
    {
        return false;
    }
    let native_hid_addresses = match async_runtime::spawn_blocking(|| {
        crate::embedded_ble::native_windows_hid_pairing_addresses()
    })
    .await
    {
        Ok(Ok(addresses)) => addresses,
        Ok(Err(query_err)) => {
            log::debug!(
                "[embedded-ble] current native Windows HID check unavailable during link recovery: {query_err}"
            );
            return false;
        }
        Err(query_err) => {
            log::debug!(
                "[embedded-ble] current native Windows HID task failed during link recovery: {query_err}"
            );
            return false;
        }
    };
    if !native_hid_addresses.is_empty() {
        return false;
    }

    let expected_ble_name = inner.prefs.get().device_ble_name;
    let expected_for_query = expected_ble_name.clone();
    let pairing = match async_runtime::spawn_blocking(move || {
        crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
    })
    .await
    {
        Ok(result) => result,
        Err(query_err) => {
            log::debug!(
                "[embedded-ble] Windows pairing query task failed during link recovery: {query_err}"
            );
            return false;
        }
    };
    if !embedded_ble_lost_current_native_pairing_should_pause(err, &native_hid_addresses, &pairing)
    {
        return false;
    }

    log::info!(
        "[embedded-ble] link recovery found no current native HID or paired Listener status={:?} matched={} already_paired={}; suppressing stale GATT retry",
        pairing.status,
        pairing.matched_devices,
        pairing.already_paired_devices,
    );
    hold_embedded_ble_for_missing_native_pairing(inner, err);
    true
}

async fn maybe_hold_embedded_ble_startup_without_current_native_pairing(
    inner: &Arc<Inner>,
) -> bool {
    if embedded_ble_type_pairasync_startup_guard_active(inner) {
        log::info!(
            "[embedded-ble] startup manual-delete pairing preflight deferred while Type PairAsync services rebuild"
        );
        return false;
    }
    let expected_ble_name = inner.prefs.get().device_ble_name;
    let started_at = Instant::now();
    let native_hid_addresses = match async_runtime::spawn_blocking(|| {
        crate::embedded_ble::native_windows_hid_present_pairing_addresses_for_startup()
    })
    .await
    {
        Ok(Ok(addresses)) => addresses,
        Ok(Err(err)) => {
            log::warn!(
                "[embedded-ble] startup native Windows HID pairing evidence unavailable; preserving persisted GATT path: {err}"
            );
            return false;
        }
        Err(err) => {
            log::warn!(
                "[embedded-ble] startup native Windows HID pairing evidence task failed; preserving persisted GATT path: {err}"
            );
            return false;
        }
    };
    crate::startup_evidence::record_startup_stage("native_hid_pnp_ready");
    if !native_hid_addresses.is_empty() {
        let active_addresses = native_hid_addresses.clone();
        let active_connection = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::native_windows_hid_pairing_active_connection(&active_addresses)
        })
        .await;
        crate::startup_evidence::record_startup_stage("native_hid_active_connection_finished");
        match active_connection {
            Ok(Ok(Some(address))) => {
                log::info!(
                    "[embedded-ble] startup native Windows HID active connection allows persisted GATT reopen address={address:012X}; ignoring incomplete paired-device enumeration"
                );
                return false;
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => log::warn!(
                "[embedded-ble] startup native Windows HID active-connection probe unavailable; keeping manual-delete preflight: {err}"
            ),
            Err(err) => log::warn!(
                "[embedded-ble] startup native Windows HID active-connection probe task failed; keeping manual-delete preflight: {err}"
            ),
        }
    }
    let expected_for_query = expected_ble_name.clone();
    let query = async_runtime::spawn_blocking(move || {
        crate::embedded_ble::query_listener_pairing(Some(&expected_for_query))
    })
    .await;
    let pairing = match query {
        Ok(pairing) => pairing,
        Err(err) => {
            log::warn!(
                "[embedded-ble] startup manual-delete pairing preflight task failed; preserving persisted GATT fast path: {err}"
            );
            return false;
        }
    };
    log::info!(
        "[embedded-ble] startup manual-delete pairing preflight status={:?} matched={} already_paired={} failed={} open_settings={} elapsed_ms={}",
        pairing.status,
        pairing.matched_devices,
        pairing.already_paired_devices,
        pairing.failed_devices,
        pairing.open_bluetooth_settings,
        started_at.elapsed().as_millis(),
    );
    if !native_hid_addresses.is_empty() {
        let labels = native_hid_addresses
            .iter()
            .map(|address| format!("{address:012X}"))
            .collect::<Vec<_>>();
        if pairing.already_paired_devices > 0 {
            log::info!(
                "[embedded-ble] startup native Windows HID/current pairing evidence allows persisted GATT reopen addresses={labels:?} elapsed_ms={}",
                started_at.elapsed().as_millis(),
            );
            return false;
        }
        let expected_for_scan = expected_ble_name.clone();
        let recovery_pairing_probe = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::listener_recovery_pairing_advertisement_probe(
                Some(&expected_for_scan),
                EMBEDDED_BLE_RECOVERY_PAIRING_ADV_SCAN_TIMEOUT,
            )
        })
        .await
        .unwrap_or_else(|err| {
            log::warn!(
                "[embedded-ble] startup stale-HID recovery advertisement probe task failed: {err}"
            );
            crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe::default()
        });
        if startup_stale_native_hid_recovery_is_authorized(
            &native_hid_addresses,
            &pairing,
            &recovery_pairing_probe,
        ) {
            let observed_addresses = recovery_pairing_probe
                .addresses
                .iter()
                .map(|address| format!("{address:012X}"))
                .collect::<Vec<_>>();
            let recovery_error = format!(
                "Listener recovery Swift Pair advertisement visible for persisted address {} after startup stale native HID pairing; missing pairing must use Type automatic PairAsync recovery before declaring notify ready",
                observed_addresses.join(",")
            );
            log::warn!(
                "[embedded-ble] startup stale native HID pairing matches active Listener recovery advertising; entering bounded local Type PairAsync recovery native_hid_addresses={labels:?} recovery_addresses={observed_addresses:?}"
            );
            record_embedded_ble_listener_last_error(inner, &recovery_error);
            let mut startup_cleanup_at = None;
            let outcome = maybe_attempt_embedded_ble_background_stale_pairing_cleanup(
                inner,
                &recovery_error,
                &mut startup_cleanup_at,
            )
            .await;
            log::info!("[embedded-ble] startup stale native HID recovery outcome={outcome:?}");
            return outcome == EmbeddedBleStalePairingCleanupOutcome::HoldForConfirmation;
        }
        log::warn!(
            "[embedded-ble] startup stale native HID has no matching recovery advertisement; preserving user/external ownership native_hid_addresses={labels:?}"
        );
        hold_embedded_ble_for_stale_native_hid_recovery(
            inner,
            "startup found stale native Windows HID without current Listener recovery advertising",
            native_hid_addresses,
            pairing,
        );
        return true;
    }
    if !embedded_ble_current_native_pairing_is_missing(&native_hid_addresses, &pairing) {
        return false;
    }
    log::warn!(
        "[embedded-ble] startup current native Windows pairing is absent; blocking persisted GATT reopen target={expected_ble_name:?}"
    );
    hold_embedded_ble_for_missing_native_pairing(
        inner,
        "startup found no current native Windows Listener HID or paired device",
    );
    true
}

fn startup_stale_native_hid_recovery_is_authorized(
    native_hid_addresses: &[u64],
    pairing: &crate::embedded_ble::BleDevicePairingPromptResult,
    recovery_pairing_probe: &crate::embedded_ble::ListenerRecoveryPairingAdvertisementProbe,
) -> bool {
    !native_hid_addresses.is_empty()
        && recovery_pairing_probe.visible
        && !native_windows_hid_pairing_blocks_type_pairasync(
            true,
            recovery_pairing_probe,
            Some(pairing),
        )
}

fn embedded_ble_background_pairasync_is_authorized(
    manual_unpair_hold: bool,
    type_observed_recovery_advertisement: bool,
    visible_recovery_allows_cleanup: bool,
    stale_cleanup_candidate: bool,
    noisy_cccd_stale_cache_type_owned_cleanup: bool,
    local_stale_cache_recovery_allows_cleanup: bool,
    firmware_name_changed: bool,
) -> bool {
    // A manual Windows removal is explicit user ownership. Background heuristics
    // must never turn it into an automatic re-pair of the old computer.
    !manual_unpair_hold
        && (type_observed_recovery_advertisement
            || visible_recovery_allows_cleanup
            || stale_cleanup_candidate
            || noisy_cccd_stale_cache_type_owned_cleanup
            || local_stale_cache_recovery_allows_cleanup
            || firmware_name_changed)
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

fn should_attempt_embedded_ble_background_direct_gatt_pairing_recovery(
    err: &str,
    snapshot: &EmbeddedBleWakeRecoverySnapshot,
    last_cleanup_at: Option<Instant>,
    now: Instant,
) -> bool {
    if last_cleanup_at.is_some_and(|last| {
        now.saturating_duration_since(last) < EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_COOLDOWN
    }) {
        return false;
    }
    if snapshot.consecutive_reconnect_failures
        < EMBEDDED_BLE_BACKGROUND_DIRECT_GATT_PAIRING_ATTEMPT_THRESHOLD
    {
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
    if !is_embedded_ble_link_loss_error(err) || is_embedded_ble_low_power_idle_candidate(err) {
        return false;
    }
    let failure = crate::embedded_ble::classify_ble_failure(err);
    matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::PairedButDisconnected
            | crate::embedded_ble::BleFailureKind::Unknown
    )
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
    let reconnect_attempt_threshold = match failure.kind {
        crate::embedded_ble::BleFailureKind::CccdProtocolError
            if is_embedded_ble_noisy_cccd_failure(err) =>
        {
            EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD
        }
        crate::embedded_ble::BleFailureKind::StaleGattService
        | crate::embedded_ble::BleFailureKind::MissingPairing => {
            EMBEDDED_BLE_BACKGROUND_STALE_CLEANUP_ATTEMPT_THRESHOLD
        }
        _ => return false,
    };

    snapshot.consecutive_reconnect_failures >= reconnect_attempt_threshold
}

fn next_embedded_ble_background_retry_delay(err: &str, current: Duration) -> Duration {
    if is_embedded_ble_idle_timeout_error(err) {
        return EMBEDDED_BLE_RETRY_BASE_DELAY;
    }

    if is_embedded_ble_link_loss_error(err) {
        return EMBEDDED_BLE_RETRY_FAST_DELAY;
    }

    if is_embedded_ble_noisy_cccd_failure(err) {
        return EMBEDDED_BLE_RETRY_NOISY_CCCD_DELAY;
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
    denzic_ble_windows::failure::is_ble_offline_backoff_error(err)
}

fn is_embedded_ble_noisy_cccd_failure(err: &str) -> bool {
    denzic_ble_windows::failure::is_ble_noisy_cccd_failure(
        err,
        &crate::embedded_ble::LISTENER_BLE_FAILURE_HINTS,
    )
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
    let noisy_cccd = is_embedded_ble_noisy_cccd_failure(err);
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
        && !noisy_cccd
        && reconnect_attempts <= 1
        && low_power_recovery_capsule_allowed;
    let decision_reason = if !automatic_recovery {
        "not_automatic_recovery"
    } else if offline_backoff {
        "offline_backoff"
    } else if noisy_cccd {
        "noisy_cccd_retry"
    } else if reconnect_attempts > 1 {
        "repeat_attempt_suppressed"
    } else if low_power_idle && usb_powered != Some(true) {
        "low_power_idle_power_unknown_or_battery_suppressed"
    } else {
        "emit"
    };
    log::info!(
        "[embedded-ble] background recovery capsule decision emit={} reason={} kind={:?} automatic_recovery={} link_loss={} offline_backoff={} noisy_cccd={} reconnect_attempts={} usb_powered={:?} low_power_idle={} err={}",
        decision,
        decision_reason,
        failure.kind,
        failure.automatic_recovery,
        link_loss,
        offline_backoff,
        noisy_cccd,
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
    // 对齐 should_emit_embedded_ble_background_recovery_capsule：仅真正的链路恢复
    // (link loss / automatic recovery) 才弹"已恢复"胶囊。所有权冲突等非链路原因
    // (例如"当前已有听写会话在运行，暂不能提交嵌入式音频")并不是链路掉线后的恢复,
    // 若每次 notify-ready 都弹,在重连循环里会变成胶囊刷屏——故对非链路原因一律不弹。
    automatic_recovery
        && !repeated_automatic_recovery
        && (!is_embedded_ble_low_power_idle_candidate(reason) || usb_powered == Some(true))
}

fn is_embedded_ble_link_loss_error(err: &str) -> bool {
    denzic_ble_windows::failure::is_ble_link_loss_error(err)
}

fn is_embedded_ble_transient_reopen_error(err: &str) -> bool {
    denzic_ble_windows::failure::is_ble_transient_reopen_error(err)
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
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
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
        // Create the waiter before the second ready check: if CCCD completes in
        // this narrow interval, either the state check succeeds or Notify keeps
        // the wake-up permit. This preserves the timeout diagnostic without a
        // polling-sized delay after the actual TYPE:READY edge.
        let ready_notification = inner.embedded_ble_listener_ready_notification.notified();
        if embedded_ble_listener_capture_ready(inner) {
            return Ok(());
        }
        tokio::select! {
            _ = ready_notification => {}
            _ = tokio::time::sleep(remaining) => {
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
        }
    }
}

async fn wait_for_embedded_ble_listener_inactive(inner: &Arc<Inner>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !embedded_ble_listener_capture_active(inner)
            && !crate::embedded_ble::notify_capture_session_active()
        {
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
        inner
            .embedded_ble_listener_ready_notification
            .notify_waiters();
        clear_embedded_ble_listener_last_error(inner);
        let recovered = record_embedded_ble_notify_ready(inner);
        if let Some(hid_observed_at) = inner.embedded_ble_power_cycle_hid_observed_at.lock().take()
        {
            let elapsed = hid_observed_at.elapsed();
            let elapsed_ms = elapsed.as_millis();
            let target_ms = EMBEDDED_BLE_POWER_CYCLE_AUDIO_RECOVERY_TARGET.as_millis();
            log::info!(
                "[embedded-ble] power-cycle audio notify recovery elapsed_ms={elapsed_ms} target_ms={target_ms} met={}",
                elapsed <= EMBEDDED_BLE_POWER_CYCLE_AUDIO_RECOVERY_TARGET
            );
        }
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
                EmbeddedBleRecoveryCapsuleMessage::AudioRecovered,
                Some(1400),
            );
        }
        flush_pending_device_key_ble_action(inner, "notify_ready");
        log::info!("[embedded-ble] background listener notify ready");
    }
}

fn install_embedded_ble_listener_cancel(inner: &Arc<Inner>, generation: u64) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let leave_notify_cccd_enabled_on_cancel = Arc::new(AtomicBool::new(false));
    inner
        .embedded_ble_listener_ready
        .store(false, Ordering::SeqCst);
    let previous = {
        let mut slot = inner.embedded_ble_listener_cancel.lock();
        slot.replace(Arc::clone(&cancel))
    };
    *inner
        .embedded_ble_listener_leave_cccd_enabled_on_cancel
        .lock() = Some((
        Arc::clone(&cancel),
        Arc::clone(&leave_notify_cccd_enabled_on_cancel),
    ));
    if let Some(previous) = previous {
        previous.store(true, Ordering::SeqCst);
        log::warn!(
            "[embedded-ble] replaced an active background capture cancel flag (generation={generation})"
        );
    }
    log::info!("[embedded-ble] background capture armed generation={generation}");
    cancel
}

fn embedded_ble_listener_cccd_handoff_flag(
    inner: &Arc<Inner>,
    cancel: &Arc<AtomicBool>,
) -> Arc<AtomicBool> {
    inner
        .embedded_ble_listener_leave_cccd_enabled_on_cancel
        .lock()
        .as_ref()
        .filter(|(active_cancel, _)| Arc::ptr_eq(active_cancel, cancel))
        .map(|(_, handoff)| Arc::clone(handoff))
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
}

fn clear_embedded_ble_listener_cancel(inner: &Arc<Inner>, cancel: &Arc<AtomicBool>) {
    let cleared = {
        let mut slot = inner.embedded_ble_listener_cancel.lock();
        if slot
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, cancel))
        {
            *slot = None;
            true
        } else {
            false
        }
    };
    if cleared {
        let mut handoff_slot = inner
            .embedded_ble_listener_leave_cccd_enabled_on_cancel
            .lock();
        if handoff_slot
            .as_ref()
            .is_some_and(|(active_cancel, _)| Arc::ptr_eq(active_cancel, cancel))
        {
            *handoff_slot = None;
        }
        inner
            .embedded_ble_listener_ready
            .store(false, Ordering::SeqCst);
        log::info!("[embedded-ble] background capture cancel flag cleared");
    }
}

fn cancel_embedded_ble_listener_capture(
    inner: &Arc<Inner>,
    reason: &str,
    leave_notify_cccd_enabled_on_cancel: bool,
) {
    inner
        .embedded_ble_listener_ready
        .store(false, Ordering::SeqCst);
    let previous = inner.embedded_ble_listener_cancel.lock().take();
    if let Some(cancel) = previous {
        if leave_notify_cccd_enabled_on_cancel {
            if let Some((active_cancel, handoff)) = inner
                .embedded_ble_listener_leave_cccd_enabled_on_cancel
                .lock()
                .as_ref()
            {
                if Arc::ptr_eq(active_cancel, &cancel) {
                    handoff.store(true, Ordering::SeqCst);
                    log::info!(
                        "[embedded-ble] confirmed BLE-name change will leave old notify CCCD enabled for firmware disconnect handoff"
                    );
                }
            }
        }
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
    cancel_embedded_ble_listener_capture(inner, reason, false);
}

fn pause_embedded_ble_listener_capture_for_ble_name_apply_handoff(inner: &Arc<Inner>) {
    let generation = inner
        .embedded_ble_listener_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    log::info!(
        "[embedded-ble] paused background listener generation={generation} (BLE name apply handoff)"
    );
    cancel_embedded_ble_listener_capture(inner, "BLE name apply handoff", true);
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
                let phase = inner_cloned.state.lock().phase;
                log::info!("[coord] global hotkey cancel received phase={phase:?}");
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
    allow_clipboard_fallback: bool,
    paste_shortcut: PasteShortcut,
    ime_target: Option<ImeSubmitTarget>,
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
            return insert_via_non_tsf_fallback(
                inner,
                polished,
                restore_clipboard,
                allow_clipboard_fallback,
                paste_shortcut,
            );
        }
        log::warn!("[windows-ime] non-TSF insertion fallback is disabled; failing insert");
        return WindowsInsertionResult {
            status: InsertStatus::Failed,
            target_confirmed: false,
        };
    };

    let request = crate::windows_ime_ipc::ImeSubmitRequest {
        session_id: Uuid::new_v4().to_string(),
        text: polished.to_string(),
        created_at: Utc::now().to_rfc3339(),
        target: ime_target,
    };

    let ime_status = match inner.windows_ime.submit_prepared(&prepared, request).await {
        Ok(status) => status,
        Err(error) if error.is_session_not_active() => {
            // session not active：录音起点 prepare_session 就没激活 Listener Type profile。
            // 目标窗口仍是用户原 IME，会拦截 SendInput 的 Unicode 事件（insert_via_non_tsf_fallback
            // 里 SendInput 假阳性返回 Inserted，实际没打字，永远到不了 clipboard 分支）。
            // 剪贴板里此时已有原文，直接 Ctrl+V 走目标窗口 paste handler 绕开 IME。
            log::warn!("[windows-ime] TSF submit failed: {error}");
            inner.windows_ime.restore_session(prepared);
            if allow_clipboard_fallback {
                return WindowsInsertionResult {
                    status: inner.inserter.insert_via_clipboard_fallback(
                        polished,
                        restore_clipboard,
                        paste_shortcut,
                    ),
                    target_confirmed: false,
                };
            }
            // 不允许 clipboard 兜底（用户关了剪贴板留存）：退回 SendInput，保持旧行为。
            return insert_via_non_tsf_fallback(
                inner,
                polished,
                restore_clipboard,
                allow_clipboard_fallback,
                paste_shortcut,
            );
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
        }
    } else if should_try_non_tsf_insertion_fallback(allow_non_tsf_insertion_fallback, ime_status) {
        insert_via_non_tsf_fallback(
            inner,
            polished,
            restore_clipboard,
            allow_clipboard_fallback,
            paste_shortcut,
        )
    } else {
        log::warn!("[windows-ime] TSF did not insert; non-TSF insertion fallback is disabled");
        WindowsInsertionResult {
            status: InsertStatus::Failed,
            target_confirmed: false,
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
    allow_clipboard_fallback: bool,
    paste_shortcut: PasteShortcut,
) -> WindowsInsertionResult {
    if inner.inserter.insert_via_unicode_keystrokes(polished) == InsertStatus::Inserted {
        log::info!(
            "[windows-ime] TSF unavailable; Unicode SendInput dispatched without target confirmation"
        );
        WindowsInsertionResult {
            status: InsertStatus::Inserted,
            target_confirmed: false,
        }
    } else if !allow_clipboard_fallback {
        log::warn!("[windows-ime] clipboard fallback disabled by final clipboard preference");
        WindowsInsertionResult {
            status: InsertStatus::Failed,
            target_confirmed: false,
        }
    } else {
        WindowsInsertionResult {
            status: inner.inserter.insert_via_clipboard_fallback(
                polished,
                restore_clipboard,
                paste_shortcut,
            ),
            target_confirmed: false,
        }
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

    let api_key = CredentialsVault::get(CredentialAccount::ArkApiKey)?.unwrap_or_default();
    let model = model.unwrap_or_else(|| "deepseek-v3-2".to_string());
    let endpoint = resolve_ark_endpoint(&active, &api_key)?;
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
